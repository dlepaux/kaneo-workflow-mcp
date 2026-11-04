//! The board rules as pure functions. Keeping them here rather than in prose
//! is the point of this server: a session cannot skip a rule a tool enforces.
//!
//! A project states its rules as lines of its description, all optional:
//! `Work in progress: In Progress`, `Column limit: In Progress holds at most 3
//! cards` and `New cards go to: To Do`. Without them the server falls back to
//! what a board's columns already say, so a default Kaneo board works as it is.

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};

use crate::model::{Column, Relation};

/// The cap line is `Column limit: <column> holds at most <n> cards`, and the
/// column it names must be the work-in-progress column: a cap on any other
/// column is one the server would not apply, so it is refused, not ignored.
const CAP_PREFIX: &str = "Column limit: ";
const CAP_MIDDLE: &str = " holds at most ";
/// The line naming the column start_card moves cards into.
const ACTIVE_PHRASE: &str = "Work in progress: ";
/// Without an active line: the slug a board named Doing has, then the slug of
/// Kaneo's default In Progress column.
const ACTIVE_FALLBACKS: [&str; 2] = ["doing", "in-progress"];
/// The line naming the column new cards go to, like `New cards go to: Backlog`.
const DEFAULT_COLUMN_PHRASE: &str = "New cards go to: ";
pub const SPRINT: &str = "sprint";
pub const BACKLOG: &str = "backlog";
pub const OPERATOR_DECISION: &str = "operator-decision";
const DECISION_PREFIX: &str = "DECISION ";
pub const PRIORITIES: [&str; 5] = ["no-priority", "low", "medium", "high", "urgent"];
pub const RELATION_KINDS: [&str; 3] = ["blocks", "related", "subtask"];
/// The value that clears an optional field in an edit.
pub const NONE: &str = "none";

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    #[error("{0} is required and cannot be blank")]
    Blank(&'static str),
    #[error("ticket {0:?} is not of the form ABC-12")]
    BadTicket(String),
    #[error("{column} is at its cap of {cap} ({held}). Finish or pause one of them first.")]
    OverCap {
        column: String,
        cap: u32,
        held: String,
    },
    #[error("project has no final column to close into")]
    NoFinalColumn,
    #[error("project has no {0:?} column")]
    NoColumn(String),
    #[error(
        "project has no work-in-progress column: name one in its description with \"Work in progress: <column>\""
    )]
    NoActiveColumn,
    #[error("the work-in-progress column cannot be the final column {0:?}")]
    FinalActive(String),
    #[error(
        "a pause cannot fall back to the intake column {0:?}: it is the work-in-progress or the final column"
    )]
    PauseIntake(String),
    #[error("a comment starting with {0:?} goes through its own tool ({1})")]
    Reserved(&'static str, &'static str),
    #[error("project has no column {wanted:?}; there are: {have}")]
    UnknownColumn { wanted: String, have: String },
    #[error("cards enter {0} only through {1}")]
    EnterOnly(String, &'static str),
    #[error("cards leave {0} only through {1}")]
    LeaveOnly(String, &'static str),
    #[error("{ticket} is in {column}, not {active}: only a card in progress is paused")]
    NotInProgress {
        ticket: String,
        column: String,
        active: String,
    },
    #[error("the line {0:?} does not read as \"Column limit: <column> holds at most <n> cards\"")]
    BadCapLine(String),
    #[error(
        "the Column limit line caps {named}, but the work-in-progress column is {active}: only that column has a cap"
    )]
    CapElsewhere { named: String, active: String },
    #[error(
        "{0} is changed in Kaneo itself, not through this server: an agent cannot loosen the rules it works under"
    )]
    Loosened(&'static str),
    #[error("project has no column a new card can go to; pass column")]
    NoDefaultColumn,
    #[error("priority {0:?} is not one of no-priority, low, medium, high, urgent")]
    BadPriority(String),
    #[error("{0} {1:?} is not a date (2026-11-01) or an RFC 3339 time")]
    BadDate(&'static str, String),
    #[error("relation kind {0:?} is not one of blocks, related, subtask")]
    BadRelationKind(String),
    #[error("a card cannot be linked to itself")]
    SelfLink,
    #[error("{0} and {1} are in different workspaces")]
    CrossWorkspace(String, String),
    #[error("{other} already has a {kind} link to {ticket}; unlink it first")]
    ReverseLink {
        kind: &'static str,
        ticket: String,
        other: String,
    },
    #[error("start {0} is after due {1}")]
    DateOrder(String, String),
    #[error("a {0} comment is a record and is not deleted; edit it instead")]
    RecordComment(&'static str),
    #[error("the new description drops the {0:?} line the old one stated")]
    DroppedRule(&'static str),
    #[error("an edit keeps the comment's kind: {0} cannot become {1}")]
    KindChange(&'static str, &'static str),
    #[error("nothing to change")]
    EmptyEdit,
}

/// A ticket such as `ABC-12`, split into project slug and card number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub slug: String,
    pub number: u32,
}

impl std::fmt::Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.slug, self.number)
    }
}

