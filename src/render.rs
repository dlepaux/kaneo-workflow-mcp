//! Plain-text results. A tool result is read by a model, once, so it carries
//! one line per thing and no JSON scaffolding: the raw objects @kaneo/mcp
//! returned cost 2k to 3k tokens a call for a few useful fields.

use std::collections::HashMap;

use crate::model::{
    Activity, BoardColumn, Column, Comment, Label, Project, Relation, SearchHit, Task,
};

pub fn ok(id: &str) -> String {
    format!("ok {id}")
}

/// A write that moved a card to a column the rules chose, so the caller sees
/// where it went.
pub fn moved(id: &str, column: &str) -> String {
    format!("ok {id} → {column}")
}

/// Project id to slug, so a related card prints as `ABC-59` rather than an id.
pub type Slugs = HashMap<String, String>;

fn ticket_of(slugs: &Slugs, project_id: &str, number: u32) -> String {
    let slug = slugs.get(project_id).map_or("?", String::as_str);
    format!("{slug}-{number}")
}

fn day(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

/// ` · labels a, b`, sorted so every tool prints a card's labels alike, or
/// nothing for a card with none.
fn labels_of(names: &[&str]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    format!(" · labels {}", sorted.join(", "))
}

fn task_labels(t: &Task) -> String {
    let names: Vec<&str> = t.labels.iter().map(|l| l.name.as_str()).collect();
    labels_of(&names)
}

fn minute(iso: &str) -> String {
    iso.get(..16).unwrap_or(iso).replace('T', " ")
}

fn comment_line(c: &Comment, with_id: bool) -> String {
    let who = c.user.as_ref().map_or("?", |u| u.name.as_str());
    let id = if with_id {
        format!(" · {}", c.id)
    } else {
        String::new()
    };
    format!("[{} {who}{id}] {}", minute(&c.created_at), c.content.trim())
}

pub struct Brief<'a> {
    pub ticket: &'a str,
    pub task: &'a Task,
    pub labels: &'a [Label],
    pub relations: &'a [Relation],
    pub comments: &'a [Comment],
    pub slugs: &'a Slugs,
    pub recent: usize,
    /// Comment ids, for edit_comment and delete_comment. Off by default: a
    /// read rarely needs them.
    pub ids: bool,
}

/// The card in one read: comments are the rulings, so every DECISION
/// is kept however old, then the newest other comments, then the description.
pub fn card_brief(b: &Brief<'_>) -> String {
    let t = b.task;
    let mut out = format!(
        "{} {} · {} · {}",
        b.ticket,
        t.title,
        t.status,
        t.priority.as_deref().unwrap_or("no-priority")
    );
    if let Some(start) = &t.start_date {
        out.push_str(&format!(" · start {}", day(start)));
    }
    if let Some(due) = &t.due_date {
        out.push_str(&format!(" · due {}", day(due)));
    }
    out.push_str(&format!("\nid {}", t.id));
    if let Some(who) = &t.assignee_name {
        out.push_str(&format!(" · assignee {who}"));
    }
    let names: Vec<&str> = b.labels.iter().map(|l| l.name.as_str()).collect();
    out.push_str(&labels_of(&names));

    if !b.relations.is_empty() {
        out.push_str("\n\n## Relations");
        for r in b.relations {
            out.push('\n');
            out.push_str(&relation_line(r, &t.id, b.slugs));
        }
    }

    let (mut decisions, mut others): (Vec<&Comment>, Vec<&Comment>) = b
        .comments
        .iter()
        .partition(|c| crate::workflow::is_decision(&c.content));
    // Kaneo sends oldest first today; sorting keeps "newest" true if it stops.
    // ISO 8601 timestamps in one zone sort correctly as text.
    decisions.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    others.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    if !decisions.is_empty() {
        out.push_str("\n\n## Decisions (UTC)");
        for c in decisions {
            out.push('\n');
            out.push_str(&comment_line(c, b.ids));
        }
    }
    if !others.is_empty() {
        let shown = others.len().min(b.recent);
        let skipped = others.len() - shown;
        out.push_str(&format!(
            "\n\n## Comments (newest {shown} of {}, UTC)",
            others.len()
        ));
        if skipped > 0 {
            out.push_str(&format!("\n… {skipped} older not shown"));
        }
        for c in &others[skipped..] {
            out.push('\n');
            out.push_str(&comment_line(c, b.ids));
        }
    }

    let description = t.description.as_deref().unwrap_or("").trim();
    if !description.is_empty() {
        out.push_str("\n\n## Description\n");
        out.push_str(description);
    }
    out
}

