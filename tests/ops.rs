//! Each tool against a mock Kaneo. Fixtures copy the shape of real Kaneo
//! responses (October 2026), trimmed to what the tools read plus a few fields
//! they must ignore. No test here reaches a real instance. Every write is
//! checked for its exact method, path and body.

use chrono::{DateTime, TimeZone, Utc};
use kaneo_workflow_mcp::api::KaneoClient;
use kaneo_workflow_mcp::ops::{self, CardEdit, ListQuery, NewCard, Pause};
use kaneo_workflow_mcp::workflow::Decision;
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WS: &str = "ws1";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 27, 10, 5, 0).unwrap()
}

fn client(server: &MockServer) -> KaneoClient {
    KaneoClient::new(&format!("{}/api", server.uri()), "test-key".into()).unwrap()
}

fn task(id: &str, number: u32, status: &str) -> Value {
    json!({
        "id": id, "title": format!("Card {number}"), "number": number, "status": status,
        "priority": "high", "projectId": "p-web", "workspaceId": WS,
        "description": "## Why\n\nBecause.", "dueDate": "2026-11-01T22:59:00.000Z",
        "columnId": "ignored", "position": 4, "userId": null
    })
}

fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

async fn get(server: &MockServer, p: &str, body: Value) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ok_json(body))
        .mount(server)
        .await;
}

async fn card58(server: &MockServer, status: &str) {
    get(
        server,
        "/api/task/by-ticket-id/WEB-58",
        task("t58", 58, status),
    )
    .await;
}

async fn projects(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/project"))
        .and(query_param("workspaceId", WS))
        .respond_with(ok_json(json!([
            {"id": "p-web", "name": "Web", "slug": "WEB", "workspaceId": WS,
             "description": "- Column limit: Doing holds at most 2 cards while two people work."},
            {"id": "p-spr", "name": "Sprints", "slug": "SPR", "workspaceId": WS}
        ])))
        .mount(server)
        .await;
}

fn relation(kind: &str, source: (&str, &str, u32), target: (&str, &str, u32)) -> Value {
    let side = |(id, project, number): (&str, &str, u32)| {
        json!({"id": id, "title": format!("Card {number}"), "number": number,
               "status": "sprint", "projectId": project, "isCompleted": false})
    };
    json!({"id": "r", "relationType": kind, "sourceTask": side(source), "targetTask": side(target)})
}

async fn write_requests(server: &MockServer) -> Vec<(String, String, Value)> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| {
            let body = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
            (r.method.to_string(), r.url.path().to_owned(), body)
        })
        .collect()
}

async fn accept_writes(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/comment/t58"))
        .respond_with(ok_json(
            json!({"id": "c-new", "content": "echo", "taskId": "t58"}),
        ))
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/task/status/t58"))
        .respond_with(ok_json(task("t58", 58, "whatever")))
        .mount(server)
        .await;
}

#[tokio::test]
async fn card_brief_reads_five_endpoints_with_the_bearer_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/task/by-ticket-id/WEB-58"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ok_json(task("t58", 58, "doing")))
        .mount(&server)
        .await;
    get(
        &server,
        "/api/comment/t58",
        json!([
            {"id": "c1", "content": "DECISION 2026-10-20 — use rmcp\n\nWhy: binary", "createdAt": "2026-10-20T08:00:00.000Z",
             "user": {"name": "Ana", "image": null}, "taskId": "t58", "userId": "u"},
            {"id": "c2", "content": "START 2026-10-27 10:10 — agent-1 — go", "createdAt": "2026-10-27T09:10:53.352Z",
             "user": {"name": "Ana", "image": null}}
        ]),
    )
    .await;
    get(
        &server,
        "/api/task-relation/t58",
        json!([relation(
            "blocks",
            ("t58", "p-web", 58),
            ("t59", "p-web", 59)
        )]),
    )
    .await;
    get(
        &server,
        "/api/label/task/t58",
        json!([{"id": "l1", "name": "Dev", "color": "#373f47", "taskId": "t58", "workspaceId": WS}]),
    )
    .await;
    projects(&server).await;

    let out = ops::card_brief(&client(&server), "web-58", 5, false)
        .await
        .unwrap();
    assert_eq!(
        out,
        "WEB-58 Card 58 · doing · high · due 2026-11-01\n\
         id t58 · labels Dev\n\n\
         ## Relations\nblocks WEB-59 Card 59 (sprint)\n\n\
         ## Decisions (UTC)\n[2026-10-20 08:00 Ana] DECISION 2026-10-20 — use rmcp\n\nWhy: binary\n\n\
         ## Comments (newest 1 of 1, UTC)\n[2026-10-27 09:10 Ana] START 2026-10-27 10:10 — agent-1 — go\n\n\
         ## Description\n## Why\n\nBecause."
    );
}

