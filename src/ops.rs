//! Each tool as one async function: fetch what the rule needs, apply the rule
//! from `workflow`, write, and render. The MCP layer only maps arguments.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use crate::api::{ApiError, KaneoClient};
use crate::model::{BoardColumn, Member, Project, Relation, Task};
use crate::render::{self, Brief, CardList, Slugs};
use crate::workflow::{self, Decision, RuleError, Ticket};

/// 20 pages of 100 cards. A board past that is a different problem than a
/// digest can solve, and the bound keeps one call under the key's rate limit.
const MAX_PAGES: u32 = 20;
pub const MAX_RECENT: usize = 20;
pub const MAX_LIST: u32 = 100;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Rule(#[from] RuleError),
    #[error("{0}")]
    NotFound(String),
    #[error("{source} (already applied: {applied})")]
    Partial {
        applied: String,
        source: Box<OpError>,
    },
}

struct Card {
    ticket: Ticket,
    task: Task,
}

/// Kaneo resolves a ticket across every workspace the key sees; a key per
/// instance with unique project slugs keeps that unambiguous.
async fn card(api: &KaneoClient, raw: &str) -> Result<Card, OpError> {
    let ticket = workflow::parse_ticket(raw)?;
    let task = api.task_by_ticket(&ticket.to_string()).await?;
    Ok(Card { ticket, task })
}

fn workspace_of(task: &Task) -> Result<&str, OpError> {
    task.workspace_id
        .as_deref()
        .ok_or_else(|| OpError::NotFound(format!("card {} has no workspace", task.id)))
}

/// Finds a project by slug in every workspace the key can see.
async fn project_by_slug(api: &KaneoClient, slug: &str) -> Result<(String, Project), OpError> {
    let slug = slug.trim().to_ascii_uppercase();
    for ws in api.workspaces().await? {
        if let Some(p) = api
            .projects(&ws.id)
            .await?
            .into_iter()
            .find(|p| p.slug == slug)
        {
            return Ok((ws.id, p));
        }
    }
    Err(OpError::NotFound(format!("no project {slug}")))
}

pub async fn card_brief(
    api: &KaneoClient,
    raw: &str,
    recent: usize,
    ids: bool,
) -> Result<String, OpError> {
    let Card { ticket, task } = card(api, raw).await?;
    let comments = api.comments(&task.id).await?;
    let relations = api.relations(&task.id).await?;
    let labels = api.task_labels(&task.id).await?;
    let slugs: Slugs = if relations.is_empty() {
        Slugs::new()
    } else {
        api.projects(workspace_of(&task)?)
            .await?
            .into_iter()
            .map(|p| (p.id, p.slug))
            .collect()
    };
    Ok(render::card_brief(&Brief {
        ticket: &ticket.to_string(),
        task: &task,
        labels: &labels,
        relations: &relations,
        comments: &comments,
        slugs: &slugs,
        recent: recent.min(MAX_RECENT),
        ids,
    }))
}

/// Every check runs before the first write, and every lifecycle write posts
/// its comment before it moves the card: a failed move leaves the record and
/// an error, never a silent move.
pub async fn start_card(
    api: &KaneoClient,
    now: DateTime<Utc>,
    raw: &str,
    session: &str,
    plan: &str,
) -> Result<String, OpError> {
    let session = workflow::required("session", session)?;
    let plan = workflow::required("plan", plan)?;
    let Card { ticket, task } = card(api, raw).await?;
    let project = api.project(&task.project_id).await?;
    let description = project.description.as_deref().unwrap_or("");
    let columns = api.columns(&project.id).await?;
    let active =
        workflow::active_column(&columns, description)?.ok_or(RuleError::NoActiveColumn)?;
    let cap = workflow::cap_rule(&columns, description, Some(active))?;
    // A card already in progress is being taken over: it adds nothing to the
    // column, so the cap does not apply even when the column is over it.
    if task.status != active.slug {
        // One page of 100 is enough: any cap below 100 is already reached.
        let held = api.tasks(&project.id, Some(&active.slug), 1).await?;
        let held: Vec<String> = held
            .data
            .columns
            .iter()
            .filter(|c| c.slug == active.slug)
            .flat_map(|c| &c.tasks)
            .map(|t| format!("{}-{}", ticket.slug, t.number))
            .collect();
        workflow::check_cap(&active.name, cap, &held)?;
    }
    api.post_comment(&task.id, &workflow::start_comment(now, session, plan))
        .await?;
    api.set_status(&task.id, &active.slug).await?;
    Ok(render::moved(&ticket.to_string(), &active.slug))
}