pub fn parse_ticket(raw: &str) -> Result<Ticket, RuleError> {
    let bad = || RuleError::BadTicket(raw.to_owned());
    let (slug, number) = raw.trim().rsplit_once('-').ok_or_else(bad)?;
    let slug = slug.to_ascii_uppercase();
    let slug_ok = slug.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && slug.chars().all(|c| c.is_ascii_alphanumeric());
    let number: u32 = number.parse().map_err(|_| bad())?;
    if !slug_ok || number == 0 {
        return Err(bad());
    }
    Ok(Ticket { slug, number })
}

pub fn required<'a>(field: &'static str, value: &'a str) -> Result<&'a str, RuleError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(RuleError::Blank(field));
    }
    Ok(value)
}

/// The rest of the line after `phrase`, trimmed of a closing full stop.
fn stated<'a>(description: &'a str, phrase: &str) -> Option<&'a str> {
    let rest = &description[description.find(phrase)? + phrase.len()..];
    let line = rest.lines().next()?.trim().trim_end_matches('.').trim();
    (!line.is_empty()).then_some(line)
}

/// The column and the number a cap line states, or `None` without a cap line.
/// A line that is there but does not read as a cap is an error rather than no
/// cap, so a typo cannot lift the limit unnoticed.
fn cap_line(description: &str) -> Result<Option<(&str, u32)>, RuleError> {
    let Some(line) = stated(description, CAP_PREFIX) else {
        return Ok(None);
    };
    let bad = || RuleError::BadCapLine(format!("{CAP_PREFIX}{line}"));
    let (column, rest) = line.split_once(CAP_MIDDLE).ok_or_else(bad)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let unit = rest[digits.len()..]
        .strip_prefix(' ')
        .and_then(|r| r.split_whitespace().next())
        .map(|w| w.trim_end_matches(|c: char| c.is_ascii_punctuation()));
    if !matches!(unit, Some("card" | "cards")) {
        return Err(bad());
    }
    let cap = digits.parse().map_err(|_| bad())?;
    Ok(Some((column.trim(), cap)))
}

/// The cap on the work-in-progress column, if the description states one.
pub fn cap_rule(
    columns: &[Column],
    description: &str,
    active: Option<&Column>,
) -> Result<Option<u32>, RuleError> {
    let Some((named, cap)) = cap_line(description)? else {
        return Ok(None);
    };
    let column = resolve_column(columns, named)?;
    match active {
        Some(active) if active.slug == column.slug => Ok(Some(cap)),
        _ => Err(RuleError::CapElsewhere {
            named: column.name.clone(),
            active: active.map_or_else(|| "none".to_owned(), |a| a.name.clone()),
        }),
    }
}

/// `others` are the tickets already in the column, not counting the card being started.
pub fn check_cap(column: &str, cap: Option<u32>, others: &[String]) -> Result<(), RuleError> {
    match cap {
        Some(cap) if others.len() >= cap as usize => Err(RuleError::OverCap {
            column: column.to_owned(),
            cap,
            held: others.join(", "),
        }),
        _ => Ok(()),
    }
}

/// The column start_card moves cards into: the one the description names, or
/// else a `doing` or `in-progress` column. `None` when the board has neither,
/// which only start_card refuses.
pub fn active_column<'a>(
    columns: &'a [Column],
    description: &str,
) -> Result<Option<&'a Column>, RuleError> {
    if let Some(wanted) = stated(description, ACTIVE_PHRASE) {
        let column = resolve_column(columns, wanted)?;
        if column.is_final {
            return Err(RuleError::FinalActive(column.slug.clone()));
        }
        return Ok(Some(column));
    }
    Ok(ACTIVE_FALLBACKS
        .iter()
        .find_map(|slug| columns.iter().find(|c| c.slug == *slug)))
}

/// The board rule for a pause: back to Sprint when the card is still linked to
/// an open sprint card, otherwise to Backlog. Only the other end of each link
/// counts, so a card on the sprints project does not match itself. Without a
/// sprints project the rule is always Backlog.
pub fn pause_destination(
    relations: &[Relation],
    task_id: &str,
    sprint_project_id: Option<&str>,
) -> &'static str {
    let Some(sprint_project_id) = sprint_project_id else {
        return BACKLOG;
    };
    let on_open_sprint = relations.iter().any(|r| {
        let other = if r.source_task.id == task_id {
            &r.target_task
        } else {
            &r.source_task
        };
        r.relation_type == "related" && other.project_id == sprint_project_id && !other.is_completed
    });
    if on_open_sprint { SPRINT } else { BACKLOG }
}

/// The first column, in the board's order, that is neither the
/// work-in-progress nor the final column: where unstarted work waits on a
/// board that names no column for it.
fn first_open<'a>(columns: &'a [Column], active: Option<&str>) -> Option<&'a Column> {
    columns
        .iter()
        .find(|c| !c.is_final && Some(c.slug.as_str()) != active)
}