/// A board run in sprints: Backlog, Sprint, Doing, Done.
async fn sprint_columns(server: &MockServer) {
    get(
        server,
        "/api/column/p-web",
        json!([
            {"id": "c0", "slug": "backlog", "name": "Backlog", "isFinal": false},
            {"id": "c1", "slug": "sprint", "name": "Sprint", "isFinal": false},
            {"id": "c2", "slug": "doing", "name": "Doing", "isFinal": false},
            {"id": "c3", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
}

/// The card's own project, which move_card reads its rules from.
async fn web_project(server: &MockServer) {
    get(
        server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB", "description": ""}),
    )
    .await;
}

async fn board_with_doing(server: &MockServer, doing: Vec<Value>) {
    sprint_columns(server).await;
    get(
        server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB",
               "description": "- Column limit: Doing holds at most 2 cards"}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .and(query_param("status", "doing"))
        .respond_with(ok_json(json!({
            "data": {"id": "p-web", "columns": [
                {"id": "doing", "slug": "doing", "name": "Doing", "isFinal": false, "tasks": doing}
            ], "archivedTasks": [], "plannedTasks": []},
            "pagination": {"total": 2, "page": 1, "pageSize": 100, "totalPages": 1}
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn start_card_refuses_at_the_cap_and_writes_nothing() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    board_with_doing(
        &server,
        vec![task("t1", 1, "doing"), task("t2", 2, "doing")],
    )
    .await;
    accept_writes(&server).await;

    let err = ops::start_card(&client(&server), now(), "WEB-58", "agent-1", "plan")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Doing is at its cap of 2 (WEB-1, WEB-2). Finish or pause one of them first."
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn start_card_under_the_cap_posts_start_then_moves() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    board_with_doing(&server, vec![task("t1", 1, "doing")]).await;
    accept_writes(&server).await;

    let out = ops::start_card(&client(&server), now(), "WEB-58", "agent-1", "build it")
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58 → doing");
    assert_eq!(
        write_requests(&server).await,
        vec![
            (
                "POST".into(),
                "/api/comment/t58".into(),
                json!({"content": "START 2026-10-27 10:05Z — agent-1 — build it"})
            ),
            (
                "PUT".into(),
                "/api/task/status/t58".into(),
                json!({"status": "doing"})
            ),
        ]
    );
}

#[tokio::test]
async fn start_card_takeover_is_allowed_even_over_the_cap() {
    // Cap 2, and Doing already holds three because someone moved one by hand.
    // Taking over WEB-58 adds nothing to Doing, so it must not be refused.
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    board_with_doing(
        &server,
        vec![
            task("t1", 1, "doing"),
            task("t2", 2, "doing"),
            task("t58", 58, "doing"),
        ],
    )
    .await;
    accept_writes(&server).await;

    let out = ops::start_card(&client(&server), now(), "WEB-58", "agent-1", "take over")
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58 → doing");
}

#[tokio::test]
async fn start_card_rejects_a_blank_plan_before_any_request() {
    let server = MockServer::start().await;
    let err = ops::start_card(&client(&server), now(), "WEB-58", "agent-1", "  ")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "plan is required and cannot be blank");
    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn workspace_labels(server: &MockServer) {
    get(
        server,
        &format!("/api/label/workspace/{WS}"),
        json!([
            {"id": "copy-on-t7", "name": "operator-decision", "taskId": "t7", "workspaceId": WS},
            {"id": "ws-od", "name": "operator-decision", "taskId": null, "workspaceId": WS},
            {"id": "ws-dev", "name": "Dev", "taskId": null, "workspaceId": WS}
        ]),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/api/label/ws-od/task"))
        .and(body_json(json!({"taskId": "t58"})))
        .respond_with(ok_json(
            json!({"id": "new-copy", "name": "operator-decision", "taskId": "t58"}),
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn pause_card_returns_to_sprint_and_flags_operator_decision() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    projects(&server).await;
    get(
        &server,
        "/api/task-relation/t58",
        json!([relation(
            "related",
            ("s5", "p-spr", 5),
            ("t58", "p-web", 58)
        )]),
    )
    .await;
    workspace_labels(&server).await;
    sprint_columns(&server).await;
    accept_writes(&server).await;

    let pause = Pause {
        ticket: "WEB-58",
        session: "agent-1",
        state: "Built, untested.",
        next_step: "run the suite",
        needs_decision: true,
        sprint_slug: Some("SPR"),
    };
    assert_eq!(
        ops::pause_card(&client(&server), now(), &pause)
            .await
            .unwrap(),
        "ok WEB-58 → sprint"
    );
    let writes = write_requests(&server).await;
    assert_eq!(
        writes,
        vec![
            (
                "POST".into(),
                "/api/comment/t58".into(),
                json!({"content": "PAUSE 2026-10-27 10:05Z — agent-1 — Built, untested. Next: run the suite"})
            ),
            (
                "PUT".into(),
                "/api/task/status/t58".into(),
                json!({"status": "sprint"})
            ),
            (
                "PUT".into(),
                "/api/label/ws-od/task".into(),
                json!({"taskId": "t58"})
            ),
        ]
    );
}

#[tokio::test]
async fn pause_card_without_a_sprint_link_goes_to_backlog() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    projects(&server).await;
    get(&server, "/api/task-relation/t58", json!([])).await;
    sprint_columns(&server).await;
    accept_writes(&server).await;

    let pause = Pause {
        ticket: "WEB-58",
        session: "S",
        state: "x.",
        next_step: "y",
        needs_decision: false,
        sprint_slug: Some("SPR"),
    };
    ops::pause_card(&client(&server), now(), &pause)
        .await
        .unwrap();
    let writes = write_requests(&server).await;
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[1].2, json!({"status": "backlog"}));
}

fn blocked_pause() -> Pause<'static> {
    Pause {
        ticket: "WEB-58",
        session: "S",
        state: "Waiting.",
        next_step: "ask Ana",
        needs_decision: true,
        sprint_slug: Some("SPR"),
    }
}

#[tokio::test]
async fn pause_card_writes_nothing_when_the_label_is_missing() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    projects(&server).await;
    get(&server, "/api/task-relation/t58", json!([])).await;
    sprint_columns(&server).await;
    get(
        &server,
        &format!("/api/label/workspace/{WS}"),
        json!([{"id": "ws-dev", "name": "Dev", "taskId": null}]),
    )
    .await;
    accept_writes(&server).await;

    let err = ops::pause_card(&client(&server), now(), &blocked_pause())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "no workspace label \"operator-decision\"; there are: Dev"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn pause_card_writes_nothing_when_there_is_nowhere_to_go() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    projects(&server).await;
    get(&server, "/api/task-relation/t58", json!([])).await;
    get(
        &server,
        "/api/column/p-web",
        json!([
            {"id": "c1", "slug": "doing", "name": "Doing", "isFinal": false},
            {"id": "c2", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
    accept_writes(&server).await;

    let err = ops::pause_card(&client(&server), now(), &blocked_pause())
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "project has no \"backlog\" column");
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn pause_card_on_a_board_without_backlog_goes_to_its_intake_column() {
    // A board shaped To-Do, Doing, Done, with no Backlog.
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    Mock::given(method("GET"))
        .and(path("/api/project"))
        .and(query_param("workspaceId", WS))
        .respond_with(ok_json(json!([
            {"id": "p-web", "name": "Mobile", "slug": "WEB", "workspaceId": WS,
             "description": "Mobile app.\n\n- New cards go to: To-Do"},
        ])))
        .mount(&server)
        .await;
    get(&server, "/api/task-relation/t58", json!([])).await;
    get(
        &server,
        "/api/column/p-web",
        json!([
            {"id": "c0", "slug": "to-do", "name": "To-Do", "isFinal": false},
            {"id": "c1", "slug": "doing", "name": "Doing", "isFinal": false},
            {"id": "c2", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
    accept_writes(&server).await;

    let pause = Pause {
        ticket: "WEB-58",
        session: "S",
        state: "x.",
        next_step: "y",
        needs_decision: false,
        sprint_slug: Some("SPR"),
    };
    assert_eq!(
        ops::pause_card(&client(&server), now(), &pause)
            .await
            .unwrap(),
        "ok WEB-58 → to-do"
    );
    let writes = write_requests(&server).await;
    assert_eq!(writes[1].2, json!({"status": "to-do"}));
}

#[tokio::test]
async fn pause_card_writes_nothing_when_its_project_is_not_listed() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    Mock::given(method("GET"))
        .and(path("/api/project"))
        .and(query_param("workspaceId", WS))
        .respond_with(ok_json(json!([
            {"id": "p-spr", "name": "Sprints", "slug": "SPR", "workspaceId": WS}
        ])))
        .mount(&server)
        .await;
    get(&server, "/api/task-relation/t58", json!([])).await;
    sprint_columns(&server).await;
    accept_writes(&server).await;

    let err = ops::pause_card(&client(&server), now(), &blocked_pause())
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "project p-web is missing from its workspace's project list"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn comment_refuses_text_that_belongs_to_a_rule_tool() {
    let server = MockServer::start().await;
    let err = ops::comment(&client(&server), "WEB-58", "DECISION 2026-10-27 — ship")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "a comment starting with \"DECISION\" goes through its own tool (decide)"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn close_card_requires_evidence_and_uses_the_final_column() {
    let server = MockServer::start().await;
    let err = ops::close_card(&client(&server), now(), "WEB-58", "S", " ")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "evidence is required and cannot be blank");

    card58(&server, "doing").await;
    get(
        &server,
        "/api/column/p-web",
        json!([
            {"id": "c1", "slug": "doing", "name": "Doing", "isFinal": false},
            {"id": "c2", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
    accept_writes(&server).await;
    let out = ops::close_card(
        &client(&server),
        now(),
        "WEB-58",
        "S",
        "commit abc, 40 tests green",
    )
    .await
    .unwrap();
    assert_eq!(out, "ok WEB-58 → done");
    let writes = write_requests(&server).await;
    assert_eq!(
        writes[0].2,
        json!({"content": "DONE 2026-10-27 10:05Z — S — commit abc, 40 tests green"})
    );
    assert_eq!(writes[1].2, json!({"status": "done"}));
}

#[tokio::test]
async fn decide_requires_provenance() {
    let server = MockServer::start().await;
    let mut d = Decision {
        choice: "Ship",
        reasoning: "ready",
        provenance: "",
        overruled: None,
        covers: Some(" "),
    };
    let err = ops::decide(&client(&server), now(), "WEB-58", &d)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "provenance is required and cannot be blank"
    );

    card58(&server, "doing").await;
    accept_writes(&server).await;
    d.provenance = "Ana: \"go\"";
    ops::decide(&client(&server), now(), "WEB-58", &d)
        .await
        .unwrap();
    assert_eq!(
        write_requests(&server).await[0].2,
        json!({"content": "DECISION 2026-10-27 — Ship\n\nWhy: ready\nProvenance: Ana: \"go\""})
    );
}

#[tokio::test]
async fn label_attaches_the_workspace_row_never_a_cards_copy() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    workspace_labels(&server).await;
    let out = ops::label(&client(&server), "WEB-58", "Operator-Decision")
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58");

    let err = ops::label(&client(&server), "WEB-58", "nope")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "no workspace label \"nope\"; there are: Dev, operator-decision"
    );
}

#[tokio::test]
async fn unlabel_detaches_the_cards_copy() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    get(
        &server,
        "/api/label/task/t58",
        json!([{"id": "copy58", "name": "operator-decision", "taskId": "t58"}]),
    )
    .await;
    Mock::given(method("DELETE"))
        .and(path("/api/label/copy58/task"))
        .respond_with(ok_json(json!({"id": "copy58"})))
        .expect(1)
        .mount(&server)
        .await;
    let out = ops::unlabel(&client(&server), "WEB-58", "operator-decision")
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58");
}

async fn workspaces(server: &MockServer, ids: &[&str]) {
    let list: Vec<Value> = ids
        .iter()
        .map(|id| json!({"id": id, "name": "w", "slug": "w"}))
        .collect();
    get(server, "/api/auth/organization/list", Value::Array(list)).await;
}

fn page(n: u32, columns: Value) -> ResponseTemplate {
    ok_json(json!({
        "data": {"id": "p-web", "columns": columns},
        "pagination": {"total": 3, "page": n, "pageSize": 100, "totalPages": 2}
    }))
}

#[tokio::test]
async fn board_digest_merges_pages_and_reports_the_cap() {
    let server = MockServer::start().await;
    workspaces(&server, &[WS]).await;
    projects(&server).await;
    sprint_columns(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .and(query_param("page", "1"))
        .respond_with(page(1, json!([
            {"slug": "doing", "name": "Doing", "isFinal": false, "tasks": [task("t1", 1, "doing")]},
            {"slug": "done", "name": "Done", "isFinal": true, "tasks": [task("t2", 2, "done")]}
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .and(query_param("page", "2"))
        .respond_with(page(2, json!([
            {"slug": "doing", "name": "Doing", "isFinal": false, "tasks": [task("t3", 3, "doing")]},
            {"slug": "done", "name": "Done", "isFinal": true, "tasks": []}
        ])))
        .mount(&server)
        .await;

    let out = ops::board_digest(&client(&server), "web", false)
        .await
        .unwrap();
    assert_eq!(
        out,
        "WEB Web · Doing 2/2\n\n## Doing (2)\n\
         WEB-1 high Card 1 · due 2026-11-01\nWEB-3 high Card 3 · due 2026-11-01\n\n## Done (1)"
    );
}

#[tokio::test]
async fn find_searches_every_workspace() {
    let server = MockServer::start().await;
    workspaces(&server, &["w1", "w2"]).await;
    for (ws, n) in [("w1", 58), ("w2", 7)] {
        Mock::given(method("GET"))
            .and(path("/api/search"))
            .and(query_param("workspaceId", ws))
            .and(query_param("type", "tasks"))
            .respond_with(ok_json(json!({"results": [
                {"id": format!("t{n}"), "type": "task", "title": "Lean", "projectSlug": "WEB", "taskNumber": n,
                 "status": "doing", "priority": "high", "description": "long text ignored"}
            ]})))
            .mount(&server)
            .await;
    }
    // One label read per workspace with hits, never one per card.
    for (ws, rows) in [
        (
            "w1",
            json!([
                {"id": "c1", "name": "security", "taskId": "t58"},
                {"id": "c2", "name": "Dev", "taskId": "t58"},
                {"id": "ws-dev", "name": "Dev", "taskId": null},
                {"id": "c3", "name": "incident", "taskId": "t99"}
            ]),
        ),
        (
            "w2",
            json!([{"id": "ws-dev", "name": "Dev", "taskId": null}]),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/api/label/workspace/{ws}")))
            .respond_with(ok_json(rows))
            .expect(1)
            .mount(&server)
            .await;
    }
    let out = ops::find(&client(&server), "lean", None).await.unwrap();
    assert_eq!(
        out,
        "WEB-58 doing high Lean · labels Dev, security\nWEB-7 doing high Lean"
    );
}

#[tokio::test]
async fn find_reads_no_labels_when_nothing_matches() {
    let server = MockServer::start().await;
    workspaces(&server, &["w1"]).await;
    get(&server, "/api/search", json!({"results": []})).await;
    let out = ops::find(&client(&server), "nothing", None).await.unwrap();
    assert_eq!(out, "no match");
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(
        !paths.iter().any(|p| p.starts_with("/api/label")),
        "{paths:?}"
    );
}

#[tokio::test]
async fn kaneo_errors_surface_with_their_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/task/by-ticket-id/WEB-999"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"message": "Task not found"})),
        )
        .mount(&server)
        .await;
    let err = ops::comment(&client(&server), "WEB-999", "hi")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "/api/task/by-ticket-id/WEB-999: HTTP 404: Task not found"
    );
}

#[tokio::test]
async fn a_redirect_is_an_error_and_is_never_followed() {
    let elsewhere = MockServer::start().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/comment/t58"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("location", format!("{}/steal", elsewhere.uri())),
        )
        .mount(&server)
        .await;
    card58(&server, "doing").await;
    let err = ops::comment(&client(&server), "WEB-58", "hi")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("HTTP 307"), "{err}");
    assert!(elsewhere.received_requests().await.unwrap().is_empty());
}

/// Two workspaces' worth of projects: WEB states its
/// intake column, MOB states one by name with a space, OPS lives elsewhere.
async fn intake_board(server: &MockServer) {
    workspaces(server, &[WS, "ws2"]).await;
    Mock::given(method("GET"))
        .and(path("/api/project"))
        .and(query_param("workspaceId", WS))
        .respond_with(ok_json(json!([
            {"id": "p-web", "name": "Web", "slug": "WEB", "icon": "Layout", "isPublic": false,
             "description": "Web app.\n\n- New cards go to: Backlog\n- Column limit: Doing holds at most 3 cards: one per person."},
            {"id": "p-mob", "name": "Mobile", "slug": "MOB", "description": "- New cards go to: To Do"},
            {"id": "p-spr", "name": "Sprints", "slug": "SPR"}
        ])))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/project"))
        .and(query_param("workspaceId", "ws2"))
        .respond_with(ok_json(
            json!([{"id": "p-ops", "name": "Ops", "slug": "OPS"}]),
        ))
        .mount(server)
        .await;
    sprint_columns(server).await;
    get(
        server,
        "/api/column/p-mob",
        json!([
            {"id": "m0", "slug": "to-do", "name": "To Do", "isFinal": false},
            {"id": "m1", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
}

async fn accept_create(server: &MockServer, project: &str) {
    Mock::given(method("POST"))
        .and(path(format!("/api/task/{project}")))
        .respond_with(ok_json(
            json!({"id": "t-new", "number": 66, "title": "x", "status": "backlog"}),
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn file_card_uses_the_stated_intake_column_and_answers_the_ticket() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    accept_create(&server, "p-web").await;
    let card = NewCard {
        project: "web",
        title: " Lean v0.2 ",
        description: Some("## Why\n\nBecause."),
        priority: Some("High"),
        column: None,
        start: Some("2026-10-27"),
        due: Some("2026-11-01T23:59:00+01:00"),
    };
    let out = ops::file_card(&client(&server), &card).await.unwrap();
    assert_eq!(out, "ok WEB-66");
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "POST".into(),
            "/api/task/p-web".into(),
            json!({
                "title": "Lean v0.2",
                "description": "## Why\n\nBecause.",
                "priority": "high",
                "status": "backlog",
                "startDate": "2026-10-27T12:00:00.000Z",
                "dueDate": "2026-11-01T22:59:00.000Z"
            })
        )]
    );
}

#[tokio::test]
async fn file_card_matches_a_column_named_with_spaces() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    accept_create(&server, "p-mob").await;
    let card = NewCard {
        project: "MOB",
        title: "Fix the login screen",
        description: None,
        priority: None,
        column: None,
        start: None,
        due: None,
    };
    ops::file_card(&client(&server), &card).await.unwrap();
    assert_eq!(
        write_requests(&server).await[0].2,
        json!({"title": "Fix the login screen", "description": "", "priority": "no-priority", "status": "to-do"})
    );
}

#[tokio::test]
async fn file_card_refuses_doing_the_final_column_and_a_missing_intake() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    accept_create(&server, "p-web").await;
    accept_create(&server, "p-spr").await;
    let card = |project, column| NewCard {
        project,
        title: "t",
        description: None,
        priority: None,
        column,
        start: None,
        due: None,
    };
    let api = client(&server);
    let doing = ops::file_card(&api, &card("WEB", Some("Doing"))).await;
    assert_eq!(
        doing.unwrap_err().to_string(),
        "cards enter Doing only through start_card"
    );
    let done = ops::file_card(&api, &card("WEB", Some("done"))).await;
    assert_eq!(
        done.unwrap_err().to_string(),
        "cards enter Done only through close_card"
    );
    let bad = ops::file_card(&api, &card("WEB", Some("review"))).await;
    assert_eq!(
        bad.unwrap_err().to_string(),
        "project has no column \"review\"; there are: backlog, sprint, doing, done"
    );
    assert!(write_requests(&server).await.is_empty());
}

/// A project as Kaneo creates it: its four default columns and no rules stated.
async fn default_board(server: &MockServer) {
    workspaces(server, &[WS]).await;
    Mock::given(method("GET"))
        .and(path("/api/project"))
        .and(query_param("workspaceId", WS))
        .respond_with(ok_json(json!([
            {"id": "p-app", "name": "App", "slug": "APP", "description": ""}
        ])))
        .mount(server)
        .await;
    get(
        server,
        "/api/project/p-app",
        json!({"id": "p-app", "name": "App", "slug": "APP", "description": ""}),
    )
    .await;
    get(
        server,
        "/api/column/p-app",
        json!([
            {"id": "k0", "slug": "to-do", "name": "To Do", "isFinal": false},
            {"id": "k1", "slug": "in-progress", "name": "In Progress", "isFinal": false},
            {"id": "k2", "slug": "in-review", "name": "In Review", "isFinal": false},
            {"id": "k3", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
}

#[tokio::test]
async fn file_card_on_a_default_board_goes_to_its_first_open_column() {
    let server = MockServer::start().await;
    default_board(&server).await;
    accept_create(&server, "p-app").await;
    let card = |column| NewCard {
        project: "APP",
        title: "t",
        description: None,
        priority: None,
        column,
        start: None,
        due: None,
    };
    let api = client(&server);
    ops::file_card(&api, &card(None)).await.unwrap();
    assert_eq!(write_requests(&server).await[0].2["status"], "to-do");
    assert_eq!(
        ops::file_card(&api, &card(Some("In Progress")))
            .await
            .unwrap_err()
            .to_string(),
        "cards enter In Progress only through start_card"
    );
}

#[tokio::test]
async fn start_and_pause_on_a_default_board_use_in_progress_and_to_do() {
    let server = MockServer::start().await;
    // The card is read twice: in To Do for the start, then In Progress for the pause.
    for status in ["to-do", "in-progress"] {
        let mut t = task("t9", 9, status);
        t["projectId"] = json!("p-app");
        Mock::given(method("GET"))
            .and(path("/api/task/by-ticket-id/APP-9"))
            .respond_with(ok_json(t))
            .up_to_n_times(1)
            .mount(&server)
            .await;
    }
    default_board(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-app"))
        .and(query_param("status", "in-progress"))
        .respond_with(ok_json(json!({
            "data": {"id": "p-app", "columns": [
                {"id": "k1", "slug": "in-progress", "name": "In Progress", "isFinal": false, "tasks": []}
            ], "archivedTasks": [], "plannedTasks": []},
            "pagination": {"total": 0, "page": 1, "pageSize": 100, "totalPages": 1}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/comment/t9"))
        .respond_with(ok_json(
            json!({"id": "c9", "content": "echo", "taskId": "t9"}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/task/status/t9"))
        .respond_with(ok_json(json!({"id": "t9"})))
        .mount(&server)
        .await;
    let api = client(&server);
    assert_eq!(
        ops::start_card(&api, now(), "APP-9", "agent-1", "build it")
            .await
            .unwrap(),
        "ok APP-9 → in-progress"
    );
    // No sprints configured: the pause reads no links and lands in To Do.
    let pause = Pause {
        ticket: "APP-9",
        session: "agent-1",
        state: "Half done.",
        next_step: "tests",
        needs_decision: false,
        sprint_slug: None,
    };
    assert_eq!(
        ops::pause_card(&api, now(), &pause).await.unwrap(),
        "ok APP-9 → to-do"
    );
    let paths: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert!(!paths.iter().any(|p| p.starts_with("/api/task-relation")));
    let writes = write_requests(&server).await;
    assert_eq!(writes[1].2, json!({"status": "in-progress"}));
    assert_eq!(writes[3].2, json!({"status": "to-do"}));
}

#[tokio::test]
async fn start_card_refuses_a_board_without_a_work_in_progress_column() {
    let server = MockServer::start().await;
    card58(&server, "backlog").await;
    get(
        &server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB", "description": ""}),
    )
    .await;
    get(
        &server,
        "/api/column/p-web",
        json!([
            {"id": "c0", "slug": "open", "name": "Open", "isFinal": false},
            {"id": "c1", "slug": "closed", "name": "Closed", "isFinal": true}
        ]),
    )
    .await;
    accept_writes(&server).await;
    let err = ops::start_card(&client(&server), now(), "WEB-58", "agent-1", "plan")
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .starts_with("project has no work-in-progress column")
    );
    assert!(write_requests(&server).await.is_empty());
}

async fn card(server: &MockServer, ticket: &str, id: &str, project: &str, ws: &str) {
    let number: u32 = ticket.rsplit('-').next().unwrap().parse().unwrap();
    get(
        server,
        &format!("/api/task/by-ticket-id/{ticket}"),
        json!({"id": id, "title": "t", "number": number, "status": "sprint",
               "projectId": project, "workspaceId": ws}),
    )
    .await;
}

async fn accept_relation_writes(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/task-relation"))
        .respond_with(ok_json(json!({"id": "rel-new", "relationType": "blocks"})))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/task-relation/rel-1"))
        .respond_with(ok_json(json!({"id": "rel-1"})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn link_posts_the_relation_from_ticket_to_other() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    card(&server, "SPR-5", "s5", "p-spr", WS).await;
    get(&server, "/api/task-relation/t58", json!([])).await;
    accept_relation_writes(&server).await;
    let out = ops::link(&client(&server), "WEB-58", "Subtask", "spr-5")
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58 subtask SPR-5");
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "POST".into(),
            "/api/task-relation".into(),
            json!({"sourceTaskId": "t58", "targetTaskId": "s5", "relationType": "subtask"})
        )]
    );
}

#[tokio::test]
async fn link_answers_an_existing_related_link_in_either_direction() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    card(&server, "SPR-5", "s5", "p-spr", WS).await;
    get(
        &server,
        "/api/task-relation/t58",
        json!([relation(
            "related",
            ("s5", "p-spr", 5),
            ("t58", "p-web", 58)
        )]),
    )
    .await;
    accept_relation_writes(&server).await;
    let api = client(&server);
    assert_eq!(
        ops::link(&api, "WEB-58", "related", "SPR-5").await.unwrap(),
        "ok WEB-58 related SPR-5"
    );
    // A `blocks` link has a direction: SPR-5 → WEB-58 related is not WEB-58 blocks SPR-5.
    ops::link(&api, "WEB-58", "blocks", "SPR-5").await.unwrap();
    assert_eq!(write_requests(&server).await.len(), 1);
}

#[tokio::test]
async fn link_refuses_self_links_other_workspaces_and_unknown_kinds() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    card(&server, "OPS-9", "h9", "p-ops", "ws2").await;
    accept_relation_writes(&server).await;
    let api = client(&server);
    let same = ops::link(&api, "WEB-58", "blocks", "web-58").await;
    assert_eq!(
        same.unwrap_err().to_string(),
        "a card cannot be linked to itself"
    );
    let far = ops::link(&api, "WEB-58", "blocks", "OPS-9").await;
    assert_eq!(
        far.unwrap_err().to_string(),
        "WEB-58 and OPS-9 are in different workspaces"
    );
    let kind = ops::link(&api, "WEB-58", "duplicates", "OPS-9").await;
    assert_eq!(
        kind.unwrap_err().to_string(),
        "relation kind \"duplicates\" is not one of blocks, related, subtask"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn unlink_deletes_the_matching_relation_by_id() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    card(&server, "WEB-59", "t59", "p-web", WS).await;
    let mut blocks = relation("blocks", ("t58", "p-web", 58), ("t59", "p-web", 59));
    blocks["id"] = json!("rel-1");
    get(&server, "/api/task-relation/t58", json!([blocks])).await;
    accept_relation_writes(&server).await;
    let api = client(&server);
    let missing = ops::unlink(&api, "WEB-58", "related", "WEB-59").await;
    assert_eq!(
        missing.unwrap_err().to_string(),
        "no related link from WEB-58 to WEB-59"
    );
    let out = ops::unlink(&api, "WEB-58", "blocks", "WEB-59")
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58 blocks WEB-59");
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "DELETE".into(),
            "/api/task-relation/rel-1".into(),
            Value::Null
        )]
    );
}

/// A card as by-ticket-id returns it, with every field the replacing PUT needs.
async fn full_card58(server: &MockServer) {
    let mut t = task("t58", 58, "sprint");
    t["position"] = json!(7);
    t["startDate"] = json!("2026-10-27T12:00:00.000Z");
    t["userId"] = json!("u-ana");
    t["assigneeName"] = json!("Ana Reyes");
    get(server, "/api/task/by-ticket-id/WEB-58", t).await;
    Mock::given(method("PUT"))
        .and(path_regex(
            r"^/api/task/((title|priority|assignee|due-date|description)/)?t58$",
        ))
        .respond_with(ok_json(task("t58", 58, "sprint")))
        .mount(server)
        .await;
    get(
        server,
        &format!("/api/workspace/{WS}/members"),
        json!([{"id": "u-ana", "name": "Ana Reyes", "email": "d@example.com", "role": "owner"},
               {"id": "u-bot", "name": "Bot", "email": "b@example.com", "role": "member"}]),
    )
    .await;
}

#[tokio::test]
async fn edit_card_uses_the_single_field_routes_so_a_move_is_never_undone() {
    let server = MockServer::start().await;
    full_card58(&server).await;
    let edit = CardEdit {
        title: Some(" New title "),
        priority: Some("urgent"),
        due: Some("2026-11-08"),
        ..CardEdit::default()
    };
    assert_eq!(
        ops::edit_card(&client(&server), "WEB-58", &edit)
            .await
            .unwrap(),
        "ok WEB-58"
    );
    // No status or position travels, so a concurrent close_card stands.
    assert_eq!(
        write_requests(&server).await,
        vec![
            (
                "PUT".into(),
                "/api/task/title/t58".into(),
                json!({"title": "New title"})
            ),
            (
                "PUT".into(),
                "/api/task/priority/t58".into(),
                json!({"priority": "urgent"})
            ),
            (
                "PUT".into(),
                "/api/task/due-date/t58".into(),
                json!({"dueDate": "2026-11-08T12:00:00.000Z"})
            ),
        ]
    );
}

#[tokio::test]
async fn edit_card_clears_the_due_date_and_assignee_through_their_routes() {
    let server = MockServer::start().await;
    full_card58(&server).await;
    let edit = CardEdit {
        due: Some("none"),
        assignee: Some("none"),
        description: Some(" Text "),
        ..CardEdit::default()
    };
    ops::edit_card(&client(&server), "WEB-58", &edit)
        .await
        .unwrap();
    assert_eq!(
        write_requests(&server).await,
        vec![
            (
                "PUT".into(),
                "/api/task/description/t58".into(),
                json!({"description": "Text"})
            ),
            ("PUT".into(), "/api/task/due-date/t58".into(), json!({})),
            (
                "PUT".into(),
                "/api/task/assignee/t58".into(),
                json!({"userId": null})
            ),
        ]
    );
}

#[tokio::test]
async fn edit_card_names_the_fields_applied_before_a_failure() {
    let server = MockServer::start().await;
    full_card58(&server).await;
    Mock::given(method("PUT"))
        .and(path("/api/task/priority/t58"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"message": "boom"})))
        .with_priority(1)
        .mount(&server)
        .await;
    let edit = CardEdit {
        title: Some("t"),
        priority: Some("low"),
        ..CardEdit::default()
    };
    let err = ops::edit_card(&client(&server), "WEB-58", &edit)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "/api/task/priority/t58: HTTP 500: boom (already applied: title)"
    );
}

#[tokio::test]
async fn edit_card_with_a_start_date_sends_the_whole_card() {
    // There is no start-date route, so this edit alone uses the replacing PUT,
    // with the start date and assignee it does not change sent back as they are.
    let server = MockServer::start().await;
    full_card58(&server).await;
    let edit = CardEdit {
        start: Some("2026-10-28"),
        priority: Some("low"),
        ..CardEdit::default()
    };
    ops::edit_card(&client(&server), "WEB-58", &edit)
        .await
        .unwrap();
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "PUT".into(),
            "/api/task/t58".into(),
            json!({
                "title": "Card 58", "priority": "low", "status": "sprint",
                "projectId": "p-web", "position": 7,
                "startDate": "2026-10-28T12:00:00.000Z",
                "dueDate": "2026-11-01T22:59:00.000Z",
                "userId": "u-ana"
            })
        )]
    );
}

#[tokio::test]
async fn edit_card_clears_with_none_and_assigns_by_name() {
    let server = MockServer::start().await;
    full_card58(&server).await;
    let api = client(&server);
    let clear = CardEdit {
        start: Some("none"),
        assignee: Some("None"),
        description: Some("New text"),
        ..CardEdit::default()
    };
    ops::edit_card(&api, "WEB-58", &clear).await.unwrap();
    let assign = CardEdit {
        assignee: Some("bot"),
        ..CardEdit::default()
    };
    ops::edit_card(&api, "WEB-58", &assign).await.unwrap();
    let writes = write_requests(&server).await;
    assert_eq!(
        writes[0].2,
        json!({
            "title": "Card 58", "priority": "high", "status": "sprint",
            "projectId": "p-web", "position": 7, "description": "New text",
            "dueDate": "2026-11-01T22:59:00.000Z"
        })
    );
    assert_eq!(writes[1].2["userId"], "u-bot");
    let unknown = CardEdit {
        assignee: Some("Nobody"),
        ..CardEdit::default()
    };
    let err = ops::edit_card(&api, "WEB-58", &unknown).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "no member \"Nobody\"; there are: Ana Reyes, Bot"
    );
    assert!(!err.to_string().contains('@'), "an email leaked: {err}");
}

#[tokio::test]
async fn edit_card_rejects_an_empty_edit_and_bad_values_before_writing() {
    let server = MockServer::start().await;
    full_card58(&server).await;
    let api = client(&server);
    let empty = ops::edit_card(&api, "WEB-58", &CardEdit::default()).await;
    assert_eq!(empty.unwrap_err().to_string(), "nothing to change");
    let bad_date = CardEdit {
        due: Some("next friday"),
        ..CardEdit::default()
    };
    assert_eq!(
        ops::edit_card(&api, "WEB-58", &bad_date)
            .await
            .unwrap_err()
            .to_string(),
        "due \"next friday\" is not a date (2026-11-01) or an RFC 3339 time"
    );
    let bad_priority = CardEdit {
        priority: Some("p1"),
        ..CardEdit::default()
    };
    assert!(ops::edit_card(&api, "WEB-58", &bad_priority).await.is_err());
    let after_due = CardEdit {
        start: Some("2026-11-05"),
        ..CardEdit::default()
    };
    assert_eq!(
        ops::edit_card(&api, "WEB-58", &after_due)
            .await
            .unwrap_err()
            .to_string(),
        "start 2026-11-05T12:00:00.000Z is after due 2026-11-01T22:59:00.000Z"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn move_card_sets_the_status_within_the_project() {
    let server = MockServer::start().await;
    card58(&server, "backlog").await;
    sprint_columns(&server).await;
    web_project(&server).await;
    accept_writes(&server).await;
    let out = ops::move_card(&client(&server), "WEB-58", "Sprint", None)
        .await
        .unwrap();
    assert_eq!(out, "ok WEB-58 → sprint");
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "PUT".into(),
            "/api/task/status/t58".into(),
            json!({"status": "sprint"})
        )]
    );
}

#[tokio::test]
async fn move_card_leaves_doing_and_done_to_the_lifecycle_tools() {
    let server = MockServer::start().await;
    sprint_columns(&server).await;
    web_project(&server).await;
    accept_writes(&server).await;
    card58(&server, "doing").await;
    let api = client(&server);
    let out_of_doing = ops::move_card(&api, "WEB-58", "sprint", None).await;
    assert_eq!(
        out_of_doing.unwrap_err().to_string(),
        "cards leave Doing only through pause_card and close_card"
    );
    let server = MockServer::start().await;
    sprint_columns(&server).await;
    web_project(&server).await;
    accept_writes(&server).await;
    card58(&server, "sprint").await;
    let api = client(&server);
    let into_done = ops::move_card(&api, "WEB-58", "done", None).await;
    assert_eq!(
        into_done.unwrap_err().to_string(),
        "cards enter Done only through close_card"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn move_card_to_another_project_answers_the_new_ticket() {
    let server = MockServer::start().await;
    card58(&server, "backlog").await;
    intake_board(&server).await;
    web_project(&server).await;
    Mock::given(method("PUT"))
        .and(path("/api/task/move/t58"))
        .respond_with(ok_json(json!({
            "task": {"id": "t58", "number": 12, "projectId": "p-mob"},
            "sourceProjectId": "p-web", "destinationProjectId": "p-mob"
        })))
        .mount(&server)
        .await;
    let api = client(&server);
    let out = ops::move_card(&api, "WEB-58", "to do", Some("mob"))
        .await
        .unwrap();
    assert_eq!(out, "ok MOB-12 → to-do");
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "PUT".into(),
            "/api/task/move/t58".into(),
            json!({"destinationProjectId": "p-mob", "destinationStatus": "to-do"})
        )]
    );
    let far = ops::move_card(&api, "WEB-58", "backlog", Some("OPS")).await;
    assert_eq!(
        far.unwrap_err().to_string(),
        "WEB-58 and OPS are in different workspaces"
    );
}

async fn comments58(server: &MockServer) {
    get(
        server,
        "/api/comment/t58",
        json!([
            {"id": "c-plain", "content": "Progress: tests", "createdAt": "2026-10-27T09:00:00.000Z"},
            {"id": "c-dec", "content": "DECISION 2026-10-27 — ship", "createdAt": "2026-10-27T09:05:00.000Z"}
        ]),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/api/comment/c-dec"))
        .respond_with(ok_json(json!({"id": "c-dec"})))
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/comment/c-plain"))
        .respond_with(ok_json(json!({"id": "c-plain"})))
        .mount(server)
        .await;
}

#[tokio::test]
async fn edit_comment_keeps_the_kind_and_the_card() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    comments58(&server).await;
    let api = client(&server);
    let out = ops::edit_comment(&api, "WEB-58", "c-dec", "DECISION 2026-10-27 — ship it")
        .await
        .unwrap();
    assert_eq!(out, "ok c-dec");
    let promote = ops::edit_comment(&api, "WEB-58", "c-plain", "DECISION 2026-10-27 — mine").await;
    assert_eq!(
        promote.unwrap_err().to_string(),
        "an edit keeps the comment's kind: plain cannot become DECISION"
    );
    let elsewhere = ops::edit_comment(&api, "WEB-58", "c-other", "text").await;
    assert_eq!(
        elsewhere.unwrap_err().to_string(),
        "WEB-58 has no comment c-other"
    );
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "PUT".into(),
            "/api/comment/c-dec".into(),
            json!({"content": "DECISION 2026-10-27 — ship it"})
        )]
    );
}

#[tokio::test]
async fn delete_comment_deletes_only_a_comment_of_that_card() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    comments58(&server).await;
    let api = client(&server);
    assert!(
        ops::delete_comment(&api, "WEB-58", "c-other")
            .await
            .is_err()
    );
    assert_eq!(
        ops::delete_comment(&api, "WEB-58", "c-plain")
            .await
            .unwrap(),
        "ok c-plain"
    );
    assert_eq!(
        write_requests(&server).await,
        vec![("DELETE".into(), "/api/comment/c-plain".into(), Value::Null)]
    );
}

#[tokio::test]
async fn list_cards_pages_by_creation_and_restores_the_order() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    let mut older = task("t61", 61, "done");
    older["createdAt"] = json!("2026-10-20T08:00:00.000Z");
    let mut newer = task("t63", 63, "sprint");
    newer["createdAt"] = json!("2026-10-27T08:00:00.000Z");
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .and(query_param("sortBy", "createdAt"))
        .and(query_param("sortOrder", "desc"))
        .and(query_param("page", "2"))
        .and(query_param("limit", "100"))
        .respond_with(ok_json(json!({
            "data": {"id": "p-web", "columns": [
                {"id": "c1", "slug": "sprint", "name": "Sprint", "isFinal": false, "tasks": [newer]},
                {"id": "c3", "slug": "done", "name": "Done", "isFinal": true, "tasks": [older]}
            ]},
            "pagination": {"total": 160, "page": 2, "pageSize": 100, "totalPages": 2}
        })))
        .mount(&server)
        .await;
    let q = ListQuery {
        project: "WEB",
        column: None,
        newest_first: true,
        page: 2,
        limit: 500,
    };
    assert_eq!(
        ops::list_cards(&client(&server), &q).await.unwrap(),
        "WEB page 2/2 · 160 cards\n\
         WEB-63 sprint high Card 63 · created 2026-10-27 · due 2026-11-01\n\
         WEB-61 done high Card 61 · created 2026-10-20 · due 2026-11-01"
    );
}

#[tokio::test]
async fn list_cards_filters_to_a_column_by_name() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .and(query_param("status", "done"))
        .and(query_param("sortOrder", "asc"))
        .respond_with(page(1, json!([])))
        .mount(&server)
        .await;
    let q = ListQuery {
        project: "WEB",
        column: Some("Done"),
        newest_first: false,
        page: 1,
        limit: 50,
    };
    assert_eq!(
        ops::list_cards(&client(&server), &q).await.unwrap(),
        "WEB page 1/2 · 3 cards\nno cards"
    );
}

#[tokio::test]
async fn card_activity_lists_status_changes_oldest_first_without_comments() {
    let server = MockServer::start().await;
    card58(&server, "done").await;
    get(
        &server,
        "/api/activity/t58",
        json!([
            {"id": "a4", "type": "status_changed", "createdAt": "2026-10-27T12:00:06.661Z",
             "eventData": {"newStatus": "done", "oldStatus": "sprint"}, "content": null},
            {"id": "a3", "type": "comment", "createdAt": "2026-10-27T12:00:05.140Z",
             "eventData": null, "content": "DECISION …"},
            {"id": "a2", "type": "status_changed", "createdAt": "2026-10-27T09:10:50.878Z",
             "eventData": {"newStatus": "doing", "oldStatus": "sprint"}, "content": null},
            {"id": "a1", "type": "created", "createdAt": "2026-10-27T08:51:25.746Z", "eventData": {}},
            {"id": "a0", "type": "due_date_changed", "createdAt": "2026-10-27T08:52:00.000Z", "eventData": null}
        ]),
    )
    .await;
    assert_eq!(
        ops::card_activity(&client(&server), "WEB-58")
            .await
            .unwrap(),
        "WEB-58 activity (UTC)\n\
         2026-10-27 08:51 created\n\
         2026-10-27 08:52 due date changed\n\
         2026-10-27 09:10 status sprint → doing\n\
         2026-10-27 12:00 status sprint → done"
    );
}

#[tokio::test]
async fn project_info_reads_the_board_rules() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    assert_eq!(
        ops::project_info(&client(&server), "WEB").await.unwrap(),
        "WEB Web\nid p-web\ncolumns backlog, sprint, doing, done (final)\n\
         work in progress in doing · cap 3 · new cards go to Backlog\n\n\
         ## Description\n\
         Web app.\n\n- New cards go to: Backlog\n- Column limit: Doing holds at most 3 cards: one per person."
    );
}

#[tokio::test]
async fn edit_project_description_replaces_it_and_keeps_the_other_fields() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    get(
        &server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB", "icon": "Terminal",
               "isPublic": false, "description": "old"}),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/api/project/p-web"))
        .respond_with(ok_json(json!({"id": "p-web"})))
        .mount(&server)
        .await;
    let api = client(&server);
    let blank = ops::edit_project_description(&api, "WEB", " ").await;
    assert_eq!(
        blank.unwrap_err().to_string(),
        "description is required and cannot be blank"
    );
    let out = ops::edit_project_description(&api, "WEB", "Web app.\n- New cards go to: Sprint\n")
        .await
        .unwrap();
    assert_eq!(
        out,
        "ok p-web · work in progress in doing · no cap stated · new cards go to Sprint"
    );
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "PUT".into(),
            "/api/project/p-web".into(),
            json!({"name": "Web", "icon": "Terminal", "slug": "WEB",
                   "description": "Web app.\n- New cards go to: Sprint", "isPublic": false})
        )]
    );
}

#[tokio::test]
async fn projects_lists_each_workspace_with_its_label_names() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    workspace_labels(&server).await;
    get(&server, "/api/label/workspace/ws2", json!([])).await;
    assert_eq!(
        ops::projects(&client(&server)).await.unwrap(),
        "## w\nWEB Web\nMOB Mobile\nSPR Sprints\nlabels Dev, operator-decision\n\n## w\nOPS Ops"
    );
}

#[tokio::test]
async fn edit_card_refuses_an_ambiguous_member_name() {
    let server = MockServer::start().await;
    get(
        &server,
        &format!("/api/workspace/{WS}/members"),
        json!([{"id": "u1", "name": "Alex"}, {"id": "u2", "name": "alex"}]),
    )
    .await;
    full_card58(&server).await;
    let edit = CardEdit {
        assignee: Some("ALEX"),
        ..CardEdit::default()
    };
    let api = client(&server);
    assert_eq!(
        ops::edit_card(&api, "WEB-58", &edit)
            .await
            .unwrap_err()
            .to_string(),
        "2 members are named \"ALEX\"; pass the member id"
    );
    let by_id = CardEdit {
        assignee: Some("u2"),
        ..CardEdit::default()
    };
    ops::edit_card(&api, "WEB-58", &by_id).await.unwrap();
    assert_eq!(
        write_requests(&server).await,
        vec![(
            "PUT".into(),
            "/api/task/assignee/t58".into(),
            json!({"userId": "u2"})
        )]
    );
}

#[tokio::test]
async fn link_refuses_the_reverse_of_an_existing_directed_link() {
    let server = MockServer::start().await;
    card58(&server, "sprint").await;
    card(&server, "WEB-59", "t59", "p-web", WS).await;
    get(
        &server,
        "/api/task-relation/t58",
        json!([relation(
            "blocks",
            ("t59", "p-web", 59),
            ("t58", "p-web", 58)
        )]),
    )
    .await;
    accept_relation_writes(&server).await;
    let err = ops::link(&client(&server), "WEB-58", "blocks", "WEB-59")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "WEB-59 already has a blocks link to WEB-58; unlink it first"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn delete_comment_keeps_the_record_comments() {
    let server = MockServer::start().await;
    card58(&server, "doing").await;
    comments58(&server).await;
    let err = ops::delete_comment(&client(&server), "WEB-58", "c-dec")
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "a DECISION comment is a record and is not deleted; edit it instead"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn list_cards_prints_the_planned_and_archived_cards_it_counts() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    let mut archived = task("t40", 40, "archived");
    archived["createdAt"] = json!("2026-10-25T08:00:00.000Z");
    let mut live = task("t63", 63, "sprint");
    live["createdAt"] = json!("2026-10-27T08:00:00.000Z");
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .respond_with(ok_json(json!({
            "data": {"id": "p-web", "columns": [
                {"id": "c1", "slug": "sprint", "name": "Sprint", "isFinal": false, "tasks": [live]}
            ], "plannedTasks": [], "archivedTasks": [archived]},
            "pagination": {"total": 2, "page": 1, "pageSize": 50, "totalPages": 1}
        })))
        .mount(&server)
        .await;
    let q = ListQuery {
        project: "WEB",
        column: None,
        newest_first: false,
        page: 1,
        limit: 50,
    };
    let out = ops::list_cards(&client(&server), &q).await.unwrap();
    assert_eq!(
        out.lines().skip(1).map(|l| &l[..6]).collect::<Vec<_>>(),
        ["WEB-40", "WEB-63"]
    );
}

#[tokio::test]
async fn move_card_says_when_kaneo_unassigned_the_card() {
    let server = MockServer::start().await;
    let mut t = task("t58", 58, "backlog");
    t["userId"] = json!("u-ana");
    get(&server, "/api/task/by-ticket-id/WEB-58", t).await;
    intake_board(&server).await;
    web_project(&server).await;
    Mock::given(method("PUT"))
        .and(path("/api/task/move/t58"))
        .respond_with(ok_json(json!({
            "task": {"id": "t58", "number": 12, "projectId": "p-mob", "userId": null}
        })))
        .mount(&server)
        .await;
    assert_eq!(
        ops::move_card(&client(&server), "WEB-58", "to-do", Some("MOB"))
            .await
            .unwrap(),
        "ok MOB-12 → to-do · unassigned"
    );
}

#[tokio::test]
async fn edit_project_description_refuses_to_drop_a_rule_or_name_a_bad_intake() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    get(
        &server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB", "icon": "Layout", "isPublic": false,
               "description": "- New cards go to: Backlog\n- Column limit: Doing holds at most 3 cards"}),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/api/project/p-web"))
        .respond_with(ok_json(json!({"id": "p-web"})))
        .mount(&server)
        .await;
    let api = client(&server);
    let no_cap = ops::edit_project_description(&api, "WEB", "- New cards go to: Backlog").await;
    assert_eq!(
        no_cap.unwrap_err().to_string(),
        "the new description drops the \"Column limit:\" line the old one stated"
    );
    let into_doing = ops::edit_project_description(
        &api,
        "WEB",
        "- New cards go to: Doing\n- Column limit: Doing holds at most 3 cards",
    )
    .await;
    assert_eq!(
        into_doing.unwrap_err().to_string(),
        "cards enter Doing only through start_card"
    );
    let unknown = ops::edit_project_description(
        &api,
        "WEB",
        "- New cards go to: Inbox\n- Column limit: Doing holds at most 3 cards",
    )
    .await;
    assert!(
        unknown
            .unwrap_err()
            .to_string()
            .starts_with("project has no column \"Inbox\"")
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn edit_project_description_refuses_to_guess_a_missing_visibility() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    get(
        &server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB", "icon": "Layout", "isPublic": null}),
    )
    .await;
    let err = ops::edit_project_description(&client(&server), "WEB", "Web app.")
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "project WEB has no isPublic to send back");
}