pub struct Pause<'a> {
    pub ticket: &'a str,
    pub session: &'a str,
    pub state: &'a str,
    pub next_step: &'a str,
    pub needs_decision: bool,
    /// The project holding the sprint cards, when the board runs sprints.
    pub sprint_slug: Option<&'a str>,
}

/// PAUSE, then out of the work-in-progress column to wherever the board rule
/// sends it. A card that waits on a decision also gets `operator-decision`,
/// so the wait is visible. The result names the destination.
pub async fn pause_card(
    api: &KaneoClient,
    now: DateTime<Utc>,
    p: &Pause<'_>,
) -> Result<String, OpError> {
    let session = workflow::required("session", p.session)?;
    let state = workflow::required("state", p.state)?;
    let next = workflow::required("next_step", p.next_step)?;
    let Card { ticket, task } = card(api, p.ticket).await?;
    let workspace = workspace_of(&task)?;
    let projects = api.projects(workspace).await?;
    let columns = api.columns(&task.project_id).await?;
    // A miss must not read as "no intake line": that error would blame the board.
    let description = projects
        .iter()
        .find(|pr| pr.id == task.project_id)
        .ok_or_else(|| {
            OpError::NotFound(format!(
                "project {} is missing from its workspace's project list",
                task.project_id
            ))
        })?
        .description
        .as_deref()
        .unwrap_or("");
    let active =
        workflow::active_column(&columns, description)?.ok_or(RuleError::NoActiveColumn)?;
    workflow::check_in_progress(&ticket.to_string(), &task.status, active)?;
    let sprint_project = p.sprint_slug.and_then(|slug| {
        projects
            .iter()
            .find(|pr| pr.slug.eq_ignore_ascii_case(slug))
    });
    // Without a sprints project the rule never reads the card's links.
    let relations = match sprint_project {
        Some(_) => api.relations(&task.id).await?,
        None => Vec::new(),
    };
    let rule = workflow::pause_destination(
        &relations,
        &task.id,
        sprint_project.map(|pr| pr.id.as_str()),
    );
    let destination = workflow::pause_column(&columns, rule, description, Some(&active.slug))?
        .slug
        .clone();
    let label_id = if p.needs_decision {
        Some(workspace_label_id(api, workspace, workflow::OPERATOR_DECISION).await?)
    } else {
        None
    };
    api.post_comment(
        &task.id,
        &workflow::pause_comment(now, session, state, next),
    )
    .await?;
    api.set_status(&task.id, &destination).await?;
    if let Some(label_id) = label_id {
        api.attach_label(&label_id, &task.id).await?;
    }
    Ok(render::moved(&ticket.to_string(), &destination))
}

/// Evidence is the only way a card reaches the final column.
pub async fn close_card(
    api: &KaneoClient,
    now: DateTime<Utc>,
    raw: &str,
    session: &str,
    evidence: &str,
) -> Result<String, OpError> {
    let session = workflow::required("session", session)?;
    let evidence = workflow::required("evidence", evidence)?;
    let Card { ticket, task } = card(api, raw).await?;
    let columns = api.columns(&task.project_id).await?;
    let done = workflow::final_column(&columns)?;
    api.post_comment(&task.id, &workflow::done_comment(now, session, evidence))
        .await?;
    api.set_status(&task.id, done).await?;
    Ok(render::moved(&ticket.to_string(), done))
}