/// Where a pause lands: the rule's column (Sprint or Backlog) when the board
/// has it, then Backlog (a Sprint pause on a board without Sprint), then the
/// intake column the description states, then the board's first open column.
/// Never the work-in-progress or the final column.
pub fn pause_column<'a>(
    columns: &'a [Column],
    rule: &str,
    description: &str,
    active: Option<&str>,
) -> Result<&'a Column, RuleError> {
    let open = |c: &&Column| !c.is_final && Some(c.slug.as_str()) != active;
    if let Some(c) = [rule, BACKLOG]
        .iter()
        .find_map(|slug| columns.iter().filter(open).find(|c| c.slug == *slug))
    {
        return Ok(c);
    }
    if let Some(intake) = default_column(description) {
        let column = resolve_column(columns, intake)?;
        if Some(column.slug.as_str()) == active || column.is_final {
            return Err(RuleError::PauseIntake(column.slug.clone()));
        }
        return Ok(column);
    }
    first_open(columns, active).ok_or_else(|| RuleError::NoColumn(rule.to_owned()))
}

/// Where a new card goes when the caller names no column: the intake column
/// the description states, or else the board's first open column.
pub fn intake_column<'a>(
    columns: &'a [Column],
    description: &str,
    active: Option<&str>,
) -> Result<&'a Column, RuleError> {
    match default_column(description) {
        Some(wanted) => resolve_column(columns, wanted),
        None => first_open(columns, active).ok_or(RuleError::NoDefaultColumn),
    }
}

/// Prefixes owned by a tool that applies a rule, and the tool to use.
const RESERVED: [(&str, &str); 4] = [
    ("START ", "start_card"),
    ("PAUSE ", "pause_card"),
    ("DONE ", "close_card"),
    (DECISION_PREFIX, "decide"),
];

pub fn plain_comment(text: &str) -> Result<(), RuleError> {
    let text = text.trim_start();
    match RESERVED.iter().find(|(prefix, _)| text.starts_with(prefix)) {
        Some((prefix, tool)) => Err(RuleError::Reserved(prefix.trim_end(), tool)),
        None => Ok(()),
    }
}

/// The intake column a project states in its description, if it states one.
pub fn default_column(description: &str) -> Option<&str> {
    stated(description, DEFAULT_COLUMN_PHRASE)
}

/// Letters and digits only, lowercased, so `To Do`, `to-do` and `TODO` meet.
fn column_key(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A column by slug, or by name as a project description states it. An
/// exact slug wins, so two columns that only differ in punctuation stay
/// reachable.
pub fn resolve_column<'a>(columns: &'a [Column], wanted: &str) -> Result<&'a Column, RuleError> {
    let wanted = wanted.trim();
    let key = column_key(wanted);
    columns
        .iter()
        .find(|c| c.slug == wanted)
        .or_else(|| {
            columns.iter().find(|c| {
                !key.is_empty() && (column_key(&c.slug) == key || column_key(&c.name) == key)
            })
        })
        .ok_or_else(|| RuleError::UnknownColumn {
            wanted: wanted.to_owned(),
            have: columns
                .iter()
                .map(|c| c.slug.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        })
}

/// The work-in-progress column belongs to the lifecycle tools, which leave a
/// record: a plain move cannot take a card out of it. A card can leave the
/// final column, since reopening finished work is a plain move.
pub fn check_leave(from: &str, active: Option<&Column>) -> Result<(), RuleError> {
    match active {
        Some(active) if active.slug == from => Err(RuleError::LeaveOnly(
            active.name.clone(),
            "pause_card and close_card",
        )),
        _ => Ok(()),
    }
}

/// A pause takes a card out of progress, so a card elsewhere has nothing to
/// pause: pausing it would post a PAUSE no START came before, and pull a
/// finished card back out of the final column.
pub fn check_in_progress(ticket: &str, status: &str, active: &Column) -> Result<(), RuleError> {
    if status == active.slug {
        return Ok(());
    }
    Err(RuleError::NotInProgress {
        ticket: ticket.to_owned(),
        column: status.to_owned(),
        active: active.slug.clone(),
    })
}

/// The work-in-progress and the final column belong to the lifecycle tools,
/// which apply the cap and require evidence: a plain move or a new card
/// cannot skip them.
pub fn check_enter(to: &Column, active: Option<&Column>) -> Result<(), RuleError> {
    if active.is_some_and(|a| a.slug == to.slug) {
        return Err(RuleError::EnterOnly(to.name.clone(), "start_card"));
    }
    if to.is_final {
        return Err(RuleError::EnterOnly(to.name.clone(), "close_card"));
    }
    Ok(())
}

pub fn priority(raw: &str) -> Result<&'static str, RuleError> {
    let wanted = raw.trim().to_ascii_lowercase();
    PRIORITIES
        .iter()
        .find(|p| **p == wanted)
        .copied()
        .ok_or_else(|| RuleError::BadPriority(raw.to_owned()))
}

pub fn relation_kind(raw: &str) -> Result<&'static str, RuleError> {
    let wanted = raw.trim().to_ascii_lowercase();
    RELATION_KINDS
        .iter()
        .find(|k| **k == wanted)
        .copied()
        .ok_or_else(|| RuleError::BadRelationKind(raw.to_owned()))
}