/// Reads from this card's side: `blocks ABC-59` or `blocked by ABC-57`.
fn relation_line(r: &Relation, me: &str, slugs: &Slugs) -> String {
    let outgoing = r.source_task.id == me;
    let other = if outgoing {
        &r.target_task
    } else {
        &r.source_task
    };
    let verb = match (r.relation_type.as_str(), outgoing) {
        ("blocks", true) => "blocks",
        ("blocks", false) => "blocked by",
        ("subtask", true) => "parent of",
        ("subtask", false) => "subtask of",
        (other_kind, _) => other_kind,
    };
    format!(
        "{verb} {} {} ({})",
        ticket_of(slugs, &other.project_id, other.number),
        other.title,
        other.status
    )
}

/// The work-in-progress column of a board and the cap it states.
#[derive(Clone, Copy)]
pub struct Wip<'a> {
    pub slug: &'a str,
    pub name: &'a str,
    pub cap: Option<u32>,
}

/// Open columns list their cards; the final column and, unless asked, Backlog
/// only give a count, since an agent reads them far less than it pays for them.
pub fn board_digest(
    slug: &str,
    name: &str,
    columns: &[BoardColumn],
    wip: Option<Wip<'_>>,
    with_backlog: bool,
) -> String {
    let mut out = format!("{slug} {name}");
    if let Some(Wip {
        slug: active,
        name: active_name,
        cap: Some(cap),
    }) = wip
    {
        let held = columns
            .iter()
            .find(|c| c.slug == active)
            .map_or(0, |c| c.tasks.len());
        out.push_str(&format!(" · {active_name} {held}/{cap}"));
    }
    for c in columns {
        let listed = !c.is_final && (with_backlog || c.slug != crate::workflow::BACKLOG);
        out.push_str(&format!("\n\n## {} ({})", c.name, c.tasks.len()));
        if !listed {
            continue;
        }
        for t in &c.tasks {
            out.push_str(&format!(
                "\n{slug}-{} {} {}",
                t.number,
                t.priority.as_deref().unwrap_or("-"),
                t.title
            ));
            if let Some(due) = &t.due_date {
                out.push_str(&format!(" · due {}", day(due)));
            }
            out.push_str(&task_labels(t));
        }
    }
    out
}

/// Label names by card id.
pub type CardLabels = HashMap<String, Vec<String>>;

pub fn search_hits(hits: &[SearchHit], labels: &CardLabels) -> String {
    if hits.is_empty() {
        return "no match".to_owned();
    }
    let lines: Vec<String> = hits
        .iter()
        .map(|h| {
            let names: Vec<&str> =
                h.id.as_ref()
                    .and_then(|id| labels.get(id))
                    .map(|names| names.iter().map(String::as_str).collect())
                    .unwrap_or_default();
            format!(
                "{}-{} {} {} {}{}",
                h.project_slug.as_deref().unwrap_or("?"),
                h.task_number.map_or("?".to_owned(), |n| n.to_string()),
                h.status.as_deref().unwrap_or("-"),
                h.priority.as_deref().unwrap_or("-"),
                h.title,
                labels_of(&names)
            )
        })
        .collect();
    lines.join("\n")
}

pub struct CardList<'a> {
    pub slug: &'a str,
    pub page: u32,
    pub total_pages: u32,
    pub total: Option<u32>,
    pub tasks: &'a [Task],
}