pub async fn decide(
    api: &KaneoClient,
    now: DateTime<Utc>,
    raw: &str,
    d: &Decision<'_>,
) -> Result<String, OpError> {
    let decision = Decision {
        choice: workflow::required("choice", d.choice)?,
        reasoning: workflow::required("reasoning", d.reasoning)?,
        provenance: workflow::required("provenance", d.provenance)?,
        overruled: d.overruled.map(str::trim).filter(|s| !s.is_empty()),
        covers: d.covers.map(str::trim).filter(|s| !s.is_empty()),
    };
    let Card { task, .. } = card(api, raw).await?;
    let id = api
        .post_comment(&task.id, &workflow::decision_comment(now, &decision))
        .await?;
    Ok(render::ok(&id))
}

/// Lifecycle and decision comments only go through their own tools, which
/// apply the rules; a plain comment cannot impersonate one.
pub async fn comment(api: &KaneoClient, raw: &str, text: &str) -> Result<String, OpError> {
    let text = workflow::required("text", text)?;
    workflow::plain_comment(text)?;
    let Card { task, .. } = card(api, raw).await?;
    Ok(render::ok(&api.post_comment(&task.id, text).await?))
}

/// The workspace label (the row with no task), never a card's copy:
/// attaching a copy moves it off the card that had it. Kaneo answers a
/// repeated attach with the card's existing copy, so attaching is idempotent.
async fn workspace_label_id(
    api: &KaneoClient,
    workspace: &str,
    name: &str,
) -> Result<String, OpError> {
    let rows = api.workspace_labels(workspace).await?;
    if let Some(label) = rows
        .iter()
        .find(|l| l.task_id.is_none() && l.name.eq_ignore_ascii_case(name))
    {
        return Ok(label.id.clone());
    }
    let mut names: Vec<&str> = rows
        .iter()
        .filter(|l| l.task_id.is_none())
        .map(|l| l.name.as_str())
        .collect();
    names.sort_unstable();
    Err(OpError::NotFound(format!(
        "no workspace label {name:?}; there are: {}",
        names.join(", ")
    )))
}

pub async fn label(api: &KaneoClient, raw: &str, name: &str) -> Result<String, OpError> {
    let name = workflow::required("name", name)?;
    let Card { ticket, task } = card(api, raw).await?;
    let label_id = workspace_label_id(api, workspace_of(&task)?, name).await?;
    api.attach_label(&label_id, &task.id).await?;
    Ok(render::ok(&ticket.to_string()))
}

/// Removes the card's own copy of the label; the workspace label stays.
pub async fn unlabel(api: &KaneoClient, raw: &str, name: &str) -> Result<String, OpError> {
    let name = workflow::required("name", name)?;
    let Card { ticket, task } = card(api, raw).await?;
    let copy = api
        .task_labels(&task.id)
        .await?
        .into_iter()
        .find(|l| l.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| OpError::NotFound(format!("{ticket} has no label {name:?}")))?;
    api.detach_label(&copy.id).await?;
    Ok(render::ok(&ticket.to_string()))
}

pub async fn board_digest(
    api: &KaneoClient,
    slug: &str,
    with_backlog: bool,
) -> Result<String, OpError> {
    let (_, project) = project_by_slug(api, slug).await?;
    let first = api.tasks(&project.id, None, 1).await?;
    let pages = first.pagination.total_pages.min(MAX_PAGES);
    let mut columns: Vec<BoardColumn> = first.data.columns;
    for page in 2..=pages {
        let next = api.tasks(&project.id, None, page).await?;
        for col in next.data.columns {
            match columns.iter_mut().find(|c| c.slug == col.slug) {
                Some(existing) => existing.tasks.extend(col.tasks),
                None => columns.push(col),
            }
        }
    }
    let description = project.description.as_deref().unwrap_or("");
    let board = api.columns(&project.id).await?;
    let active = workflow::active_column(&board, description)?;
    let cap = workflow::cap_rule(&board, description, active)?;
    let wip = active.map(|active| render::Wip {
        slug: &active.slug,
        name: &active.name,
        cap,
    });
    let mut out = render::board_digest(&project.slug, &project.name, &columns, wip, with_backlog);
    if first.pagination.total_pages > MAX_PAGES {
        out.push_str(&format!(
            "\n\n(only the first {MAX_PAGES} pages of 100 cards were read)"
        ));
    }
    Ok(out)
}

