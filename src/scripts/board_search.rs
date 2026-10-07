//! Finding a piece of work by what was WRITTEN about it, rather than by where it sits.
//!
//! Every other read on this board is a filter: a column, a kind, an assignee, a goal. Those answer "what is
//! in this state", which is what a board is for — and they cannot answer the question an agent actually
//! arrives with, which is "where did we discuss X". The text is the only place that knows, and most of it
//! lives on threads that no listing returns: `tasks_list` reports `comments_amount` and not one comment,
//! for the same context reason `documents_list` reports no texts.
//!
//! So the reasoning behind a decision is reachable today only by listing a board and then reading every
//! thread on it one call at a time. This is that, done in one pass over memory.

use crate::app::AppContext;
use crate::board::{BoardInner, GoalModel, ProjectModel, TaskModel, compose_task_handle};

use super::{Matcher, TextMatch, search_text};

/// The most cards one search reports. A board is a hand-written list, but a one-letter query over a
/// long-lived project would still return all of it — and a search that returns everything has answered
/// nothing.
pub const MAX_BOARD_HITS: i64 = 40;

/// How many cards come back when nobody says. Below the ceiling on purpose: the default is what a caller
/// gets without thinking about it, and the ceiling is there for the one who did.
pub const DEFAULT_BOARD_HITS: i64 = 30;

/// The most matching lines reported per card, across all of its parts.
pub const MAX_MATCHES_PER_CARD: usize = 6;

/// Which part of a card the text was found in.
///
/// Reported per match rather than per card because it changes what the caller does next: text found in the
/// task itself is what the work IS, and text found on the thread is what somebody said about it — usually
/// the more useful of the two, and always the one that needs `tasks_get_comments` to read in full.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoardMatchPart {
    /// A task's own text, or a goal's name and description.
    Text,
    /// A comment on the thread.
    Comment,
    /// A checklist item — its title or its longer half.
    Subtask,
}

impl BoardMatchPart {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Comment => "comment",
            Self::Subtask => "subtask",
        }
    }
}

/// One line that matched, and where on the card it was.
pub struct BoardMatch {
    pub part: BoardMatchPart,
    /// Who wrote it, for a comment. Absent for the other parts, whose author is the card's own history.
    pub who: Option<String>,
    pub moment_unix_seconds: Option<i64>,
    pub line: String,
}

/// One card that matched.
pub struct BoardHit {
    /// `RMS-42` for a task, `RMS-G7` for a goal.
    pub id: String,
    pub is_goal: bool,
    /// The first line of the text — what the card reads as at a glance, so a caller can pick without a
    /// second call.
    pub headline: String,
    pub status: String,
    pub priority: String,
    /// The goal a task belongs to. Absent on a goal, and on a task that stands on its own.
    pub goal: Option<String>,
    pub assignee: Option<String>,
    pub matches: Vec<BoardMatch>,
    pub matches_total: i32,
    pub comments_amount: i32,
    pub is_archived: bool,
    pub is_deleted: bool,
}

/// What a caller is looking for on a board.
pub struct BoardSearchQuery {
    pub query: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub include_comments: bool,
    pub include_archived: bool,
    pub include_deleted: bool,
    pub goals_only: bool,
    pub max_results: i64,
}

/// What a board search found.
pub struct BoardSearchOutcome {
    pub hits: Vec<BoardHit>,
    pub searched_tasks: i32,
    pub searched_goals: i32,
    pub truncated: bool,
}

