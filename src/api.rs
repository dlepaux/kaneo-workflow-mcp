//! A thin typed client over the Kaneo REST endpoints the tools use. The paths
//! and verbs mirror @kaneo/mcp 0.1.12 (dist/tools/register.js) and the Kaneo
//! API sources, so a Kaneo upgrade that breaks one breaks the other too.

use std::time::Duration;

use reqwest::{Method, Url};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::model::{
    Activity, Column, Comment, Created, CreatedTask, Label, Member, Moved, Project, Relation,
    SearchHit, SearchResults, Task, TaskPage, Workspace,
};

/// Kaneo's own MCP server waits 10 s per request; an agent waits no longer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Kaneo error bodies can be whole HTML pages; a result only needs the gist.
const MAX_ERROR_CHARS: usize = 300;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{path}: HTTP {status}: {message}")]
    Http {
        path: String,
        status: u16,
        message: String,
    },
    #[error("{path}: {message}")]
    Transport { path: String, message: String },
    #[error("{path}: unexpected response: {message}")]
    Decode { path: String, message: String },
    #[error("bad base URL {0}")]
    BaseUrl(String),
}

#[derive(Clone)]
pub struct KaneoClient {
    http: reqwest::Client,
    base: Url,
    api_key: String,
}

impl std::fmt::Debug for KaneoClient {
    // The key never reaches a log line, a panic message or a tool result.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KaneoClient")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

/// Accepts the origin with or without a trailing `/api`, as @kaneo/mcp does,
/// so a KANEO_API_URL written for it works here unchanged.
pub fn normalize_base_url(raw: &str) -> Result<Url, ApiError> {
    let trimmed = raw.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix("/api").unwrap_or(trimmed);
    let url = Url::parse(trimmed).map_err(|_| ApiError::BaseUrl(trimmed.to_owned()))?;
    let local = match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    // The key travels as a Bearer header: plain http only where it never
    // leaves the machine.
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && local);
    if url.cannot_be_a_base() || !scheme_ok {
        return Err(ApiError::BaseUrl(trimmed.to_owned()));
    }
    Ok(url)
}