#[tokio::test]
async fn a_stated_work_in_progress_column_wins_over_a_doing_column() {
    let server = MockServer::start().await;
    card58(&server, "backlog").await;
    sprint_columns(&server).await;
    get(
        &server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB",
               "description": "- Work in progress: Sprint\n- Column limit: Sprint holds at most 1 card"}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/task/tasks/p-web"))
        .and(query_param("status", "sprint"))
        .respond_with(ok_json(json!({
            "data": {"id": "p-web", "columns": [
                {"id": "c1", "slug": "sprint", "name": "Sprint", "isFinal": false,
                 "tasks": [task("t7", 7, "sprint")]}
            ], "archivedTasks": [], "plannedTasks": []},
            "pagination": {"total": 1, "page": 1, "pageSize": 100, "totalPages": 1}
        })))
        .mount(&server)
        .await;
    accept_writes(&server).await;
    let api = client(&server);
    assert_eq!(
        ops::start_card(&api, now(), "WEB-58", "agent-1", "plan")
            .await
            .unwrap_err()
            .to_string(),
        "Sprint is at its cap of 1 (WEB-7). Finish or pause one of them first."
    );
    assert_eq!(
        ops::move_card(&api, "WEB-58", "sprint", None)
            .await
            .unwrap_err()
            .to_string(),
        "cards enter Sprint only through start_card"
    );
    assert!(write_requests(&server).await.is_empty());
    // Doing is a plain column on this board.
    assert_eq!(
        ops::move_card(&api, "WEB-58", "doing", None).await.unwrap(),
        "ok WEB-58 → doing"
    );
}