/// A bare date becomes midday UTC, which is the same calendar day from
/// UTC-11 to UTC+11, so the board shows the day that was meant wherever it
/// is read. A full time is kept, in UTC.
pub fn date(field: &'static str, raw: &str) -> Result<String, RuleError> {
    let raw = raw.trim();
    if let Ok(day) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(format!("{day}T12:00:00.000Z"));
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|t| t.to_utc().to_rfc3339_opts(SecondsFormat::Millis, true))
        .map_err(|_| RuleError::BadDate(field, raw.to_owned()))
}

/// The reserved prefix a comment starts with, or `plain`.
pub fn comment_kind(text: &str) -> &'static str {
    let text = text.trim_start();
    RESERVED
        .iter()
        .find(|(prefix, _)| text.starts_with(prefix))
        .map_or("plain", |(prefix, _)| prefix.trim_end())
}

/// Only a plain comment is deleted: START, PAUSE, DONE and DECISION are the
/// card's record, which an edit can correct but not erase.
pub fn check_comment_delete(text: &str) -> Result<(), RuleError> {
    match comment_kind(text) {
        "plain" => Ok(()),
        kind => Err(RuleError::RecordComment(kind)),
    }
}

/// Kaneo refuses a start after the due date, but only after a write began;
/// both are normalised UTC times, so they compare as text.
pub fn check_date_order(start: Option<&str>, due: Option<&str>) -> Result<(), RuleError> {
    match (start, due) {
        (Some(s), Some(d)) if s > d => Err(RuleError::DateOrder(s.to_owned(), d.to_owned())),
        _ => Ok(()),
    }
}

/// A description edit keeps every rule line the old text stated, its rules
/// must resolve on the board, and it may tighten them but not loosen them: the
/// work-in-progress column stays where it is and its cap does not rise. The
/// agents that edit a description work under its rules, so loosening them is
/// done in Kaneo itself. Rules the old text left broken are not held against
/// the edit that repairs them.
pub fn check_description_rules(old: &str, new: &str, columns: &[Column]) -> Result<(), RuleError> {
    for phrase in [CAP_PREFIX, ACTIVE_PHRASE, DEFAULT_COLUMN_PHRASE] {
        if stated(old, phrase).is_some() && stated(new, phrase).is_none() {
            return Err(RuleError::DroppedRule(phrase.trim_end()));
        }
    }
    let active = active_column(columns, new)?;
    let cap = cap_rule(columns, new, active)?;
    if let Some(intake) = default_column(new) {
        check_enter(resolve_column(columns, intake)?, active)?;
    }
    if let Ok(Some(was)) = active_column(columns, old) {
        if active.is_none_or(|a| a.slug != was.slug) {
            return Err(RuleError::Loosened("the work-in-progress column"));
        }
        if let Ok(Some(old_cap)) = cap_rule(columns, old, Some(was))
            && cap.is_none_or(|n| n > old_cap)
        {
            return Err(RuleError::Loosened("the cap"));
        }
    }
    Ok(())
}

/// An edit may fix a comment's text but not its kind: a plain comment cannot
/// become a DECISION after the fact, and a START stays a START.
pub fn check_comment_edit(old: &str, new: &str) -> Result<(), RuleError> {
    let (was, is) = (comment_kind(old), comment_kind(new));
    if was == is {
        Ok(())
    } else {
        Err(RuleError::KindChange(was, is))
    }
}

pub fn final_column(columns: &[Column]) -> Result<&str, RuleError> {
    columns
        .iter()
        .find(|c| c.is_final)
        .map(|c| c.slug.as_str())
        .ok_or(RuleError::NoFinalColumn)
}

pub fn is_decision(comment: &str) -> bool {
    comment.starts_with(DECISION_PREFIX)
}

/// The record comments carry their zone, so a board read from anywhere tells
/// the same time.
fn stamp(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%d %H:%MZ").to_string()
}

pub fn start_comment(now: DateTime<Utc>, session: &str, plan: &str) -> String {
    format!("START {} — {session} — {plan}", stamp(now))
}

pub fn pause_comment(now: DateTime<Utc>, session: &str, state: &str, next: &str) -> String {
    format!("PAUSE {} — {session} — {state} Next: {next}", stamp(now))
}

pub fn done_comment(now: DateTime<Utc>, session: &str, evidence: &str) -> String {
    format!("DONE {} — {session} — {evidence}", stamp(now))
}

/// The decision-of-record format: choice, reasoning, any concern overruled,
/// and the provenance Kaneo cannot record itself (every write is credited to
/// the key's owner).
pub struct Decision<'a> {
    pub choice: &'a str,
    pub reasoning: &'a str,
    pub provenance: &'a str,
    pub overruled: Option<&'a str>,
    pub covers: Option<&'a str>,
}

