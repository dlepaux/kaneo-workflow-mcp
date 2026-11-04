//! The MCP surface: argument structs and one thin method per tool. Rules and
//! formatting live in `workflow` and `render`, so this file only maps names.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use crate::api::KaneoClient;
use crate::ops::{self, CardEdit, ListQuery, NewCard, OpError, Pause};
use crate::workflow::Decision;

const DEFAULT_RECENT: usize = 5;
const DEFAULT_LIST: u32 = 50;
const INSTRUCTIONS: &str = "Kaneo boards with enforced workflow rules. Cards are named by ticket (ABC-12) and projects by slug (ABC). Read a card with card_brief. Cards enter and leave the work-in-progress column and reach the final column only through start_card, pause_card and close_card, which apply the board rules; move_card covers every other column. Card writes answer `ok <ticket>`, comment writes `ok <comment id>`, and file_card answers the new ticket.";

pub struct KaneoServer {
    api: KaneoClient,
    /// Slug of the project holding the sprint cards, when the boards run sprints.
    sprint_slug: Option<String>,
    /// Built once: by default rmcp rebuilds the router and its schemas per call.
    tool_router: ToolRouter<Self>,
}

impl KaneoServer {
    pub fn new(api: KaneoClient, sprint_slug: Option<String>) -> Self {
        Self {
            api,
            sprint_slug,
            tool_router: Self::tool_router(),
        }
    }
}

/// A Kaneo or rule failure is the caller's to read, so it is a tool error,
/// never a protocol error the client would render opaquely.
fn result(r: Result<String, OpError>) -> CallToolResult {
    match r {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("error: {e}"))]),
    }
}