#[tokio::test]
async fn move_card_across_projects_keeps_both_boards_lifecycle_columns() {
    let server = MockServer::start().await;
    card58(&server, "backlog").await;
    intake_board(&server).await;
    web_project(&server).await;
    get(
        &server,
        "/api/column/p-spr",
        json!([
            {"id": "s0", "slug": "to-do", "name": "To Do", "isFinal": false},
            {"id": "s1", "slug": "in-progress", "name": "In Progress", "isFinal": false},
            {"id": "s2", "slug": "done", "name": "Done", "isFinal": true}
        ]),
    )
    .await;
    let api = client(&server);
    let into_final = ops::move_card(&api, "WEB-58", "done", Some("MOB")).await;
    assert_eq!(
        into_final.unwrap_err().to_string(),
        "cards enter Done only through close_card"
    );
    let into_active = ops::move_card(&api, "WEB-58", "in progress", Some("SPR")).await;
    assert_eq!(
        into_active.unwrap_err().to_string(),
        "cards enter In Progress only through start_card"
    );
    assert!(write_requests(&server).await.is_empty());

    let server = MockServer::start().await;
    card58(&server, "doing").await;
    intake_board(&server).await;
    web_project(&server).await;
    let out_of_active = ops::move_card(&client(&server), "WEB-58", "to do", Some("MOB")).await;
    assert_eq!(
        out_of_active.unwrap_err().to_string(),
        "cards leave Doing only through pause_card and close_card"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn pause_card_refuses_a_card_that_is_not_in_progress() {
    let server = MockServer::start().await;
    card58(&server, "done").await;
    projects(&server).await;
    sprint_columns(&server).await;
    accept_writes(&server).await;
    let pause = Pause {
        ticket: "WEB-58",
        session: "S",
        state: "x.",
        next_step: "y",
        needs_decision: false,
        sprint_slug: Some("SPR"),
    };
    assert_eq!(
        ops::pause_card(&client(&server), now(), &pause)
            .await
            .unwrap_err()
            .to_string(),
        "WEB-58 is in done, not doing: only a card in progress is paused"
    );
    assert!(write_requests(&server).await.is_empty());
}

#[tokio::test]
async fn edit_project_description_cannot_loosen_the_rules() {
    let server = MockServer::start().await;
    intake_board(&server).await;
    get(
        &server,
        "/api/project/p-web",
        json!({"id": "p-web", "name": "Web", "slug": "WEB", "icon": "Layout", "isPublic": false,
               "description": "- Work in progress: Doing\n- Column limit: Doing holds at most 3 cards"}),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path("/api/project/p-web"))
        .respond_with(ok_json(json!({"id": "p-web"})))
        .mount(&server)
        .await;
    let api = client(&server);
    let raised = ops::edit_project_description(
        &api,
        "WEB",
        "- Work in progress: Doing\n- Column limit: Doing holds at most 99 cards",
    )
    .await;
    assert_eq!(
        raised.unwrap_err().to_string(),
        "the cap is changed in Kaneo itself, not through this server: an agent cannot loosen the rules it works under"
    );
    let moved = ops::edit_project_description(
        &api,
        "WEB",
        "- Work in progress: Backlog\n- Column limit: Backlog holds at most 3 cards",
    )
    .await;
    assert!(
        moved
            .unwrap_err()
            .to_string()
            .starts_with("the work-in-progress column is changed in Kaneo itself")
    );
    assert!(write_requests(&server).await.is_empty());
    let lowered = ops::edit_project_description(
        &api,
        "WEB",
        "- Work in progress: Doing\n- Column limit: Doing holds at most 1 card",
    )
    .await
    .unwrap();
    assert_eq!(
        lowered,
        "ok p-web · work in progress in doing · cap 1 · no intake column stated"
    );
}