pub async fn find(
    api: &KaneoClient,
    query: &str,
    project: Option<&str>,
) -> Result<String, OpError> {
    let query = workflow::required("query", query)?;
    let mut labels = render::CardLabels::new();
    let hits = match project.map(str::trim).filter(|s| !s.is_empty()) {
        Some(slug) => {
            let (ws, p) = project_by_slug(api, slug).await?;
            let hits = api.search_tasks(&ws, query, Some(&p.id)).await?;
            add_card_labels(api, &ws, &hits, &mut labels).await?;
            hits
        }
        None => {
            let mut hits = Vec::new();
            for ws in api.workspaces().await? {
                let found = api.search_tasks(&ws.id, query, None).await?;
                add_card_labels(api, &ws.id, &found, &mut labels).await?;
                hits.extend(found);
            }
            hits
        }
    };
    Ok(render::search_hits(&hits, &labels))
}

/// The labels of a workspace's cards, read once for all its hits: a search hit
/// carries none, and one read per hit would spend the key's shared rate limit.
async fn add_card_labels(
    api: &KaneoClient,
    workspace: &str,
    hits: &[crate::model::SearchHit],
    labels: &mut render::CardLabels,
) -> Result<(), OpError> {
    if hits.is_empty() {
        return Ok(());
    }
    for row in api.workspace_labels(workspace).await? {
        if let Some(task) = row.task_id {
            labels.entry(task).or_default().push(row.name);
        }
    }
    Ok(())
}

fn ticket_of(slug: &str, number: Option<u32>, id: &str) -> String {
    number.map_or(id.to_owned(), |n| format!("{slug}-{n}"))
}

/// A field an edit may set, or clear with `none`.
fn clearable(value: Option<&str>) -> Option<Option<&str>> {
    let value = value?.trim();
    Some((!value.eq_ignore_ascii_case(workflow::NONE)).then_some(value))
}

pub struct NewCard<'a> {
    pub project: &'a str,
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub priority: Option<&'a str>,
    /// Slug or name; the project's stated intake column when absent.
    pub column: Option<&'a str>,
    pub start: Option<&'a str>,
    pub due: Option<&'a str>,
}

/// Files a card where the board says new cards go, unless told otherwise,
/// and never straight into the work-in-progress or the final column.
pub async fn file_card(api: &KaneoClient, c: &NewCard<'_>) -> Result<String, OpError> {
    let title = workflow::required("title", c.title)?;
    let priority = workflow::priority(c.priority.unwrap_or("no-priority"))?;
    let start = c.start.map(|d| workflow::date("start", d)).transpose()?;
    let due = c.due.map(|d| workflow::date("due", d)).transpose()?;
    workflow::check_date_order(start.as_deref(), due.as_deref())?;
    let (_, project) = project_by_slug(api, c.project).await?;
    let description = project.description.as_deref().unwrap_or("");
    let columns = api.columns(&project.id).await?;
    let active = workflow::active_column(&columns, description)?;
    let column = match c.column.map(str::trim).filter(|s| !s.is_empty()) {
        Some(wanted) => workflow::resolve_column(&columns, wanted)?,
        None => workflow::intake_column(&columns, description, active.map(|c| c.slug.as_str()))?,
    };
    workflow::check_enter(column, active)?;
    let mut body = json!({
        "title": title,
        "description": c.description.unwrap_or("").trim(),
        "priority": priority,
        "status": column.slug,
    });
    if let Some(start) = start {
        body["startDate"] = json!(start);
    }
    if let Some(due) = due {
        body["dueDate"] = json!(due);
    }
    let created = api.create_task(&project.id, body).await?;
    Ok(render::ok(&ticket_of(
        &project.slug,
        created.number,
        &created.id,
    )))
}