/// One line per card with its creation day, so a review can page by date.
pub fn card_list(l: &CardList<'_>) -> String {
    let total = l.total.map_or(String::new(), |n| format!(" · {n} cards"));
    let mut out = format!("{} page {}/{}{total}", l.slug, l.page, l.total_pages.max(1));
    for t in l.tasks {
        out.push_str(&format!(
            "\n{}-{} {} {} {}",
            l.slug,
            t.number,
            t.status,
            t.priority.as_deref().unwrap_or("-"),
            t.title
        ));
        if let Some(created) = &t.created_at {
            out.push_str(&format!(" · created {}", day(created)));
        }
        if let Some(due) = &t.due_date {
            out.push_str(&format!(" · due {}", day(due)));
        }
        out.push_str(&task_labels(t));
    }
    if l.tasks.is_empty() {
        out.push_str("\nno cards");
    }
    out
}

fn event_field<'a>(e: &'a Activity, key: &str) -> Option<&'a str> {
    e.event_data.as_ref()?.get(key)?.as_str()
}

/// A card's history without its comments (card_brief has those), oldest
/// first: status changes read `sprint → doing`, the rest name their kind.
pub fn activity(ticket: &str, entries: &[Activity]) -> String {
    let mut events: Vec<&Activity> = entries.iter().filter(|e| e.kind != "comment").collect();
    events.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut out = format!("{ticket} activity (UTC)");
    for e in events {
        let what = match (
            e.kind.as_str(),
            event_field(e, "oldStatus"),
            event_field(e, "newStatus"),
        ) {
            ("status_changed", Some(old), Some(new)) => format!("status {old} → {new}"),
            ("moved", _, _) => match event_field(e, "toProjectName") {
                Some(to) => format!("moved to {to}"),
                None => "moved".to_owned(),
            },
            (kind, _, _) => kind.replace('_', " "),
        };
        out.push_str(&format!("\n{} {what}", minute(&e.created_at)));
    }
    out
}

/// The project as an agent reads its rules: columns, the stated cap and intake
/// column, then the description whole.
pub fn project_info(p: &Project, columns: &[Column]) -> String {
    let description = p.description.as_deref().unwrap_or("");
    let names: Vec<String> = columns
        .iter()
        .map(|c| {
            if c.is_final {
                format!("{} (final)", c.slug)
            } else {
                c.slug.clone()
            }
        })
        .collect();
    let mut out = format!(
        "{} {}\nid {}\ncolumns {}",
        p.slug,
        p.name,
        p.id,
        names.join(", ")
    );
    out.push_str(&format!("\n{}", project_rules(description, columns)));
    let description = description.trim();
    if !description.is_empty() {
        out.push_str("\n\n## Description\n");
        out.push_str(description);
    }
    out
}

/// What the server reads from a description, so an edit that breaks a rule
/// phrase shows in its answer.
pub fn project_rules(description: &str, columns: &[Column]) -> String {
    let active = crate::workflow::active_column(columns, description);
    let cap = match crate::workflow::cap_rule(
        columns,
        description,
        active.as_ref().ok().copied().flatten(),
    ) {
        Ok(Some(n)) => format!("cap {n}"),
        Ok(None) => "no cap stated".to_owned(),
        Err(e) => format!("cap: {e}"),
    };
    let active = match active {
        Ok(Some(c)) => format!("work in progress in {}", c.slug),
        Ok(None) => "no work-in-progress column".to_owned(),
        Err(e) => format!("work in progress: {e}"),
    };
    let intake = crate::workflow::default_column(description)
        .map_or("no intake column stated".to_owned(), |c| {
            format!("new cards go to {c}")
        });
    format!("{active} · {cap} · {intake}")
}