impl KaneoClient {
    pub fn new(base_url: &str, api_key: String) -> Result<Self, ApiError> {
        let base = normalize_base_url(base_url)?;
        // A redirect would carry the request, body included, to a host the
        // operator never named: the configured origin is the only one spoken to.
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("kaneo-workflow-mcp/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ApiError::Transport {
                path: String::new(),
                message: e.to_string(),
            })?;
        Ok(Self {
            http,
            base,
            api_key,
        })
    }

    /// Builds `<base>/api/<segments...>` with each segment percent-encoded, so an
    /// id or ticket can never add a path segment of its own.
    fn url(&self, segments: &[&str], query: &[(&str, &str)]) -> Url {
        let mut url = self.base.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("normalize_base_url rejects cannot-be-a-base URLs");
            path.pop_if_empty().push("api");
            path.extend(segments);
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        url
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<T, ApiError> {
        let url = self.url(segments, query);
        let path = url.path().to_owned();
        let mut req = self.http.request(method, url).bearer_auth(&self.api_key);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.map_err(|e| ApiError::Transport {
            path: path.clone(),
            message: e.without_url().to_string(),
        })?;
        let status = res.status();
        let text = res.text().await.map_err(|e| ApiError::Transport {
            path: path.clone(),
            message: e.without_url().to_string(),
        })?;
        if !status.is_success() {
            return Err(ApiError::Http {
                path,
                status: status.as_u16(),
                message: error_message(&text),
            });
        }
        serde_json::from_str(&text).map_err(|e| ApiError::Decode {
            path,
            message: e.to_string(),
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, &str)],
    ) -> Result<T, ApiError> {
        self.call(Method::GET, segments, query, None).await
    }

    pub async fn workspaces(&self) -> Result<Vec<Workspace>, ApiError> {
        self.get(&["auth", "organization", "list"], &[]).await
    }

    pub async fn projects(&self, workspace_id: &str) -> Result<Vec<Project>, ApiError> {
        self.get(&["project"], &[("workspaceId", workspace_id)])
            .await
    }

    pub async fn project(&self, project_id: &str) -> Result<Project, ApiError> {
        self.get(&["project", project_id], &[]).await
    }

    pub async fn columns(&self, project_id: &str) -> Result<Vec<Column>, ApiError> {
        self.get(&["column", project_id], &[]).await
    }

    pub async fn task_by_ticket(&self, ticket: &str) -> Result<Task, ApiError> {
        self.get(&["task", "by-ticket-id", ticket], &[]).await
    }

    /// One page of a project's board. `status` filters to one column slug.
    pub async fn tasks(
        &self,
        project_id: &str,
        status: Option<&str>,
        page: u32,
    ) -> Result<TaskPage, ApiError> {
        let page = page.to_string();
        let mut query = vec![("limit", "100"), ("page", page.as_str())];
        if let Some(status) = status {
            query.push(("status", status));
        }
        self.get(&["task", "tasks", project_id], &query).await
    }

    /// One page of a project's cards in creation order, sorted by Kaneo before
    /// it pages, so page 2 follows page 1 across every column.
    pub async fn tasks_by_creation(
        &self,
        project_id: &str,
        status: Option<&str>,
        page: u32,
        limit: u32,
        newest_first: bool,
    ) -> Result<TaskPage, ApiError> {
        let (page, limit) = (page.to_string(), limit.to_string());
        let order = if newest_first { "desc" } else { "asc" };
        let mut query = vec![
            ("limit", limit.as_str()),
            ("page", page.as_str()),
            ("sortBy", "createdAt"),
            ("sortOrder", order),
        ];
        if let Some(status) = status {
            query.push(("status", status));
        }
        self.get(&["task", "tasks", project_id], &query).await
    }

    /// Newest first, as Kaneo orders them.
    pub async fn activity(&self, task_id: &str) -> Result<Vec<Activity>, ApiError> {
        self.get(&["activity", task_id], &[]).await
    }

    pub async fn members(&self, workspace_id: &str) -> Result<Vec<Member>, ApiError> {
        self.get(&["workspace", workspace_id, "members"], &[]).await
    }

    /// Oldest first, as Kaneo orders them.
    pub async fn comments(&self, task_id: &str) -> Result<Vec<Comment>, ApiError> {
        self.get(&["comment", task_id], &[]).await
    }

    pub async fn relations(&self, task_id: &str) -> Result<Vec<Relation>, ApiError> {
        self.get(&["task-relation", task_id], &[]).await
    }

    pub async fn task_labels(&self, task_id: &str) -> Result<Vec<Label>, ApiError> {
        self.get(&["label", "task", task_id], &[]).await
    }

    /// Every label row of the workspace: the workspace labels and every card's
    /// copy. Large (191k characters on a busy instance in October 2026), which is
    /// why only the server ever reads it.
    pub async fn workspace_labels(&self, workspace_id: &str) -> Result<Vec<Label>, ApiError> {
        self.get(&["label", "workspace", workspace_id], &[]).await
    }

    pub async fn search_tasks(
        &self,
        workspace_id: &str,
        q: &str,
        project_id: Option<&str>,
    ) -> Result<Vec<SearchHit>, ApiError> {
        let mut query = vec![
            ("q", q),
            ("type", "tasks"),
            ("limit", "20"),
            ("workspaceId", workspace_id),
        ];
        if let Some(project_id) = project_id {
            query.push(("projectId", project_id));
        }
        let found: SearchResults = self.get(&["search"], &query).await?;
        Ok(found.results)
    }

    pub async fn post_comment(&self, task_id: &str, content: &str) -> Result<String, ApiError> {
        let created: Created = self
            .call(
                Method::POST,
                &["comment", task_id],
                &[],
                Some(json!({ "content": content })),
            )
            .await?;
        Ok(created.id)
    }

    pub async fn set_status(&self, task_id: &str, status: &str) -> Result<String, ApiError> {
        let updated: Created = self
            .call(
                Method::PUT,
                &["task", "status", task_id],
                &[],
                Some(json!({ "status": status })),
            )
            .await?;
        Ok(updated.id)
    }

    /// Kaneo copies a workspace label onto the card; the workspace row stays.
    pub async fn attach_label(&self, label_id: &str, task_id: &str) -> Result<String, ApiError> {
        let attached: Created = self
            .call(
                Method::PUT,
                &["label", label_id, "task"],
                &[],
                Some(json!({ "taskId": task_id })),
            )
            .await?;
        Ok(attached.id)
    }

    pub async fn detach_label(&self, label_id: &str) -> Result<(), ApiError> {
        let _: Value = self
            .call(Method::DELETE, &["label", label_id, "task"], &[], None)
            .await?;
        Ok(())
    }

    pub async fn create_task(
        &self,
        project_id: &str,
        body: Value,
    ) -> Result<CreatedTask, ApiError> {
        self.call(Method::POST, &["task", project_id], &[], Some(body))
            .await
    }

    /// Kaneo replaces every field it is sent and clears the dates and the
    /// assignee it is not sent, so `body` must carry the whole card.
    pub async fn update_task(&self, task_id: &str, body: Value) -> Result<String, ApiError> {
        let updated: Created = self
            .call(Method::PUT, &["task", task_id], &[], Some(body))
            .await?;
        Ok(updated.id)
    }

    /// One of Kaneo's single-field routes (`title`, `priority`, `assignee`,
    /// `due-date`, `description`). They never touch the status or position,
    /// so an edit cannot undo a move another session made meanwhile.
    pub async fn update_task_field(
        &self,
        route: &str,
        task_id: &str,
        body: Value,
    ) -> Result<String, ApiError> {
        let updated: Created = self
            .call(Method::PUT, &["task", route, task_id], &[], Some(body))
            .await?;
        Ok(updated.id)
    }

    /// The card gets a new number in the destination project.
    pub async fn move_task(
        &self,
        task_id: &str,
        project_id: &str,
        status: &str,
    ) -> Result<CreatedTask, ApiError> {
        let moved: Moved = self
            .call(
                Method::PUT,
                &["task", "move", task_id],
                &[],
                Some(json!({ "destinationProjectId": project_id, "destinationStatus": status })),
            )
            .await?;
        Ok(moved.task)
    }

    /// `subtask`: source is the parent. `blocks`: source blocks target.
    pub async fn create_relation(
        &self,
        source_id: &str,
        target_id: &str,
        kind: &str,
    ) -> Result<String, ApiError> {
        let created: Created = self
            .call(
                Method::POST,
                &["task-relation"],
                &[],
                Some(json!({
                    "sourceTaskId": source_id,
                    "targetTaskId": target_id,
                    "relationType": kind,
                })),
            )
            .await?;
        Ok(created.id)
    }

    pub async fn delete_relation(&self, relation_id: &str) -> Result<(), ApiError> {
        let _: Value = self
            .call(Method::DELETE, &["task-relation", relation_id], &[], None)
            .await?;
        Ok(())
    }

    pub async fn update_comment(
        &self,
        comment_id: &str,
        content: &str,
    ) -> Result<String, ApiError> {
        let updated: Created = self
            .call(
                Method::PUT,
                &["comment", comment_id],
                &[],
                Some(json!({ "content": content })),
            )
            .await?;
        Ok(updated.id)
    }

    pub async fn delete_comment(&self, comment_id: &str) -> Result<(), ApiError> {
        let _: Value = self
            .call(Method::DELETE, &["comment", comment_id], &[], None)
            .await?;
        Ok(())
    }

    /// A full replace, like the task PUT: name, icon, slug, description and
    /// visibility all travel.
    pub async fn update_project(&self, project_id: &str, body: Value) -> Result<String, ApiError> {
        let updated: Created = self
            .call(Method::PUT, &["project", project_id], &[], Some(body))
            .await?;
        Ok(updated.id)
    }
}