/// The relation between two cards of that kind, read from `from`'s side:
/// `blocks` and `subtask` have a direction, `related` does not.
fn find_relation<'a>(
    relations: &'a [Relation],
    kind: &str,
    from: &str,
    to: &str,
) -> Option<&'a Relation> {
    relations.iter().find(|r| {
        let (s, t) = (r.source_task.id.as_str(), r.target_task.id.as_str());
        r.relation_type == kind
            && ((s == from && t == to) || (kind == "related" && s == to && t == from))
    })
}

/// Two cards and their shared workspace: Kaneo links across boards, never
/// across workspaces.
async fn pair(api: &KaneoClient, a: &str, b: &str) -> Result<(Card, Card), OpError> {
    let (a, b) = (card(api, a).await?, card(api, b).await?);
    if a.task.id == b.task.id {
        return Err(RuleError::SelfLink.into());
    }
    if workspace_of(&a.task)? != workspace_of(&b.task)? {
        return Err(RuleError::CrossWorkspace(a.ticket.to_string(), b.ticket.to_string()).into());
    }
    Ok((a, b))
}

/// `ticket blocks other`, `ticket is the parent of other` (subtask), or
/// `related`. An existing link answers its id without a second write.
pub async fn link(
    api: &KaneoClient,
    ticket: &str,
    kind: &str,
    other: &str,
) -> Result<String, OpError> {
    let kind = workflow::relation_kind(kind)?;
    let (from, to) = pair(api, ticket, other).await?;
    let relations = api.relations(&from.task.id).await?;
    let answer = render::ok(&format!("{} {kind} {}", from.ticket, to.ticket));
    if find_relation(&relations, kind, &from.task.id, &to.task.id).is_some() {
        return Ok(answer);
    }
    // Kaneo refuses the reverse of an existing link as a duplicate; saying
    // which link is in the way is the useful answer.
    if find_relation(&relations, kind, &to.task.id, &from.task.id).is_some() {
        return Err(RuleError::ReverseLink {
            kind,
            ticket: from.ticket.to_string(),
            other: to.ticket.to_string(),
        }
        .into());
    }
    api.create_relation(&from.task.id, &to.task.id, kind)
        .await?;
    Ok(answer)
}

pub async fn unlink(
    api: &KaneoClient,
    ticket: &str,
    kind: &str,
    other: &str,
) -> Result<String, OpError> {
    let kind = workflow::relation_kind(kind)?;
    let (from, to) = pair(api, ticket, other).await?;
    let relations = api.relations(&from.task.id).await?;
    let relation =
        find_relation(&relations, kind, &from.task.id, &to.task.id).ok_or_else(|| {
            OpError::NotFound(format!(
                "no {kind} link from {} to {}",
                from.ticket, to.ticket
            ))
        })?;
    api.delete_relation(&relation.id).await?;
    Ok(render::ok(&format!("{} {kind} {}", from.ticket, to.ticket)))
}

/// Any subset of a card's fields. Dates and the assignee take `none` to clear.
#[derive(Default)]
pub struct CardEdit<'a> {
    pub title: Option<&'a str>,
    pub description: Option<&'a str>,
    pub priority: Option<&'a str>,
    pub start: Option<&'a str>,
    pub due: Option<&'a str>,
    /// A workspace member's name.
    pub assignee: Option<&'a str>,
}

impl CardEdit<'_> {
    fn is_empty(&self) -> bool {
        [
            self.title,
            self.description,
            self.priority,
            self.start,
            self.due,
            self.assignee,
        ]
        .iter()
        .all(Option::is_none)
    }
}

/// A member by name or id. Two members with the same name are refused rather
/// than guessed between.
fn member_id(members: &[Member], name: &str) -> Result<String, OpError> {
    let found: Vec<&Member> = members
        .iter()
        .filter(|m| m.name.eq_ignore_ascii_case(name) || m.id == name)
        .collect();
    match found.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => {
            let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
            Err(OpError::NotFound(format!(
                "no member {name:?}; there are: {}",
                names.join(", ")
            )))
        }
        _ => Err(OpError::NotFound(format!(
            "{} members are named {name:?}; pass the member id",
            found.len()
        ))),
    }
}