/// Record comments are stamped in UTC, so a board reads the same from every zone.
fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct BriefArgs {
    /// Ticket, like ABC-12.
    pub ticket: String,
    /// How many of the newest non-DECISION comments to include (default 5, at most 20).
    pub recent: Option<usize>,
    /// Show each comment's id, for edit_comment and delete_comment.
    #[serde(default)]
    pub comment_ids: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct StartArgs {
    pub ticket: String,
    /// Who is working the card, like a session or agent name.
    pub session: String,
    /// One or two sentences: what this session will do.
    pub plan: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct PauseArgs {
    pub ticket: String,
    pub session: String,
    /// Where the work stands.
    pub state: String,
    /// What the next session should do first.
    pub next_step: String,
    /// True when nothing can continue until someone decides: adds the operator-decision label.
    #[serde(default)]
    pub needs_decision: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CloseArgs {
    pub ticket: String,
    pub session: String,
    /// What proves it is done: PR or commit, test results, reviewer verdict.
    pub evidence: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct DecideArgs {
    pub ticket: String,
    /// The choice made, as one sentence.
    pub choice: String,
    pub reasoning: String,
    /// Who decided: the decider's words quoted, or the evidence when an agent decided alone. Kaneo credits every write to the API key's owner, so this is the only record of who decided.
    pub provenance: String,
    /// Any concern overruled.
    pub overruled: Option<String>,
    /// The sha, PR or runbook the decision covers.
    pub covers: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct DigestArgs {
    /// Project slug, like ABC.
    pub project: String,
    /// List Backlog cards too, not just their count.
    #[serde(default)]
    pub with_backlog: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CommentArgs {
    pub ticket: String,
    pub text: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// A full ticket like ABC-12 (ranked first), or words from the title or description.
    /// A bare number only matches text that contains it.
    pub query: String,
    /// Project slug to limit the search to.
    pub project: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct LabelArgs {
    pub ticket: String,
    /// Workspace label name, like operator-decision.
    pub name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct FileArgs {
    /// Project slug, like ABC.
    pub project: String,
    pub title: String,
    pub description: Option<String>,
    /// no-priority (default), low, medium, high or urgent.
    pub priority: Option<String>,
    /// Column slug or name. Defaults to the column the project description
    /// names ("New cards go to: Backlog"). Never the work-in-progress or the final column.
    pub column: Option<String>,
    /// 2026-11-01, or an RFC 3339 time.
    pub start: Option<String>,
    /// 2026-11-01, or an RFC 3339 time.
    pub due: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct LinkArgs {
    pub ticket: String,
    /// blocks (ticket blocks other), subtask (ticket is the parent of other) or related.
    pub kind: String,
    /// The other card, on any board of the workspace.
    pub other: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct EditArgs {
    pub ticket: String,
    pub title: Option<String>,
    /// Replaces the whole description.
    pub description: Option<String>,
    /// no-priority, low, medium, high or urgent.
    pub priority: Option<String>,
    /// 2026-11-01, an RFC 3339 time, or none to clear.
    pub start: Option<String>,
    /// 2026-11-01, an RFC 3339 time, or none to clear.
    pub due: Option<String>,
    /// A workspace member's name, or none to unassign.
    pub assignee: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    pub ticket: String,
    /// Column slug or name, like sprint or Backlog.
    pub column: String,
    /// Another project's slug, to move the card there (it gets a new ticket).
    pub project: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct EditCommentArgs {
    pub ticket: String,
    /// The comment id, from a write's `ok <id>` or card_brief with comment_ids.
    pub comment: String,
    /// The new text. A START, PAUSE, DONE or DECISION comment keeps its prefix.
    pub text: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct DeleteCommentArgs {
    pub ticket: String,
    pub comment: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Project slug, like ABC.
    pub project: String,
    /// Column slug or name, to list only that column.
    pub column: Option<String>,
    /// Oldest first instead of newest first.
    #[serde(default)]
    pub oldest_first: bool,
    /// Page number, from 1.
    pub page: Option<u32>,
    /// Cards per page (default 50, at most 100).
    pub limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TicketArgs {
    pub ticket: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ProjectArgs {
    /// Project slug, like ABC.
    pub project: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ProjectDescriptionArgs {
    /// Project slug, like ABC.
    pub project: String,
    /// The whole new description. Read it with project_info first: this replaces it.
    pub description: String,
}

#[tool_router]
impl KaneoServer {
    #[tool(
        description = "Read a card in one call: header (with the start and due dates), labels, relations, every DECISION comment, the newest comments and the description, in one call.",
        annotations(read_only_hint = true)
    )]
    async fn card_brief(&self, Parameters(a): Parameters<BriefArgs>) -> CallToolResult {
        let recent = a.recent.unwrap_or(DEFAULT_RECENT);
        result(ops::card_brief(&self.api, &a.ticket, recent, a.comment_ids).await)
    }

    #[tool(
        description = "Start a card: refuses when the work-in-progress column is at the project's cap, otherwise posts START and moves the card there."
    )]
    async fn start_card(&self, Parameters(a): Parameters<StartArgs>) -> CallToolResult {
        result(ops::start_card(&self.api, now(), &a.ticket, &a.session, &a.plan).await)
    }

    #[tool(
        description = "Pause a card that is in progress: posts PAUSE and moves it out of the work-in-progress column, to Sprint when sprints are configured and it is linked to an open sprint card, otherwise to Backlog. A board without that column gets its stated intake column, else its first open column."
    )]
    async fn pause_card(&self, Parameters(a): Parameters<PauseArgs>) -> CallToolResult {
        let pause = Pause {
            ticket: &a.ticket,
            session: &a.session,
            state: &a.state,
            next_step: &a.next_step,
            needs_decision: a.needs_decision,
            sprint_slug: self.sprint_slug.as_deref(),
        };
        result(ops::pause_card(&self.api, now(), &pause).await)
    }

    #[tool(
        description = "Close a card: posts DONE with the evidence and moves it to the final column."
    )]
    async fn close_card(&self, Parameters(a): Parameters<CloseArgs>) -> CallToolResult {
        result(ops::close_card(&self.api, now(), &a.ticket, &a.session, &a.evidence).await)
    }

    #[tool(description = "Record a decision of record as a DECISION comment, with its provenance.")]
    async fn decide(&self, Parameters(a): Parameters<DecideArgs>) -> CallToolResult {
        let decision = Decision {
            choice: &a.choice,
            reasoning: &a.reasoning,
            provenance: &a.provenance,
            overruled: a.overruled.as_deref(),
            covers: a.covers.as_deref(),
        };
        result(ops::decide(&self.api, now(), &a.ticket, &decision).await)
    }

    #[tool(
        description = "A project's board: open columns list their cards with their labels, Backlog and the final column give counts, with the work-in-progress column against its cap.",
        annotations(read_only_hint = true)
    )]
    async fn board_digest(&self, Parameters(a): Parameters<DigestArgs>) -> CallToolResult {
        result(ops::board_digest(&self.api, &a.project, a.with_backlog).await)
    }

    #[tool(description = "Post a plain comment on a card.")]
    async fn comment(&self, Parameters(a): Parameters<CommentArgs>) -> CallToolResult {
        result(ops::comment(&self.api, &a.ticket, &a.text).await)
    }

    #[tool(
        description = "Find cards by ticket (ABC-12, ranked first) or by words, one line per card with its labels.",
        annotations(read_only_hint = true)
    )]
    async fn find(&self, Parameters(a): Parameters<FindArgs>) -> CallToolResult {
        result(ops::find(&self.api, &a.query, a.project.as_deref()).await)
    }

    #[tool(description = "Attach a workspace label to a card, by name.")]
    async fn label(&self, Parameters(a): Parameters<LabelArgs>) -> CallToolResult {
        result(ops::label(&self.api, &a.ticket, &a.name).await)
    }

    #[tool(description = "Remove a label from a card, by name. The workspace label stays.")]
    async fn unlabel(&self, Parameters(a): Parameters<LabelArgs>) -> CallToolResult {
        result(ops::unlabel(&self.api, &a.ticket, &a.name).await)
    }

    #[tool(
        description = "File a new card in a project, in the column its description names for new cards unless told otherwise. Answers the new ticket."
    )]
    async fn file_card(&self, Parameters(a): Parameters<FileArgs>) -> CallToolResult {
        let card = NewCard {
            project: &a.project,
            title: &a.title,
            description: a.description.as_deref(),
            priority: a.priority.as_deref(),
            column: a.column.as_deref(),
            start: a.start.as_deref(),
            due: a.due.as_deref(),
        };
        result(ops::file_card(&self.api, &card).await)
    }

    #[tool(
        description = "Link two cards: blocks, subtask (ticket is the parent) or related. An existing link answers without a second write."
    )]
    async fn link(&self, Parameters(a): Parameters<LinkArgs>) -> CallToolResult {
        result(ops::link(&self.api, &a.ticket, &a.kind, &a.other).await)
    }

    #[tool(description = "Remove the link of that kind from ticket to other.")]
    async fn unlink(&self, Parameters(a): Parameters<LinkArgs>) -> CallToolResult {
        result(ops::unlink(&self.api, &a.ticket, &a.kind, &a.other).await)
    }

    #[tool(
        description = "Edit any subset of a card's title, description, priority, start and due dates, and assignee. Dates and assignee take none to clear."
    )]
    async fn edit_card(&self, Parameters(a): Parameters<EditArgs>) -> CallToolResult {
        let edit = CardEdit {
            title: a.title.as_deref(),
            description: a.description.as_deref(),
            priority: a.priority.as_deref(),
            start: a.start.as_deref(),
            due: a.due.as_deref(),
            assignee: a.assignee.as_deref(),
        };
        result(ops::edit_card(&self.api, &a.ticket, &edit).await)
    }

    #[tool(
        description = "Move a card to another column of its project, or to another project. The work-in-progress and the final column go through start_card, pause_card and close_card."
    )]
    async fn move_card(&self, Parameters(a): Parameters<MoveArgs>) -> CallToolResult {
        result(ops::move_card(&self.api, &a.ticket, &a.column, a.project.as_deref()).await)
    }

    #[tool(
        description = "Replace a comment's text. The comment must be on that card, and keeps its kind (START, PAUSE, DONE, DECISION or plain)."
    )]
    async fn edit_comment(&self, Parameters(a): Parameters<EditCommentArgs>) -> CallToolResult {
        result(ops::edit_comment(&self.api, &a.ticket, &a.comment, &a.text).await)
    }

    #[tool(
        description = "Delete a plain comment from a card. START, PAUSE, DONE and DECISION comments are the record and stay: edit them instead.",
        annotations(destructive_hint = true)
    )]
    async fn delete_comment(&self, Parameters(a): Parameters<DeleteCommentArgs>) -> CallToolResult {
        result(ops::delete_comment(&self.api, &a.ticket, &a.comment).await)
    }

    #[tool(
        description = "A project's cards by creation date, newest first unless asked, one line each with its labels, under a page header. Filter to one column.",
        annotations(read_only_hint = true)
    )]
    async fn list_cards(&self, Parameters(a): Parameters<ListArgs>) -> CallToolResult {
        let query = ListQuery {
            project: &a.project,
            column: a.column.as_deref(),
            newest_first: !a.oldest_first,
            page: a.page.unwrap_or(1),
            limit: a.limit.unwrap_or(DEFAULT_LIST),
        };
        result(ops::list_cards(&self.api, &query).await)
    }

    #[tool(
        description = "A card's history without its comments: when it was created and each status change, oldest first.",
        annotations(read_only_hint = true)
    )]
    async fn card_activity(&self, Parameters(a): Parameters<TicketArgs>) -> CallToolResult {
        result(ops::card_activity(&self.api, &a.ticket).await)
    }

    #[tool(
        description = "A project's columns, the rules read from its description (work-in-progress column, cap, intake column) and the whole description.",
        annotations(read_only_hint = true)
    )]
    async fn project_info(&self, Parameters(a): Parameters<ProjectArgs>) -> CallToolResult {
        result(ops::project_info(&self.api, &a.project).await)
    }

    #[tool(
        description = "Replace a project's whole description. Its rule lines may tighten the rules but not loosen them: the work-in-progress column stays, and its cap does not rise. Answers the rules the server now reads from it."
    )]
    async fn edit_project_description(
        &self,
        Parameters(a): Parameters<ProjectDescriptionArgs>,
    ) -> CallToolResult {
        result(ops::edit_project_description(&self.api, &a.project, &a.description).await)
    }

    #[tool(
        description = "Every project the key sees, by workspace, with each workspace's label names.",
        annotations(read_only_hint = true)
    )]
    async fn projects(&self) -> CallToolResult {
        result(ops::projects(&self.api).await)
    }
}

// Named explicitly: without it rmcp reports its own crate name and version.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for KaneoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}