/// Search one project's tasks, goals and threads for a piece of text.
///
/// **Served entirely from memory**, which is what makes searching the threads affordable at all: comments
/// ride inside the task's own row and are already in the snapshot, so this is a walk over what is loaded
/// rather than a query. That is also why there is no equivalent of the documents search's blob problem —
/// there are no blobs on this side.
///
/// Deleted and archived work is left out by default and reachable by asking. Both defaults match what
/// `tasks_list` does, so "search finds a task the board does not show" cannot happen by accident.
pub fn search_board(
    app: &AppContext,
    project_prefix: &str,
    query: &BoardSearchQuery,
) -> Result<BoardSearchOutcome, String> {
    let matcher = Matcher::new(&query.query, query.is_regex, query.case_sensitive)?;

    let board = app.board.read();
    let project = super::resolve_project_by_prefix(&board, project_prefix)?;

    let max_results = query.max_results.clamp(1, MAX_BOARD_HITS) as usize;

    let mut outcome = BoardSearchOutcome {
        hits: Vec::new(),
        searched_tasks: 0,
        searched_goals: 0,
        truncated: false,
    };

    // Goals first, deliberately: a goal is the container work is organised under, so when both a goal and
    // its tasks mention something, the goal is the thread to read.
    for goal in board.goals_of_project(&project.id) {
        if goal.is_deleted() && !query.include_deleted {
            continue;
        }

        if board.is_goal_archived(&goal) && !query.include_archived {
            continue;
        }

        outcome.searched_goals += 1;

        if let Some(hit) = match_goal(&matcher, &goal, &project, &board, query) {
            outcome.hits.push(hit);
        }
    }

    if !query.goals_only {
        for task in board.tasks_of_project(&project.id) {
            if task.is_deleted() && !query.include_deleted {
                continue;
            }

            if board.is_archived(&task) && !query.include_archived {
                continue;
            }

            outcome.searched_tasks += 1;

            if let Some(hit) = match_task(&matcher, &task, &project, &board, query) {
                outcome.hits.push(hit);
            }
        }
    }

    // Most matches first: a card that mentions the thing four times is more likely to be the one somebody
    // meant than one that mentions it in passing. Ties break by id, so the order is stable between two
    // identical calls — a search whose order moves is one a caller cannot page through by eye.
    outcome.hits.sort_by(|left, right| {
        right
            .matches_total
            .cmp(&left.matches_total)
            .then_with(|| left.id.cmp(&right.id))
    });

    if outcome.hits.len() > max_results {
        outcome.truncated = true;
        outcome.hits.truncate(max_results);
    }

    Ok(outcome)
}

/// One task, if anything on it matched.
fn match_task(
    matcher: &Matcher,
    task: &TaskModel,
    project: &ProjectModel,
    board: &BoardInner,
    query: &BoardSearchQuery,
) -> Option<BoardHit> {
    let mut matches = Vec::new();
    let mut total = 0;

    collect(matcher, &task.text, BoardMatchPart::Text, None, None, &mut matches, &mut total);

    for subtask in &task.subtasks {
        collect(matcher, &subtask.title, BoardMatchPart::Subtask, None, None, &mut matches, &mut total);
        collect(matcher, &subtask.text, BoardMatchPart::Subtask, None, None, &mut matches, &mut total);
    }

    if query.include_comments {
        for comment in &task.comments {
            collect(
                matcher,
                &comment.text,
                BoardMatchPart::Comment,
                Some(&comment.who),
                Some(comment.moment.unix_microseconds / 1_000_000),
                &mut matches,
                &mut total,
            );
        }
    }

    if total == 0 {
        return None;
    }

    let goal = board.effective_goal(task);

    Some(BoardHit {
        id: compose_task_handle(&project.prefix, task.number),
        is_goal: false,
        headline: headline_of(&task.text),
        status: project.effective_status(&task.status),
        priority: rust_extensions::AsStr::as_str(&task.priority).to_string(),
        goal: goal.map(|itm| crate::board::compose_goal_handle(&project.prefix, itm.number)),
        assignee: task.assignee.clone(),
        matches,
        matches_total: total,
        comments_amount: task.comments.len() as i32,
        is_archived: board.is_archived(task),
        is_deleted: task.is_deleted(),
    })
}

