//! stdio entry point. The instance comes from the environment, so one binary
//! serves any number of Kaneo instances with the env each MCP entry sets.

use std::process::ExitCode;

use kaneo_workflow_mcp::{api::KaneoClient, server::KaneoServer};
use rmcp::{ServiceExt, transport::stdio};

fn env(name: &str) -> Result<String, String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(format!("{name} is not set")),
    }
}

const HELP: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    env!("CARGO_PKG_VERSION"),
    "\n",
    env!("CARGO_PKG_DESCRIPTION"),
    "\n\n",
    "Speaks MCP over stdio; an MCP client starts it. Configuration is the environment:\n",
    "  KANEO_API_URL         the Kaneo instance, like https://kaneo.example.com (required)\n",
    "  KANEO_API_KEY         an API key of that instance (required)\n",
    "  KANEO_SPRINT_PROJECT  slug of the project holding sprint cards (optional)\n",
    "\n",
    "Options:\n",
    "  -V, --version  print the version\n",
    "  -h, --help     print this help\n",
);

#[tokio::main]
async fn main() -> ExitCode {
    // An installer probes the binary with --version before trusting it.
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!(
                "{}: unknown argument {other:?}; see --help",
                env!("CARGO_PKG_NAME")
            );
            return ExitCode::from(2);
        }
        None => {}
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // stdout carries the protocol; diagnostics go to stderr only.
            eprintln!("{}: {e}", env!("CARGO_PKG_NAME"));
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let url = env("KANEO_API_URL")?;
    let key = env("KANEO_API_KEY")?;
    let sprint = std::env::var("KANEO_SPRINT_PROJECT")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let api = KaneoClient::new(&url, key).map_err(|e| e.to_string())?;
    let service = KaneoServer::new(api, sprint)
        .serve(stdio())
        .await
        .map_err(|e| format!("MCP handshake failed: {e}"))?;
    service.waiting().await.map_err(|e| e.to_string())?;
    Ok(())
}