/// An edit with every value checked and resolved, before any write.
/// `Some(None)` clears the field.
struct Resolved {
    title: Option<String>,
    description: Option<String>,
    priority: Option<&'static str>,
    start: Option<Option<String>>,
    due: Option<Option<String>>,
    user: Option<Option<String>>,
}

fn resolve_dates(
    field: &'static str,
    raw: Option<&str>,
) -> Result<Option<Option<String>>, RuleError> {
    clearable(raw)
        .map(|v| v.map(|d| workflow::date(field, d)).transpose())
        .transpose()
}

/// The whole card for Kaneo's replacing PUT, which only a start-date edit
/// needs (there is no start-date route). It still sends the status and
/// position read a moment ago, so a move landing in between is undone: a
/// narrow race, declared rather than closed, since Kaneo offers no
/// compare-and-set. The description is left out unless edited, which Kaneo
/// reads as "keep it".
fn task_update_body(task: &Task, r: &Resolved) -> Result<Value, OpError> {
    let position = task
        .position
        .ok_or_else(|| OpError::NotFound(format!("card {} has no position", task.id)))?;
    let priority = match r.priority {
        Some(p) => p,
        None => workflow::priority(task.priority.as_deref().unwrap_or("no-priority"))?,
    };
    let mut body = Map::new();
    body.insert(
        "title".into(),
        json!(r.title.as_deref().unwrap_or(&task.title)),
    );
    body.insert("priority".into(), json!(priority));
    body.insert("status".into(), json!(task.status));
    body.insert("projectId".into(), json!(task.project_id));
    body.insert("position".into(), json!(position));
    if let Some(description) = &r.description {
        body.insert("description".into(), json!(description));
    }
    let fields = [
        ("startDate", &r.start, &task.start_date),
        ("dueDate", &r.due, &task.due_date),
        ("userId", &r.user, &task.user_id),
    ];
    for (key, edited, current) in fields {
        if let Some(value) = edited.as_ref().unwrap_or(current) {
            body.insert(key.into(), json!(value));
        }
    }
    Ok(Value::Object(body))
}

/// One single-field write per edited field, in a fixed order. A failure part
/// way names the fields already applied, since these writes are not atomic.
async fn apply_fields(api: &KaneoClient, task_id: &str, r: &Resolved) -> Result<(), OpError> {
    let mut writes: Vec<(&str, &str, Value)> = Vec::new();
    if let Some(title) = &r.title {
        writes.push(("title", "title", json!({ "title": title })));
    }
    if let Some(description) = &r.description {
        writes.push((
            "description",
            "description",
            json!({ "description": description }),
        ));
    }
    if let Some(priority) = r.priority {
        writes.push(("priority", "priority", json!({ "priority": priority })));
    }
    if let Some(due) = &r.due {
        // No dueDate clears it.
        let body = due.as_ref().map_or(json!({}), |d| json!({ "dueDate": d }));
        writes.push(("due", "due-date", body));
    }
    if let Some(user) = &r.user {
        writes.push(("assignee", "assignee", json!({ "userId": user })));
    }
    let mut applied: Vec<&str> = Vec::new();
    for (field, route, body) in writes {
        if let Err(e) = api.update_task_field(route, task_id, body).await {
            if applied.is_empty() {
                return Err(e.into());
            }
            return Err(OpError::Partial {
                applied: applied.join(", "),
                source: Box::new(e.into()),
            });
        }
        applied.push(field);
    }
    Ok(())
}