/// Kaneo answers errors as `{"message": ...}` JSON or as plain text.
fn error_message(body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| body.trim().to_owned());
    if message.is_empty() {
        return "no body".to_owned();
    }
    if message.chars().count() > MAX_ERROR_CHARS {
        let cut: String = message.chars().take(MAX_ERROR_CHARS).collect();
        return format!("{cut}…");
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_drops_trailing_api_and_slashes() {
        for raw in [
            "https://kaneo.example.com",
            "https://kaneo.example.com/",
            "https://kaneo.example.com/api",
            "https://kaneo.example.com/api/",
        ] {
            assert_eq!(
                normalize_base_url(raw).unwrap().as_str(),
                "https://kaneo.example.com/"
            );
        }
    }

    #[test]
    fn base_url_allows_plain_http_only_on_this_machine() {
        for local in [
            "http://kaneo.localhost",
            "http://localhost:1337",
            "http://127.0.0.1:9",
            "http://[::1]:8",
        ] {
            assert!(normalize_base_url(local).is_ok(), "{local}");
        }
        assert!(normalize_base_url("http://kaneo.example.com").is_err());
        assert!(normalize_base_url("file:///etc/passwd").is_err());
        assert!(normalize_base_url("not a url").is_err());
    }

    #[test]
    fn segments_are_encoded_not_spliced() {
        let client = KaneoClient::new("http://kaneo.localhost", "k".into()).unwrap();
        let url = client.url(&["task", "by-ticket-id", "WEB-1/../../user"], &[]);
        assert_eq!(
            url.as_str(),
            "http://kaneo.localhost/api/task/by-ticket-id/WEB-1%2F..%2F..%2Fuser"
        );
    }

    #[test]
    fn debug_hides_the_key() {
        let client = KaneoClient::new("http://kaneo.localhost", "secret-key".into()).unwrap();
        assert!(!format!("{client:?}").contains("secret-key"));
    }

    #[test]
    fn error_message_prefers_json_message_and_truncates() {
        assert_eq!(
            error_message(r#"{"message":"Task not found"}"#),
            "Task not found"
        );
        assert_eq!(error_message(""), "no body");
        let long = "x".repeat(1000);
        assert_eq!(error_message(&long).chars().count(), MAX_ERROR_CHARS + 1);
    }
}
