//! Prints `card_brief` for one ticket, read-only, to compare its size with the
//! raw objects @kaneo/mcp returns for the same card:
//!
//!   KANEO_API_URL=... KANEO_API_KEY=... cargo run --example brief -- WEB-58

use kaneo_workflow_mcp::{api::KaneoClient, ops};

#[tokio::main]
async fn main() {
    let ticket = std::env::args().nth(1).expect("usage: brief <TICKET>");
    let url = std::env::var("KANEO_API_URL").expect("KANEO_API_URL");
    let key = std::env::var("KANEO_API_KEY").expect("KANEO_API_KEY");
    let api = KaneoClient::new(&url, key).expect("client");
    match ops::card_brief(&api, &ticket, 5, false).await {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("error: {e}"),
    }
}
