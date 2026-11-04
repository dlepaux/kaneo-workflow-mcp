//! The slices of Kaneo's API objects this server reads. Every struct names only
//! the fields a tool prints or decides on, so serde drops the rest at the
//! boundary and nothing else can leak into a result.

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub number: u32,
    pub status: String,
    #[serde(default)]
    pub priority: Option<String>,
    pub project_id: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub due_date: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    /// Kaneo's PUT /task/{id} replaces every field, so an edit sends these
    /// back unchanged.
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub assignee_name: Option<String>,
    /// Kaneo's task list sends each card's labels with it (2.46), so a listing
    /// shows them with no read per card. A single-card read leaves this empty.
    #[serde(default)]
    pub labels: Vec<TaskLabel>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskLabel {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    pub content: String,
    pub created_at: String,
    #[serde(default)]
    pub user: Option<Author>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Author {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub id: String,
    pub relation_type: String,
    pub source_task: RelatedTask,
    pub target_task: RelatedTask,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedTask {
    pub id: String,
    pub title: String,
    pub number: u32,
    pub status: String,
    pub project_id: String,
    /// Required on purpose: the pause rule depends on it, so a Kaneo that
    /// stops sending it must fail loudly rather than count every card open.
    pub is_completed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Sent back unchanged by a description edit: Kaneo's PUT replaces them too.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub is_public: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_final: bool,
}

/// A label row. Kaneo keeps one row per workspace label (no `taskId`) and one
/// copy per card that carries it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// A workspace member, reduced to what assigning a card needs. The email
/// Kaneo also sends is dropped here, so it never reaches a result.
#[derive(Debug, Clone, Deserialize)]
pub struct Member {
    pub id: String,
    pub name: String,
}

/// One entry of a card's history, newest first as Kaneo sends it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: String,
    #[serde(default)]
    pub event_data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskPage {
    pub data: Board,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub columns: Vec<BoardColumn>,
    /// Cards outside every column. Kaneo pages over them with the rest, so a
    /// listing that skipped them would print fewer lines than it counts.
    #[serde(default)]
    pub planned_tasks: Vec<Task>,
    #[serde(default)]
    pub archived_tasks: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardColumn {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub is_final: bool,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub total_pages: u32,
    #[serde(default)]
    pub total: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResults {
    pub results: Vec<SearchHit>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// The card's id, which ties a hit to its labels.
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    #[serde(default)]
    pub project_slug: Option<String>,
    #[serde(default)]
    pub task_number: Option<u32>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
}

/// What every write endpoint returns, reduced to the one field a write reports.
#[derive(Debug, Clone, Deserialize)]
pub struct Created {
    pub id: String,
}

/// A created or moved card. The number is optional so a write that succeeded
/// still reports its id if Kaneo ever stops sending it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedTask {
    pub id: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Moved {
    pub task: CreatedTask,
}
