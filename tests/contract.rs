//! Read-only contract tests against a real Kaneo: they prove the response
//! shapes the mock tests assume still decode. Ignored by default; run with
//!
//!   KANEO_API_URL=... KANEO_API_KEY=... KANEO_CONTRACT_TICKET=WEB-58 \
//!     cargo test --test contract -- --ignored
//!
//! They never write: a write contract needs a scratch card on a scratch
//! instance, such as one running locally, which these tests do not assume exists.

use kaneo_workflow_mcp::api::KaneoClient;
use kaneo_workflow_mcp::ops;
use kaneo_workflow_mcp::workflow::parse_ticket;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set for contract tests"))
}

fn live() -> (KaneoClient, String) {
    let client = KaneoClient::new(&env("KANEO_API_URL"), env("KANEO_API_KEY")).unwrap();
    (client, env("KANEO_CONTRACT_TICKET"))
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn card_brief_decodes_a_real_card() {
    let (api, ticket) = live();
    let out = ops::card_brief(&api, &ticket, 5, false).await.unwrap();
    assert!(
        out.starts_with(&format!("{} ", parse_ticket(&ticket).unwrap())),
        "{out}"
    );
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn board_digest_decodes_a_real_board() {
    let (api, ticket) = live();
    let slug = parse_ticket(&ticket).unwrap().slug;
    let out = ops::board_digest(&api, &slug, false).await.unwrap();
    assert!(out.starts_with(&format!("{slug} ")), "{out}");
    assert!(out.contains("## "), "{out}");
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn find_decodes_real_search_hits() {
    let (api, ticket) = live();
    let t = parse_ticket(&ticket).unwrap();
    // Kaneo ranks the full ticket first; the bare number only matches text.
    let out = ops::find(&api, &t.to_string(), Some(&t.slug))
        .await
        .unwrap();
    assert!(out.starts_with(&t.to_string()), "{out}");
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn workspace_labels_and_columns_decode() {
    let (api, ticket) = live();
    let task = api.task_by_ticket(&ticket).await.unwrap();
    let ws = task.workspace_id.expect("by-ticket-id returns workspaceId");
    let labels = api.workspace_labels(&ws).await.unwrap();
    assert!(
        labels.iter().any(|l| l.task_id.is_none()),
        "no workspace label rows"
    );
    let columns = api.columns(&task.project_id).await.unwrap();
    assert!(columns.iter().any(|c| c.is_final), "no final column");
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn list_cards_decodes_a_real_page_in_creation_order() {
    let (api, ticket) = live();
    let slug = parse_ticket(&ticket).unwrap().slug;
    let q = ops::ListQuery {
        project: &slug,
        column: None,
        newest_first: true,
        page: 1,
        limit: 5,
    };
    let out = ops::list_cards(&api, &q).await.unwrap();
    assert!(out.starts_with(&format!("{slug} page 1/")), "{out}");
    let days: Vec<&str> = out
        .lines()
        .filter_map(|l| l.split(" · created ").nth(1))
        .map(|rest| &rest[..10])
        .collect();
    assert!(!days.is_empty(), "{out}");
    assert!(
        days.windows(2).all(|w| w[0] >= w[1]),
        "not newest first: {out}"
    );
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn card_activity_decodes_a_real_history() {
    let (api, ticket) = live();
    let out = ops::card_activity(&api, &ticket).await.unwrap();
    assert!(out.contains(" created"), "{out}");
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn project_info_and_projects_decode() {
    let (api, ticket) = live();
    let slug = parse_ticket(&ticket).unwrap().slug;
    let info = ops::project_info(&api, &slug).await.unwrap();
    assert!(info.contains("(final)"), "{info}");
    let all = ops::projects(&api).await.unwrap();
    assert!(
        all.lines().any(|l| l.starts_with(&format!("{slug} "))),
        "{all}"
    );
}

#[tokio::test]
#[ignore = "needs a live Kaneo"]
async fn the_fields_an_edit_sends_back_decode() {
    // edit_card replaces the whole card, so every field it sends back must
    // arrive from by-ticket-id; members must decode for assignee names.
    let (api, ticket) = live();
    let task = api.task_by_ticket(&ticket).await.unwrap();
    assert!(task.position.is_some(), "by-ticket-id sends no position");
    let members = api
        .members(task.workspace_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(!members.is_empty());
    let relations = api.relations(&task.id).await.unwrap();
    assert!(relations.iter().all(|r| !r.id.is_empty()));
}