/// Every project the key sees, one line each, with each workspace's labels.
pub fn projects(workspaces: &[(String, Vec<Project>, Vec<String>)]) -> String {
    let mut parts = Vec::new();
    for (name, projects, labels) in workspaces {
        let mut out = format!("## {name}");
        for p in projects {
            out.push_str(&format!("\n{} {}", p.slug, p.name));
        }
        if !labels.is_empty() {
            out.push_str(&format!("\nlabels {}", labels.join(", ")));
        }
        parts.push(out);
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, RelatedTask};

    fn task() -> Task {
        Task {
            id: "t58".into(),
            title: "Lean server".into(),
            number: 58,
            status: "doing".into(),
            priority: Some("high".into()),
            project_id: "p-web".into(),
            workspace_id: Some("w".into()),
            description: Some("## Why\n\nTokens.".into()),
            due_date: Some("2026-11-01T22:59:00.000Z".into()),
            ..Task::default()
        }
    }

    fn comment(id: &str, at: &str, content: &str) -> Comment {
        Comment {
            id: id.into(),
            content: content.into(),
            created_at: at.into(),
            user: Some(Author { name: "Ana".into() }),
        }
    }

    fn related(id: &str, project: &str, number: u32) -> RelatedTask {
        RelatedTask {
            id: id.into(),
            title: format!("card {number}"),
            number,
            status: "sprint".into(),
            project_id: project.into(),
            is_completed: false,
        }
    }

    #[test]
    fn brief_keeps_every_decision_and_only_the_newest_comments() {
        let comments = vec![
            comment(
                "c1",
                "2026-10-20T08:00:00.000Z",
                "DECISION 2026-10-20 — old ruling",
            ),
            // Out of order on purpose: the brief sorts, it does not trust Kaneo's order.
            comment("c4", "2026-10-23T08:00:00.000Z", "third"),
            comment("c2", "2026-10-21T08:00:00.000Z", "first"),
            comment("c3", "2026-10-22T08:00:00.000Z", "second"),
        ];
        let relations = vec![
            Relation {
                id: "r".into(),
                relation_type: "blocks".into(),
                source_task: related("t58", "p-web", 58),
                target_task: related("t59", "p-web", 59),
            },
            Relation {
                id: "r".into(),
                relation_type: "related".into(),
                source_task: related("s5", "p-spr", 5),
                target_task: related("t58", "p-web", 58),
            },
        ];
        let labels = vec![Label {
            id: "l".into(),
            name: "Dev".into(),
            task_id: Some("t58".into()),
        }];
        let slugs: Slugs = [("p-web", "WEB"), ("p-spr", "SPR")]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let out = card_brief(&Brief {
            ticket: "WEB-58",
            task: &task(),
            labels: &labels,
            relations: &relations,
            comments: &comments,
            slugs: &slugs,
            recent: 2,
            ids: false,
        });
        assert_eq!(
            out,
            "WEB-58 Lean server · doing · high · due 2026-11-01\n\
             id t58 · labels Dev\n\n\
             ## Relations\n\
             blocks WEB-59 card 59 (sprint)\n\
             related SPR-5 card 5 (sprint)\n\n\
             ## Decisions (UTC)\n\
             [2026-10-20 08:00 Ana] DECISION 2026-10-20 — old ruling\n\n\
             ## Comments (newest 2 of 3, UTC)\n\
             … 1 older not shown\n\
             [2026-10-22 08:00 Ana] second\n\
             [2026-10-23 08:00 Ana] third\n\n\
             ## Description\n\
             ## Why\n\nTokens."
        );
    }

    #[test]
    fn brief_of_a_bare_card_is_two_lines() {
        let mut t = task();
        t.description = None;
        t.due_date = None;
        let out = card_brief(&Brief {
            ticket: "WEB-58",
            task: &t,
            labels: &[],
            relations: &[],
            comments: &[],
            slugs: &Slugs::new(),
            recent: 5,
            ids: false,
        });
        assert_eq!(out, "WEB-58 Lean server · doing · high\nid t58");
    }

    #[test]
    fn relations_read_from_this_cards_side() {
        let slugs = Slugs::new();
        let r = Relation {
            id: "r".into(),
            relation_type: "subtask".into(),
            source_task: related("epic", "p", 1),
            target_task: related("t58", "p", 58),
        };
        assert_eq!(
            relation_line(&r, "t58", &slugs),
            "subtask of ?-1 card 1 (sprint)"
        );
        let r = Relation {
            id: "r".into(),
            relation_type: "blocks".into(),
            source_task: related("t57", "p", 57),
            target_task: related("t58", "p", 58),
        };
        assert_eq!(
            relation_line(&r, "t58", &slugs),
            "blocked by ?-57 card 57 (sprint)"
        );
    }

    fn labelled(names: &[&str]) -> Task {
        Task {
            labels: names
                .iter()
                .map(|n| crate::model::TaskLabel { name: (*n).into() })
                .collect(),
            ..task()
        }
    }

    #[test]
    fn brief_shows_the_start_date_before_the_due_date() {
        let t = Task {
            start_date: Some("2026-11-04T21:32:00.000Z".into()),
            description: None,
            ..task()
        };
        let slugs = Slugs::new();
        let b = Brief {
            ticket: "WEB-58",
            task: &t,
            labels: &[],
            relations: &[],
            comments: &[],
            slugs: &slugs,
            recent: 5,
            ids: false,
        };
        assert_eq!(
            card_brief(&b),
            "WEB-58 Lean server · doing · high · start 2026-11-04 · due 2026-11-01\nid t58"
        );
    }

    #[test]
    fn card_list_lines_carry_labels() {
        let tasks = [labelled(&["operator-decision"]), task()];
        let out = card_list(&CardList {
            slug: "WEB",
            page: 1,
            total_pages: 1,
            total: Some(2),
            tasks: &tasks,
        });
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines[1].ends_with("· due 2026-11-01 · labels operator-decision"),
            "{out}"
        );
        assert!(lines[2].ends_with("· due 2026-11-01"), "{out}");
    }

    #[test]
    fn digest_lists_open_columns_and_counts_the_rest() {
        let col = |slug: &str, name: &str, is_final: bool, tasks: Vec<Task>| BoardColumn {
            slug: slug.into(),
            name: name.into(),
            is_final,
            tasks,
        };
        let columns = vec![
            col("backlog", "Backlog", false, vec![task()]),
            col(
                "doing",
                "Doing",
                false,
                vec![labelled(&["security", "Dev"])],
            ),
            col("done", "Done", true, vec![task(), task()]),
        ];
        let wip = |cap| {
            Some(Wip {
                slug: "doing",
                name: "Doing",
                cap,
            })
        };
        assert_eq!(
            board_digest("WEB", "Web", &columns, wip(Some(3)), false),
            "WEB Web · Doing 1/3\n\n## Backlog (1)\n\n## Doing (1)\nWEB-58 high Lean server · due 2026-11-01 · labels Dev, security\n\n## Done (2)"
        );
        // No cap stated: the header carries no count.
        assert!(board_digest("WEB", "Web", &columns, wip(None), false).starts_with("WEB Web\n\n"));
        let with_backlog = board_digest("WEB", "Web", &columns, None, true);
        assert!(with_backlog.starts_with("WEB Web\n\n## Backlog (1)\nWEB-58 high"));
    }

    #[test]
    fn search_hits_are_one_line_each() {
        let hit = |id: &str| SearchHit {
            id: Some(id.into()),
            title: "Lean server".into(),
            project_slug: Some("WEB".into()),
            task_number: Some(58),
            status: Some("doing".into()),
            priority: Some("high".into()),
        };
        let labels = CardLabels::from([("t58".into(), vec!["security".into(), "Dev".into()])]);
        assert_eq!(
            search_hits(&[hit("t58")], &labels),
            "WEB-58 doing high Lean server · labels Dev, security"
        );
        assert_eq!(
            search_hits(&[hit("t9")], &labels),
            "WEB-58 doing high Lean server"
        );
        assert_eq!(search_hits(&[], &labels), "no match");
    }

    #[test]
    fn brief_shows_the_assignee_and_comment_ids_when_asked() {
        let mut t = task();
        t.description = None;
        t.assignee_name = Some("Ana".into());
        let comments = vec![comment("c-9", "2026-10-27T09:00:00.000Z", "hello")];
        let out = card_brief(&Brief {
            ticket: "WEB-58",
            task: &t,
            labels: &[],
            relations: &[],
            comments: &comments,
            slugs: &Slugs::new(),
            recent: 5,
            ids: true,
        });
        assert!(out.contains("\nid t58 · assignee Ana\n"), "{out}");
        assert!(out.ends_with("[2026-10-27 09:00 Ana · c-9] hello"), "{out}");
    }

    #[test]
    fn card_list_of_an_empty_project_says_so() {
        let out = card_list(&CardList {
            slug: "WEB",
            page: 1,
            total_pages: 0,
            total: Some(0),
            tasks: &[],
        });
        assert_eq!(out, "WEB page 1/1 · 0 cards\nno cards");
    }
}