/// One goal, if anything on it matched.
fn match_goal(
    matcher: &Matcher,
    goal: &GoalModel,
    project: &ProjectModel,
    board: &BoardInner,
    query: &BoardSearchQuery,
) -> Option<BoardHit> {
    let mut matches = Vec::new();
    let mut total = 0;

    collect(matcher, &goal.name, BoardMatchPart::Text, None, None, &mut matches, &mut total);
    collect(matcher, &goal.description, BoardMatchPart::Text, None, None, &mut matches, &mut total);

    for subtask in &goal.subtasks {
        collect(matcher, &subtask.title, BoardMatchPart::Subtask, None, None, &mut matches, &mut total);
        collect(matcher, &subtask.text, BoardMatchPart::Subtask, None, None, &mut matches, &mut total);
    }

    if query.include_comments {
        for comment in &goal.comments {
            collect(
                matcher,
                &comment.text,
                BoardMatchPart::Comment,
                Some(&comment.who),
                Some(comment.moment.unix_microseconds / 1_000_000),
                &mut matches,
                &mut total,
            );
        }
    }

    if total == 0 {
        return None;
    }

    Some(BoardHit {
        id: crate::board::compose_goal_handle(&project.prefix, goal.number),
        is_goal: true,
        headline: goal.name.clone(),
        status: goal.status().to_string(),
        priority: rust_extensions::AsStr::as_str(&goal.priority).to_string(),
        goal: None,
        assignee: None,
        matches,
        matches_total: total,
        comments_amount: goal.comments.len() as i32,
        is_archived: board.is_goal_archived(goal),
        is_deleted: goal.is_deleted(),
    })
}

/// Add whatever matched in one piece of text, keeping at most a handful per card.
///
/// The total keeps counting past the cap, exactly as the document search does: "6 of 19" tells the caller
/// this card is where the conversation happened, which six lines alone would not.
fn collect(
    matcher: &Matcher,
    text: &str,
    part: BoardMatchPart,
    who: Option<&str>,
    moment_unix_seconds: Option<i64>,
    into: &mut Vec<BoardMatch>,
    total: &mut i32,
) {
    if text.is_empty() {
        return;
    }

    // No context lines: a task's text is a sticker and a comment is a paragraph or two, so the line IS the
    // context. The document search takes context because a line of a 76 KB specification is not.
    let (matches, found) = search_text(matcher, text, 0, MAX_MATCHES_PER_CARD);

    *total += found;

    for TextMatch { line, .. } in matches {
        if into.len() >= MAX_MATCHES_PER_CARD {
            return;
        }

        into.push(BoardMatch {
            part,
            who: who.map(|itm| itm.to_string()),
            moment_unix_seconds,
            line,
        });
    }
}

/// The first non-empty line of a text, short enough to read in a list.
///
/// A task's text is Markdown and its first line is written to be the headline of the work — which is what
/// the board's card draws, so this is the same thing a person would be looking at.
fn headline_of(text: &str) -> String {
    const LIMIT: usize = 160;

    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");

    if first.chars().count() <= LIMIT {
        return first.to_string();
    }

    let clipped: String = first.chars().take(LIMIT).collect();

    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_headline_is_the_first_line_that_says_something() {
        assert_eq!(headline_of("\n\n**Fix the thing**\nmore"), "**Fix the thing**");
        assert_eq!(headline_of(""), "");
    }

    #[test]
    fn a_long_headline_is_clipped() {
        let headline = headline_of(&"a".repeat(400));

        assert!(headline.chars().count() <= 161);
        assert!(headline.ends_with('…'));
    }

    /// The four parts have to be distinct strings, or a caller cannot tell "the work says this" from
    /// "somebody said this about the work" — which is the one distinction this field exists for.
    #[test]
    fn every_part_has_its_own_name() {
        let names = [
            BoardMatchPart::Text.as_str(),
            BoardMatchPart::Comment.as_str(),
            BoardMatchPart::Subtask.as_str(),
        ];

        let mut sorted = names.to_vec();
        sorted.sort();
        sorted.dedup();

        assert_eq!(sorted.len(), names.len(), "two parts share a name");
    }

    /// The cap is per card and the count is not — the same rule the document search follows, and the reason
    /// a caller can tell a passing mention from the thread where it was decided.
    #[test]
    fn the_per_card_cap_does_not_stop_the_count() {
        let matcher = Matcher::new("x", false, false).unwrap();

        let mut matches = Vec::new();
        let mut total = 0;

        let text = "x\n".repeat(20);
        collect(&matcher, &text, BoardMatchPart::Text, None, None, &mut matches, &mut total);

        assert_eq!(matches.len(), MAX_MATCHES_PER_CARD);
        assert_eq!(total, 20);
    }
}