pub async fn edit_card(
    api: &KaneoClient,
    raw: &str,
    edit: &CardEdit<'_>,
) -> Result<String, OpError> {
    if edit.is_empty() {
        return Err(RuleError::EmptyEdit.into());
    }
    let mut r = Resolved {
        title: edit
            .title
            .map(|t| workflow::required("title", t).map(str::to_owned))
            .transpose()?,
        description: edit.description.map(|d| d.trim().to_owned()),
        priority: edit.priority.map(workflow::priority).transpose()?,
        start: resolve_dates("start", edit.start)?,
        due: resolve_dates("due", edit.due)?,
        user: None,
    };
    let Card { ticket, task } = card(api, raw).await?;
    let start = r
        .start
        .as_ref()
        .map_or(task.start_date.as_deref(), Option::as_deref);
    let due = r
        .due
        .as_ref()
        .map_or(task.due_date.as_deref(), Option::as_deref);
    workflow::check_date_order(start, due)?;
    r.user = match clearable(edit.assignee) {
        Some(Some(name)) => Some(Some(member_id(
            &api.members(workspace_of(&task)?).await?,
            name,
        )?)),
        Some(None) => Some(None),
        None => None,
    };
    if r.start.is_some() {
        let body = task_update_body(&task, &r)?;
        api.update_task(&task.id, body).await?;
    } else {
        apply_fields(api, &task.id, &r).await?;
    }
    Ok(render::ok(&ticket.to_string()))
}

/// Any column of the card's project, or a column of another project of the
/// same workspace. The work-in-progress and the final column stay with the
/// lifecycle tools.
/// A move to another project answers the card's new ticket, since Kaneo
/// renumbers it there.
pub async fn move_card(
    api: &KaneoClient,
    raw: &str,
    column: &str,
    project: Option<&str>,
) -> Result<String, OpError> {
    let wanted = workflow::required("column", column)?;
    let Card { ticket, task } = card(api, raw).await?;
    let elsewhere = project
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&ticket.slug));
    let here = api.project(&task.project_id).await?;
    let here_columns = api.columns(&here.id).await?;
    let here_active =
        workflow::active_column(&here_columns, here.description.as_deref().unwrap_or(""))?;
    workflow::check_leave(&task.status, here_active)?;
    let Some(slug) = elsewhere else {
        let to = workflow::resolve_column(&here_columns, wanted)?;
        workflow::check_enter(to, here_active)?;
        if to.slug != task.status {
            api.set_status(&task.id, &to.slug).await?;
        }
        return Ok(render::moved(&ticket.to_string(), &to.slug));
    };
    let (ws, dest) = project_by_slug(api, slug).await?;
    if ws != workspace_of(&task)? {
        return Err(RuleError::CrossWorkspace(ticket.to_string(), dest.slug).into());
    }
    let columns = api.columns(&dest.id).await?;
    let to = workflow::resolve_column(&columns, wanted)?;
    let active = workflow::active_column(&columns, dest.description.as_deref().unwrap_or(""))?;
    workflow::check_enter(to, active)?;
    let moved = api.move_task(&task.id, &dest.id, &to.slug).await?;
    let mut out = render::moved(&ticket_of(&dest.slug, moved.number, &moved.id), &to.slug);
    // Kaneo unassigns a card whose assignee cannot reach the new project.
    if task.user_id.is_some() && moved.user_id.is_none() {
        out.push_str(" · unassigned");
    }
    Ok(out)
}

/// The comment, checked to belong to the card, so an id from one card's
/// brief cannot reach another card's comment.
async fn own_comment(
    api: &KaneoClient,
    raw: &str,
    comment_id: &str,
) -> Result<crate::model::Comment, OpError> {
    let comment_id = workflow::required("comment", comment_id)?;
    let Card { ticket, task } = card(api, raw).await?;
    api.comments(&task.id)
        .await?
        .into_iter()
        .find(|c| c.id == comment_id)
        .ok_or_else(|| OpError::NotFound(format!("{ticket} has no comment {comment_id}")))
}

pub async fn edit_comment(
    api: &KaneoClient,
    raw: &str,
    comment_id: &str,
    text: &str,
) -> Result<String, OpError> {
    let text = workflow::required("text", text)?;
    let old = own_comment(api, raw, comment_id).await?;
    workflow::check_comment_edit(&old.content, text)?;
    Ok(render::ok(&api.update_comment(&old.id, text).await?))
}

