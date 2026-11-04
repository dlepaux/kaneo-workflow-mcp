//! The built binary over stdio, as an MCP client runs it: handshake, the tool
//! list, and one call that reaches a mock Kaneo.

use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_kaneo-workflow-mcp");

struct Session {
    _child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
}

impl Session {
    fn spawn(url: &str) -> Self {
        let mut child = Command::new(BIN)
            .env("KANEO_API_URL", url)
            .env("KANEO_API_KEY", "test-key")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        Self {
            _child: child,
            stdin,
            stdout,
        }
    }

    async fn send(&mut self, msg: Value) {
        let line = format!("{msg}\n");
        self.stdin.write_all(line.as_bytes()).await.unwrap();
    }

    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        // Bounded: the server answers each request with exactly one line.
        for _ in 0..16 {
            let line = self
                .stdout
                .next_line()
                .await
                .unwrap()
                .expect("server closed stdout");
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == json!(id) {
                return msg;
            }
        }
        panic!("no response to request {id}");
    }

    async fn handshake(&mut self) {
        let init = self
            .request(
                1,
                "initialize",
                json!({"protocolVersion": "2025-06-18", "capabilities": {},
                       "clientInfo": {"name": "test", "version": "0"}}),
            )
            .await;
        assert_eq!(init["result"]["serverInfo"]["name"], "kaneo-workflow-mcp");
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
    }
}

#[tokio::test]
async fn lists_exactly_its_tools() {
    let mut s = Session::spawn("http://127.0.0.1:9");
    s.handshake().await;
    let list = s.request(2, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    // MCP requires an object input schema for every tool, the argument-less
    // `projects` included.
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
    }
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "board_digest",
            "card_activity",
            "card_brief",
            "close_card",
            "comment",
            "decide",
            "delete_comment",
            "edit_card",
            "edit_comment",
            "edit_project_description",
            "file_card",
            "find",
            "label",
            "link",
            "list_cards",
            "move_card",
            "pause_card",
            "project_info",
            "projects",
            "start_card",
            "unlabel",
            "unlink"
        ]
    );
}

#[tokio::test]
async fn a_write_answers_ok_id_and_an_error_is_a_tool_error() {
    let kaneo = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/task/by-ticket-id/WEB-58"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "t58", "title": "t", "number": 58, "status": "doing", "projectId": "p"
        })))
        .mount(&kaneo)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/comment/t58"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "c1"})))
        .mount(&kaneo)
        .await;

    let mut s = Session::spawn(&kaneo.uri());
    s.handshake().await;
    let ok = s
        .request(
            2,
            "tools/call",
            json!({"name": "comment", "arguments": {"ticket": "WEB-58", "text": "hello"}}),
        )
        .await;
    assert_eq!(ok["result"]["content"][0]["text"], "ok c1");
    assert_ne!(ok["result"]["isError"], json!(true));

    let bad = s
        .request(
            3,
            "tools/call",
            json!({"name": "comment", "arguments": {"ticket": "nope", "text": "hello"}}),
        )
        .await;
    assert_eq!(bad["result"]["isError"], json!(true));
    assert_eq!(
        bad["result"]["content"][0]["text"],
        "error: ticket \"nope\" is not of the form ABC-12"
    );
}

#[tokio::test]
async fn exits_with_a_message_when_the_env_is_missing() {
    let out = Command::new(BIN)
        .env_remove("KANEO_API_URL")
        .env("KANEO_API_KEY", "k")
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "kaneo-workflow-mcp: KANEO_API_URL is not set"
    );
}

#[tokio::test]
async fn answers_version_without_any_env_and_refuses_unknown_arguments() {
    let run = |arg: &'static str| {
        Command::new(BIN)
            .arg(arg)
            .env_remove("KANEO_API_URL")
            .env_remove("KANEO_API_KEY")
            .stdin(Stdio::null())
            .output()
    };
    let version = run("--version").await.unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        concat!("kaneo-workflow-mcp ", env!("CARGO_PKG_VERSION"))
    );
    let unknown = run("--serve").await.unwrap();
    assert_eq!(unknown.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unknown argument \"--serve\""));
}