/// Dated by its UTC day.
pub fn decision_comment(now: DateTime<Utc>, d: &Decision<'_>) -> String {
    let mut text = format!(
        "{DECISION_PREFIX}{} — {}\n\nWhy: {}",
        now.format("%Y-%m-%d"),
        d.choice,
        d.reasoning
    );
    if let Some(overruled) = d.overruled {
        text.push_str(&format!("\nOverruled: {overruled}"));
    }
    if let Some(covers) = d.covers {
        text.push_str(&format!("\nCovers: {covers}"));
    }
    text.push_str(&format!("\nProvenance: {}", d.provenance));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RelatedTask;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 27, 10, 5, 0).unwrap()
    }

    fn rel(kind: &str, other_project: &str, completed: bool) -> Relation {
        rel_on(kind, other_project, completed, "p-web")
    }

    fn rel_on(kind: &str, other_project: &str, completed: bool, my_project: &str) -> Relation {
        let task = |id: &str, project: &str, done: bool| RelatedTask {
            id: id.into(),
            title: "t".into(),
            number: 1,
            status: "sprint".into(),
            project_id: project.into(),
            is_completed: done,
        };
        Relation {
            id: "r".into(),
            relation_type: kind.into(),
            source_task: task("s", other_project, completed),
            target_task: task("me", my_project, false),
        }
    }

    fn col(slug: &str, name: &str, is_final: bool) -> Column {
        Column {
            slug: slug.into(),
            name: name.into(),
            is_final,
        }
    }

    /// The columns a new Kaneo project starts with.
    fn kaneo_default() -> Vec<Column> {
        vec![
            col("to-do", "To Do", false),
            col("in-progress", "In Progress", false),
            col("in-review", "In Review", false),
            col("done", "Done", true),
        ]
    }

    fn sprint_board() -> Vec<Column> {
        vec![
            col("backlog", "Backlog", false),
            col("sprint", "Sprint", false),
            col("doing", "Doing", false),
            col("done", "Done", true),
        ]
    }

    #[test]
    fn tickets_parse_and_reject() {
        assert_eq!(
            parse_ticket(" web-58 ").unwrap(),
            Ticket {
                slug: "WEB".into(),
                number: 58
            }
        );
        for bad in ["WEB", "WEB-", "-5", "WEB-x", "W/B-5", "WEB-0", "1A-2"] {
            assert!(parse_ticket(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_cap_is_read_from_its_line_on_the_work_in_progress_column() {
        let board = sprint_board();
        let doing = board.iter().find(|c| c.slug == "doing");
        let cap = |desc| cap_rule(&board, desc, doing);
        let desc = "Web app.\n- Column limit: Doing holds at most 3 cards while two people work";
        assert_eq!(cap(desc), Ok(Some(3)));
        assert_eq!(
            cap("- Column limit: doing holds at most 2 cards."),
            Ok(Some(2))
        );
        assert_eq!(
            cap("- Column limit: Doing holds at most 1 card"),
            Ok(Some(1))
        );
        assert_eq!(cap("no cap stated"), Ok(None));
        let defaults = kaneo_default();
        let in_progress = defaults.iter().find(|c| c.slug == "in-progress");
        assert_eq!(
            cap_rule(
                &defaults,
                "Column limit: In Progress holds at most 2 cards",
                in_progress
            ),
            Ok(Some(2))
        );
    }

    #[test]
    fn a_cap_line_that_does_not_read_as_one_is_refused_not_ignored() {
        let board = sprint_board();
        let doing = board.iter().find(|c| c.slug == "doing");
        for line in [
            "Column limit: Doing holds at most many cards",
            "Column limit: Doing holds at most 3 tasks",
            "Column limit: Doing holds at most 3cards",
            "Column limit: Doing\nholds at most 3 cards",
        ] {
            assert!(
                matches!(cap_rule(&board, line, doing), Err(RuleError::BadCapLine(_))),
                "{line}"
            );
        }
    }

    #[test]
    fn a_cap_on_another_column_is_refused_not_applied_elsewhere() {
        let board = sprint_board();
        let doing = board.iter().find(|c| c.slug == "doing");
        assert_eq!(
            cap_rule(&board, "Column limit: Sprint holds at most 2 cards", doing),
            Err(RuleError::CapElsewhere {
                named: "Sprint".into(),
                active: "Doing".into()
            })
        );
        assert!(matches!(
            cap_rule(&board, "Column limit: Review holds at most 2 cards", doing),
            Err(RuleError::UnknownColumn { .. })
        ));
    }

    #[test]
    fn cap_blocks_only_at_or_over_the_limit() {
        let two = vec!["WEB-1".to_owned(), "WEB-2".to_owned()];
        assert!(check_cap("Doing", Some(3), &two).is_ok());
        assert!(check_cap("Doing", None, &two).is_ok());
        let err = check_cap("In Progress", Some(2), &two).unwrap_err();
        assert_eq!(
            err.to_string(),
            "In Progress is at its cap of 2 (WEB-1, WEB-2). Finish or pause one of them first."
        );
    }

    #[test]
    fn the_active_column_is_stated_or_found_by_its_usual_slug() {
        assert_eq!(
            active_column(&sprint_board(), "").unwrap().unwrap().slug,
            "doing"
        );
        assert_eq!(
            active_column(&kaneo_default(), "").unwrap().unwrap().slug,
            "in-progress"
        );
        let stated = "- Work in progress: In Review";
        assert_eq!(
            active_column(&kaneo_default(), stated)
                .unwrap()
                .unwrap()
                .slug,
            "in-review"
        );
        let plain = vec![col("open", "Open", false), col("closed", "Closed", true)];
        assert_eq!(active_column(&plain, ""), Ok(None));
    }

    #[test]
    fn a_stated_active_column_must_exist_and_not_be_final() {
        assert!(matches!(
            active_column(&kaneo_default(), "Work in progress: Building"),
            Err(RuleError::UnknownColumn { .. })
        ));
        assert_eq!(
            active_column(&kaneo_default(), "Work in progress: Done"),
            Err(RuleError::FinalActive("done".into()))
        );
    }

    #[test]
    fn pause_goes_to_sprint_only_when_linked_to_an_open_sprint_card() {
        assert_eq!(
            pause_destination(&[rel("related", "p-spr", false)], "me", Some("p-spr")),
            SPRINT
        );
        assert_eq!(
            pause_destination(&[rel("related", "p-spr", true)], "me", Some("p-spr")),
            BACKLOG
        );
        assert_eq!(
            pause_destination(&[rel("blocks", "p-spr", false)], "me", Some("p-spr")),
            BACKLOG
        );
        assert_eq!(
            pause_destination(&[rel("related", "p-api", false)], "me", Some("p-spr")),
            BACKLOG
        );
        assert_eq!(
            pause_destination(&[rel("related", "p-spr", false)], "me", None),
            BACKLOG
        );
    }

    #[test]
    fn a_card_on_the_sprints_project_does_not_match_itself() {
        // The card "me" lives on p-spr and links to a card elsewhere.
        let r = rel_on("related", "p-api", false, "p-spr");
        assert_eq!(pause_destination(&[r], "me", Some("p-spr")), BACKLOG);
    }

    #[test]
    fn final_column_is_the_one_kaneo_marks_final() {
        let cols = vec![col("doing", "", false), col("shipped", "", true)];
        assert_eq!(final_column(&cols).unwrap(), "shipped");
        assert_eq!(final_column(&cols[..1]), Err(RuleError::NoFinalColumn));
    }

    #[test]
    fn required_trims_and_rejects_blank() {
        assert_eq!(required("evidence", "  PR #4 ").unwrap(), "PR #4");
        assert_eq!(
            required("evidence", " \n"),
            Err(RuleError::Blank("evidence"))
        );
    }

    #[test]
    fn record_comments_are_stamped_in_utc() {
        assert_eq!(
            start_comment(now(), "agent-1", "build it"),
            "START 2026-10-27 10:05Z — agent-1 — build it"
        );
        assert_eq!(
            pause_comment(now(), "agent-1", "Half done.", "write tests"),
            "PAUSE 2026-10-27 10:05Z — agent-1 — Half done. Next: write tests"
        );
        assert_eq!(
            done_comment(now(), "agent-1", "PR #3, CI green"),
            "DONE 2026-10-27 10:05Z — agent-1 — PR #3, CI green"
        );
        let full = decision_comment(
            now(),
            &Decision {
                choice: "Use rmcp",
                reasoning: "single binary",
                provenance: "Ana: \"Rust\"",
                overruled: Some("memory"),
                covers: Some("abc123"),
            },
        );
        assert_eq!(
            full,
            "DECISION 2026-10-27 — Use rmcp\n\nWhy: single binary\nOverruled: memory\nCovers: abc123\nProvenance: Ana: \"Rust\""
        );
        assert!(is_decision(&full));
        assert!(!is_decision("A DECISION later in the text"));
    }

    #[test]
    fn plain_comments_cannot_impersonate_rule_comments() {
        assert!(plain_comment("Progress: tests written").is_ok());
        assert!(plain_comment("We STARTed late").is_ok());
        assert_eq!(
            plain_comment("  DECISION 2026-10-27 — ship")
                .unwrap_err()
                .to_string(),
            "a comment starting with \"DECISION\" goes through its own tool (decide)"
        );
        for (text, tool) in [
            ("START x", "start_card"),
            ("PAUSE x", "pause_card"),
            ("DONE x", "close_card"),
        ] {
            assert!(
                plain_comment(text)
                    .unwrap_err()
                    .to_string()
                    .ends_with(&format!("({tool})"))
            );
        }
    }

    #[test]
    fn default_column_is_read_from_the_intake_line() {
        assert_eq!(
            default_column("Web app.\n\n- New cards go to: Backlog\n- Column limit: ..."),
            Some("Backlog")
        );
        assert_eq!(default_column("- New cards go to: To Do."), Some("To Do"));
        assert_eq!(default_column("- New cards go to: \nnext"), None);
        assert_eq!(default_column("no intake stated"), None);
    }

    #[test]
    fn columns_resolve_by_slug_or_name() {
        let cols = vec![col("to-do", "To Do", false), col("done", "Done", true)];
        for wanted in ["to-do", "To Do", "todo", " TO-DO "] {
            assert_eq!(
                resolve_column(&cols, wanted).unwrap().slug,
                "to-do",
                "{wanted}"
            );
        }
        assert_eq!(
            resolve_column(&cols, "---").unwrap_err().to_string(),
            "project has no column \"---\"; there are: to-do, done"
        );
    }

    #[test]
    fn a_new_card_goes_to_the_stated_intake_or_the_first_open_column() {
        let board = kaneo_default();
        assert_eq!(
            intake_column(&board, "", Some("in-progress")).unwrap().slug,
            "to-do"
        );
        assert_eq!(
            intake_column(&board, "- New cards go to: In Review", Some("in-progress"))
                .unwrap()
                .slug,
            "in-review"
        );
        // The first column is the active one: the next open one takes new cards.
        let doing_first = vec![col("doing", "Doing", false), col("later", "Later", false)];
        assert_eq!(
            intake_column(&doing_first, "", Some("doing")).unwrap().slug,
            "later"
        );
        let only = vec![col("doing", "Doing", false), col("done", "Done", true)];
        assert_eq!(
            intake_column(&only, "", Some("doing")),
            Err(RuleError::NoDefaultColumn)
        );
    }

    #[test]
    fn a_pause_goes_to_the_rule_column_when_the_board_has_it() {
        let desc = "- New cards go to: Sprint";
        assert_eq!(
            pause_column(&sprint_board(), BACKLOG, desc, Some("doing"))
                .unwrap()
                .slug,
            "backlog"
        );
        assert_eq!(
            pause_column(&sprint_board(), SPRINT, desc, Some("doing"))
                .unwrap()
                .slug,
            "sprint"
        );
    }

    #[test]
    fn a_sprint_pause_on_a_board_without_sprint_falls_back_to_backlog_first() {
        let cols = vec![
            col("backlog", "Backlog", false),
            col("to-do", "To-Do", false),
        ];
        let desc = "- New cards go to: To-Do";
        assert_eq!(
            pause_column(&cols, SPRINT, desc, None).unwrap().slug,
            "backlog"
        );
    }

    #[test]
    fn a_pause_falls_back_to_the_intake_column_on_a_board_without_backlog() {
        let cols = vec![
            col("to-do", "To-Do", false),
            col("ready", "Ready", false),
            col("doing", "Doing", false),
            col("done", "Done", true),
        ];
        let desc = "Web app.\n\n- New cards go to: Ready\n";
        for rule in [BACKLOG, SPRINT] {
            assert_eq!(
                pause_column(&cols, rule, desc, Some("doing")).unwrap().slug,
                "ready"
            );
        }
    }

    #[test]
    fn a_pause_on_a_default_board_goes_to_its_first_open_column() {
        assert_eq!(
            pause_column(&kaneo_default(), BACKLOG, "", Some("in-progress"))
                .unwrap()
                .slug,
            "to-do"
        );
    }

    #[test]
    fn a_pause_never_lands_in_the_active_column_even_when_the_rule_names_it() {
        let desc = "- Work in progress: Sprint";
        assert_eq!(
            pause_column(&sprint_board(), SPRINT, desc, Some("sprint"))
                .unwrap()
                .slug,
            "backlog"
        );
    }

    #[test]
    fn only_a_card_in_progress_is_paused() {
        let doing = col("doing", "Doing", false);
        assert!(check_in_progress("WEB-5", "doing", &doing).is_ok());
        assert_eq!(
            check_in_progress("WEB-5", "done", &doing),
            Err(RuleError::NotInProgress {
                ticket: "WEB-5".into(),
                column: "done".into(),
                active: "doing".into()
            })
        );
    }

    #[test]
    fn a_pause_with_nowhere_to_go_is_refused() {
        let cols = vec![col("doing", "Doing", false), col("done", "Done", true)];
        assert_eq!(
            pause_column(&cols, BACKLOG, "", Some("doing")).unwrap_err(),
            RuleError::NoColumn(BACKLOG.into())
        );
    }

    #[test]
    fn a_pause_never_falls_back_into_the_active_or_the_final_column() {
        let cols = vec![col("doing", "Doing", false), col("done", "Done", true)];
        assert_eq!(
            pause_column(&cols, BACKLOG, "- New cards go to: Doing", Some("doing")).unwrap_err(),
            RuleError::PauseIntake("doing".into())
        );
        assert_eq!(
            pause_column(&cols, BACKLOG, "- New cards go to: Done", Some("doing")).unwrap_err(),
            RuleError::PauseIntake("done".into())
        );
    }

    #[test]
    fn plain_moves_stay_out_of_the_active_and_the_final_column() {
        let (sprint, doing, done) = (
            col("sprint", "Sprint", false),
            col("doing", "Doing", false),
            col("shipped", "Shipped", true),
        );
        let active = Some(&doing);
        assert!(check_leave("backlog", active).is_ok());
        assert!(check_enter(&sprint, active).is_ok());
        assert_eq!(
            check_enter(&doing, active),
            Err(RuleError::EnterOnly("Doing".into(), "start_card"))
        );
        assert_eq!(
            check_leave("doing", active),
            Err(RuleError::LeaveOnly(
                "Doing".into(),
                "pause_card and close_card"
            ))
        );
        // Reopening finished work is a plain move.
        assert!(check_leave("shipped", active).is_ok());
        assert_eq!(
            check_enter(&done, active),
            Err(RuleError::EnterOnly("Shipped".into(), "close_card"))
        );
        // A board without an active column still keeps its final column.
        assert!(check_enter(&doing, None).is_ok());
        assert!(check_leave("doing", None).is_ok());
        assert!(check_enter(&done, None).is_err());
    }

    #[test]
    fn dates_take_a_day_or_a_full_time() {
        assert_eq!(
            date("due", "2026-11-01").unwrap(),
            "2026-11-01T12:00:00.000Z"
        );
        assert_eq!(
            date("due", "2026-11-01T23:59:00+01:00").unwrap(),
            "2026-11-01T22:59:00.000Z"
        );
        assert_eq!(
            date("due", "01/11/2026"),
            Err(RuleError::BadDate("due", "01/11/2026".into()))
        );
    }

    #[test]
    fn priorities_and_relation_kinds_are_closed_sets() {
        assert_eq!(priority(" High ").unwrap(), "high");
        assert!(priority("p1").is_err());
        assert_eq!(relation_kind("Blocks").unwrap(), "blocks");
        assert!(relation_kind("duplicates").is_err());
    }

    #[test]
    fn a_comment_edit_keeps_its_kind() {
        assert!(check_comment_edit("Progress", "Progress, fixed").is_ok());
        assert!(check_comment_edit("DECISION 2026-10-27 — a", "DECISION 2026-10-27 — b").is_ok());
        assert_eq!(
            check_comment_edit("START x", "Progress"),
            Err(RuleError::KindChange("START", "plain"))
        );
        assert_eq!(
            check_comment_edit("note", "DONE 2026-10-27"),
            Err(RuleError::KindChange("plain", "DONE"))
        );
    }

    #[test]
    fn record_comments_are_not_deleted() {
        assert!(check_comment_delete("Progress").is_ok());
        assert_eq!(
            check_comment_delete("DONE 2026-10-27 — PR 3"),
            Err(RuleError::RecordComment("DONE"))
        );
    }

    #[test]
    fn start_must_not_follow_due() {
        let (a, b) = ("2026-10-27T12:00:00.000Z", "2026-11-01T22:59:00.000Z");
        assert!(check_date_order(Some(a), Some(b)).is_ok());
        assert!(check_date_order(Some(b), None).is_ok());
        assert!(check_date_order(Some(b), Some(a)).is_err());
    }

    #[test]
    fn a_description_edit_keeps_the_rules_it_had() {
        let board = sprint_board();
        let check = |old, new| check_description_rules(old, new, &board);
        let old = "- Work in progress: Doing\n- New cards go to: Backlog\n- Column limit: Doing holds at most 3 cards";
        assert!(check(old, old).is_ok());
        assert!(check("free text", "other text").is_ok());
        assert_eq!(
            check(old, "- Work in progress: Doing\n- New cards go to: Backlog"),
            Err(RuleError::DroppedRule("Column limit:"))
        );
        assert_eq!(
            check(
                old,
                "- New cards go to: Backlog\n- Column limit: Doing holds at most 2 cards"
            ),
            Err(RuleError::DroppedRule("Work in progress:"))
        );
        assert_eq!(
            check(
                old,
                "- Work in progress: Doing\n- Column limit: Doing holds at most 2 cards"
            ),
            Err(RuleError::DroppedRule("New cards go to:"))
        );
    }

    #[test]
    fn a_description_edit_may_tighten_the_rules_but_not_loosen_them() {
        let board = sprint_board();
        let check = |old, new| check_description_rules(old, new, &board);
        let old = "- Work in progress: Doing\n- Column limit: Doing holds at most 3 cards";
        assert!(
            check(
                old,
                "- Work in progress: Doing\n- Column limit: Doing holds at most 2 cards"
            )
            .is_ok()
        );
        assert!(check("free text", "- Column limit: Doing holds at most 2 cards").is_ok());
        assert_eq!(
            check(
                old,
                "- Work in progress: Doing\n- Column limit: Doing holds at most 9 cards"
            ),
            Err(RuleError::Loosened("the cap"))
        );
        assert_eq!(
            check(
                old,
                "- Work in progress: Sprint\n- Column limit: Sprint holds at most 3 cards"
            ),
            Err(RuleError::Loosened("the work-in-progress column"))
        );
        // Naming the column the board already falls back to changes nothing.
        assert!(check("free text", "- Work in progress: Doing").is_ok());
        // An old line that named no real column is not held against its repair.
        assert!(check("- Work in progress: Gone", "- Work in progress: Sprint").is_ok());
    }

    #[test]
    fn a_description_edit_must_state_rules_that_resolve() {
        let board = sprint_board();
        let check = |new| check_description_rules("", new, &board);
        assert!(matches!(
            check("- Column limit: Doing holds at most some cards"),
            Err(RuleError::BadCapLine(_))
        ));
        assert!(matches!(
            check("- Column limit: Sprint holds at most 2 cards"),
            Err(RuleError::CapElsewhere { .. })
        ));
        assert!(matches!(
            check("- New cards go to: Doing"),
            Err(RuleError::EnterOnly(..))
        ));
        assert!(matches!(
            check("- Work in progress: Review"),
            Err(RuleError::UnknownColumn { .. })
        ));
    }
}