pub async fn delete_comment(
    api: &KaneoClient,
    raw: &str,
    comment_id: &str,
) -> Result<String, OpError> {
    let old = own_comment(api, raw, comment_id).await?;
    workflow::check_comment_delete(&old.content)?;
    api.delete_comment(&old.id).await?;
    Ok(render::ok(&old.id))
}

pub struct ListQuery<'a> {
    pub project: &'a str,
    pub column: Option<&'a str>,
    pub newest_first: bool,
    pub page: u32,
    pub limit: u32,
}

pub async fn list_cards(api: &KaneoClient, q: &ListQuery<'_>) -> Result<String, OpError> {
    let (_, project) = project_by_slug(api, q.project).await?;
    let status = match q.column.map(str::trim).filter(|s| !s.is_empty()) {
        Some(wanted) => {
            let columns = api.columns(&project.id).await?;
            Some(workflow::resolve_column(&columns, wanted)?.slug.clone())
        }
        None => None,
    };
    let (page, limit) = (q.page.max(1), q.limit.clamp(1, MAX_LIST));
    let found = api
        .tasks_by_creation(&project.id, status.as_deref(), page, limit, q.newest_first)
        .await?;
    // Kaneo sorts before it pages but answers grouped by column, so the page
    // is put back in creation order here.
    let board = found.data;
    let mut tasks: Vec<Task> = board.columns.into_iter().flat_map(|c| c.tasks).collect();
    tasks.extend(board.planned_tasks);
    tasks.extend(board.archived_tasks);
    tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    if q.newest_first {
        tasks.reverse();
    }
    Ok(render::card_list(&CardList {
        slug: &project.slug,
        page,
        total_pages: found.pagination.total_pages,
        total: found.pagination.total,
        tasks: &tasks,
    }))
}

pub async fn card_activity(api: &KaneoClient, raw: &str) -> Result<String, OpError> {
    let Card { ticket, task } = card(api, raw).await?;
    let entries = api.activity(&task.id).await?;
    Ok(render::activity(&ticket.to_string(), &entries))
}

pub async fn project_info(api: &KaneoClient, slug: &str) -> Result<String, OpError> {
    let (_, project) = project_by_slug(api, slug).await?;
    let columns = api.columns(&project.id).await?;
    Ok(render::project_info(&project, &columns))
}

/// Replaces the description whole: it holds the board rules, so the answer
/// repeats what the server now reads from it.
pub async fn edit_project_description(
    api: &KaneoClient,
    slug: &str,
    description: &str,
) -> Result<String, OpError> {
    let description = workflow::required("description", description)?;
    let (_, found) = project_by_slug(api, slug).await?;
    let p = api.project(&found.id).await?;
    let columns = api.columns(&p.id).await?;
    workflow::check_description_rules(
        p.description.as_deref().unwrap_or(""),
        description,
        &columns,
    )?;
    // Kaneo's PUT requires both; guessing a value could flip the visibility.
    let missing =
        |field| OpError::NotFound(format!("project {} has no {field} to send back", p.slug));
    let body = json!({
        "name": p.name,
        "icon": p.icon.as_deref().ok_or_else(|| missing("icon"))?,
        "slug": p.slug,
        "description": description,
        "isPublic": p.is_public.ok_or_else(|| missing("isPublic"))?,
    });
    let id = api.update_project(&p.id, body).await?;
    Ok(format!(
        "{} · {}",
        render::ok(&id),
        render::project_rules(description, &columns)
    ))
}

/// Every project and workspace label the key sees: what the server can
/// resolve a slug or a label name to.
pub async fn projects(api: &KaneoClient) -> Result<String, OpError> {
    let mut out = Vec::new();
    for ws in api.workspaces().await? {
        let projects = api.projects(&ws.id).await?;
        let mut labels: Vec<String> = api
            .workspace_labels(&ws.id)
            .await?
            .into_iter()
            .filter(|l| l.task_id.is_none())
            .map(|l| l.name)
            .collect();
        labels.sort_unstable();
        labels.dedup();
        let name = if ws.name.is_empty() { ws.id } else { ws.name };
        out.push((name, projects, labels));
    }
    Ok(render::projects(&out))
}
