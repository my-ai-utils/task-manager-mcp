use std::collections::HashMap;

use dioxus::prelude::*;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::projects::{COLUMN_ID_DONE, COLUMN_ID_TODO, ProjectResponse};
use task_manager_shared::tasks::TaskResponse;

use super::super::render::{GoalStatus, task_status_name};
use super::actions::{
    DAY, GoalLane, Life, Month, Row, Span, TaskLane, date_of, lay_out, wall_clock,
};

/// The goals of one month as a Gantt chart: a row per goal, a column per day — and under a goal somebody
/// unfolded, a row per task.
///
/// `month` is `None` for "this month", which is what the screen opens on and what `Today` goes back to —
/// held as `None` rather than as the month it is now, so a tab left open over midnight on the 31st moves
/// on with the calendar.
#[component]
pub fn RenderTimeline(
    goals: Vec<(GoalResponse, GoalStatus)>,
    of_goal: HashMap<String, Vec<TaskResponse>>,
    project: ProjectResponse,
    month: Option<Month>,
    wanted: Option<GoalStatus>,
    expanded: Vec<String>,
    hide_done: bool,
    on_month: EventHandler<Option<Month>>,
    on_toggle: EventHandler<String>,
) -> Element {
    let now = wall_clock(js_sys::Date::now() as i64 / 1_000);
    let this_month = Month::of(now);
    let shown = month.unwrap_or(this_month);

    let rows = lay_out(
        shown,
        goals,
        &of_goal,
        &expanded,
        hide_done,
        now,
        &wall_clock,
    );

    let days = shown.days();
    let today = shown.day_of(now);

    // A goal's row is taller than the rows of its tasks: the tree reads as a tree by its rhythm before
    // anybody notices the indent.
    let heights: Vec<&str> = rows
        .iter()
        .map(|row| match row {
            Row::Goal(_) => "var(--gantt-row)",
            Row::Task(_) | Row::Nothing { .. } => "var(--gantt-task-row)",
        })
        .collect();

    // The template, inline, because the number of columns is the month's and the rows are the data's.
    // Every day the same width, which is what lets a lane's day lines be one repeating gradient.
    let grid_style = format!(
        "grid-template-columns: var(--gantt-label) repeat({days}, minmax(var(--gantt-day), 1fr)); \
         grid-template-rows: var(--gantt-head) {};",
        heights.join(" ")
    );
    let lines_style = format!("background-size: calc(100% / {days}) 100%;");

    let empty_note = if rows.is_empty() {
        let which = match wanted {
            Some(status) => format!("No {} goal", status.title()),
            None => "No goal".to_string(),
        };
        let hidden = if hide_done && wanted.is_none() {
            " Done goals are hidden."
        } else {
            ""
        };
        Some(format!("{which} was open in {}.{hidden}", shown.title()))
    } else {
        None
    };

    let rows_end = rows.len() + 2;

    rsx! {
        div { class: "gantt-toolbar",
            button {
                class: "btn btn-sm",
                title: "Previous month",
                onclick: move |_| on_month.call(Some(shown.prev())),
                "‹"
            }
            span { class: "gantt-month", "{shown.title()}" }
            button {
                class: "btn btn-sm",
                title: "Next month",
                onclick: move |_| on_month.call(Some(shown.next())),
                "›"
            }
            button {
                class: "btn btn-sm",
                disabled: shown == this_month,
                onclick: move |_| on_month.call(None),
                "Today"
            }

            div { class: "gantt-legend",
                span { class: "gantt-legend-item",
                    span { class: "gantt-legend-wait" }
                    "waiting"
                }
                span { class: "gantt-legend-item",
                    span { class: "gantt-legend-work" }
                    "in work"
                }
                span { class: "gantt-legend-item",
                    span { class: "gantt-legend-tentative" }
                    "no start or end set"
                }
                span { class: "gantt-legend-item",
                    span { class: "gantt-release" }
                    "release"
                }
                span { class: "gantt-legend-item",
                    span { class: "gantt-done" }
                    "task done"
                }
            }
        }

        if let Some(note) = empty_note {
            div { class: "empty-note", "{note}" }
        } else {
            div { class: "gantt",
                div { class: "gantt-grid", style: "{grid_style}",
                    div { class: "gantt-corner", style: "grid-row: 1; grid-column: 1", "Goal" }

                    div { class: "gantt-head-lines", style: "grid-row: 1; grid-column: 2 / -1; {lines_style}" }

                    for day in 0..days {
                        div {
                            key: "head-{day}",
                            class: day_class("gantt-day", shown.weekday(day), today == Some(day)),
                            style: "grid-row: 1; grid-column: {day + 2}",
                            span { class: "gantt-day-number", "{day + 1}" }
                            span { class: "gantt-day-name", "{weekday_letter(shown.weekday(day))}" }
                        }
                    }

                    // Weekends and today, as one tall stripe each rather than a cell per row: the stripe
                    // is under every lane at once, and the lanes stay one element wide.
                    for day in (0..days).filter(|day| shown.weekday(*day) >= 5 || today == Some(*day)) {
                        div {
                            key: "stripe-{day}",
                            class: if today == Some(day) { "gantt-stripe today" } else { "gantt-stripe" },
                            style: "grid-row: 2 / {rows_end}; grid-column: {day + 2}",
                        }
                    }

                    for (index , row) in rows.iter().enumerate() {
                        RenderRow {
                            key: "{row.key()}",
                            row: row.clone(),
                            line: index + 2,
                            lines_style: lines_style.clone(),
                            month: shown,
                            project: project.clone(),
                            on_toggle,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenderRow(
    row: Row,
    line: usize,
    lines_style: String,
    month: Month,
    project: ProjectResponse,
    on_toggle: EventHandler<String>,
) -> Element {
    match row {
        Row::Goal(lane) => rsx! {
            RenderGoalLane { lane, line, lines_style, month, on_toggle }
        },
        Row::Task(lane) => rsx! {
            RenderTaskLane { lane, line, lines_style, project }
        },
        Row::Nothing { .. } => rsx! {
            div {
                class: "gantt-label task nothing",
                style: "grid-row: {line}; grid-column: 1",
                "Nothing under this goal in {month.title()}"
            }
            div {
                class: "gantt-lane",
                style: "grid-row: {line}; grid-column: 2 / -1; {lines_style}",
            }
        },
    }
}

/// A goal's row. The row is the fold: a click on its name or its bar unfolds its tasks, the way a click
/// on a goal's head does in the list — and the eye at the end opens the goal itself, as it does there.
#[component]
fn RenderGoalLane(
    lane: GoalLane,
    line: usize,
    lines_style: String,
    month: Month,
    on_toggle: EventHandler<String>,
) -> Element {
    let goal = &lane.goal;
    let status = lane.status;
    let hex = KindColor::parse_or_default(&goal.color).hex();

    let tooltip = format!(
        "{} · {}\n{}\n{} / {} tasks done\nClick to {} its tasks",
        goal.id,
        goal.name,
        life_lines(&lane.life, "Closed", status == GoalStatus::Todo),
        goal.done_amount,
        goal.tasks_amount,
        if lane.open { "fold" } else { "unfold" },
    );

    let marks = Marks::of(&lane, month);

    let waiting = lane.bars.waiting.map(|span| {
        (
            stretch_class("gantt-wait", span),
            format!("grid-row: {line}; {} color: {hex};", columns(span)),
        )
    });
    let work = lane.bars.work.map(|span| {
        (
            span,
            stretch_class(&bar_kind("gantt-bar", lane.bars.tentative), span),
            format!(
                "grid-row: {line}; {} {}",
                columns(span),
                bar_background(span, hex, lane.bars.tentative)
            ),
        )
    });
    let closed_here = lane.life.closed.is_some();

    let for_dialog = goal.clone();
    let id = goal.id.clone();
    let toggle = move |_: MouseEvent| on_toggle.call(id.clone());
    // One per element that folds the row: `rsx!` may build the conditional ones in any order, so none of
    // them may be the one that moves the original.
    let (toggle_label, toggle_wait, toggle_bar) = (toggle.clone(), toggle.clone(), toggle);

    rsx! {
        div {
            class: "gantt-label goal",
            style: "grid-row: {line}; grid-column: 1",
            title: "{tooltip}",
            onclick: toggle_label,
            span { class: "goal-caret", if lane.open { "▾" } else { "▸" } }
            span { class: "gantt-swatch", style: "background: {hex}" }
            span { class: "goal-id", "{goal.id}" }
            span { class: "gantt-name", "{goal.name}" }
            span { class: "goal-status {status.key()}", title: "{status.hint()}", "{status.title()}" }
            // `stop_propagation`, because the row around it is the fold: one click, one thing.
            button {
                class: "gantt-eye",
                title: "Read this goal and its comments",
                onclick: move |event| {
                    event.stop_propagation();
                    open_goal(&for_dialog, status);
                },
                "👁"
            }
        }

        div {
            class: "gantt-lane",
            style: "grid-row: {line}; grid-column: 2 / -1; {lines_style}",
        }

        if let Some((class, style)) = waiting {
            div {
                class: "{class}",
                style: "{style}",
                title: "{tooltip}",
                onclick: toggle_wait,
            }
        }

        if let Some((span, class, style)) = work {
            div {
                class: "{class}",
                style: "{style}",
                title: "{tooltip}",
                onclick: toggle_bar,
                if span.cut_before {
                    span { class: "gantt-bar-cut", "‹" }
                }
                // The count rides inside the bar only where there is room for it — two days and up.
                if span.last > span.first {
                    span { class: "gantt-bar-text", "{goal.done_amount} / {goal.tasks_amount}" }
                }
                if closed_here && !span.cut_after {
                    span { class: "gantt-bar-end", "✓" }
                }
                if span.cut_after {
                    span { class: "gantt-bar-cut after", "›" }
                }
            }
        }

        for (day , hint) in marks.done {
            div {
                key: "done-{day}",
                class: "gantt-mark done",
                style: "grid-row: {line}; grid-column: {day + 2}",
                span { class: "gantt-done", title: "{hint}" }
            }
        }

        for (day , hint) in marks.releases {
            div {
                key: "release-{day}",
                class: "gantt-mark release",
                style: "grid-row: {line}; grid-column: {day + 2}",
                span { class: "gantt-release", title: "{hint}" }
            }
        }
    }
}

/// A task's row, under its goal: drawn in the goal's colour, lighter, and opening the task the way its
/// card does.
#[component]
fn RenderTaskLane(
    lane: TaskLane,
    line: usize,
    lines_style: String,
    project: ProjectResponse,
) -> Element {
    let task = &lane.task;
    let hex = KindColor::parse_or_default(&lane.color).hex();
    let title = task_manager_shared::task_title::task_title(&task.text);
    let status_name = task_status_name(&task.status, &project);
    let done = task.status == COLUMN_ID_DONE;

    let assignee = task
        .assignee_name
        .clone()
        .or_else(|| task.assignee.clone())
        .unwrap_or_else(|| "Unassigned".to_string());

    let tooltip = format!(
        "{} · {title}\n{}\nIn {status_name}, {assignee}",
        task.id,
        life_lines(&lane.life, "Done", task.status == COLUMN_ID_TODO),
    );

    let waiting = lane.bars.waiting.map(|span| {
        (
            stretch_class("gantt-wait task", span),
            format!("grid-row: {line}; {} color: {hex};", columns(span)),
        )
    });
    let work = lane.bars.work.map(|span| {
        (
            span,
            stretch_class(&bar_kind("gantt-bar task", lane.bars.tentative), span),
            format!(
                "grid-row: {line}; {} {}",
                columns(span),
                bar_background(span, hex, lane.bars.tentative)
            ),
        )
    });
    let closed_here = lane.life.closed.is_some();

    let found = crate::api::find_task_locally(task, &project);
    let open = move |_: MouseEvent| {
        crate::dialogs::open(crate::dialogs::DialogState::ViewTask {
            found: found.clone(),
        });
    };
    let (open_label, open_wait, open_bar) = (open.clone(), open.clone(), open);

    rsx! {
        div {
            class: if done { "gantt-label task done" } else { "gantt-label task" },
            style: "grid-row: {line}; grid-column: 1",
            title: "{tooltip}",
            onclick: open_label,
            span { class: "goal-task-id", "{task.id}" }
            span { class: "gantt-task-title", "{title}" }
            span { class: "gantt-task-status", "{status_name}" }
        }

        div {
            class: "gantt-lane",
            style: "grid-row: {line}; grid-column: 2 / -1; {lines_style}",
        }

        if let Some((class, style)) = waiting {
            div {
                class: "{class}",
                style: "{style}",
                title: "{tooltip}",
                onclick: open_wait,
            }
        }

        if let Some((span, class, style)) = work {
            div {
                class: "{class}",
                style: "{style}",
                title: "{tooltip}",
                onclick: open_bar,
                if closed_here && !span.cut_after {
                    span { class: "gantt-bar-end", "✓" }
                }
            }
        }
    }
}

/// The day marks on a goal's row, with what each says on hover.
struct Marks {
    done: Vec<(u32, String)>,
    releases: Vec<(u32, String)>,
}

impl Marks {
    fn of(lane: &GoalLane, month: Month) -> Self {
        let date_of_day = |day: u32| date_of(month.start() + day as i64 * DAY);

        let done = lane
            .done
            .iter()
            .map(|(day, tasks)| {
                let what = if tasks.len() == 1 {
                    "1 task".to_string()
                } else {
                    format!("{} tasks", tasks.len())
                };
                (
                    *day,
                    format!("{what} done {}: {}", date_of_day(*day), tasks.join(", ")),
                )
            })
            .collect();

        let releases = lane
            .releases
            .iter()
            .map(|(day, titles)| {
                (
                    *day,
                    format!("Released {}: {}", date_of_day(*day), titles.join(", ")),
                )
            })
            .collect();

        Self { done, releases }
    }
}

/// Where a stretch sits: the days it covers, as grid columns. The first column is the names.
fn columns(span: Span) -> String {
    format!("grid-column: {} / {};", span.first + 2, span.last + 3)
}

/// A stretch's classes: what it is, and which of its ends are cut by the edges of the month.
fn stretch_class(base: &str, span: Span) -> String {
    let mut class = base.to_string();

    if span.cut_before {
        class.push_str(" cut-before");
    }
    if span.cut_after {
        class.push_str(" cut-after");
    }
    if span.ongoing {
        class.push_str(" ongoing");
    }

    class
}

/// A bar's classes before the cuts: what it is, and whether it is a guess — see `Life::tentative`.
fn bar_kind(base: &str, tentative: bool) -> String {
    if tentative {
        format!("{base} tentative")
    } else {
        base.to_string()
    }
}

/// What is under way fades out at today rather than ending square: it stops there because that is as far
/// as time has got, not because the work did. A guess is drawn see-through inside a dashed edge of the same
/// colour — visible, and plainly not a record.
fn bar_background(span: Span, hex: &str, tentative: bool) -> String {
    if tentative {
        format!("background: {hex}73; border-color: {hex};")
    } else if span.ongoing {
        format!(
            "background: linear-gradient(90deg, {hex} 0, {hex} calc(100% - 18px), {hex}33 100%);"
        )
    } else {
        format!("background: {hex};")
    }
}

/// The three moments of a life, as the tooltips say them — including which kind of missing start a bar
/// with none on record is: not started at all (`in_todo`), started without anybody saying when, or finished
/// before starts were recorded.
fn life_lines(life: &Life, closed_word: &str, in_todo: bool) -> String {
    let opened = format!("Opened {}", date_of(life.opened));

    let started = if life.start_on_record {
        format!("Started {}", date_of(life.started))
    } else if in_todo {
        "Not started yet — drawn from when it was opened".to_string()
    } else if life.tentative() {
        "Start not set — drawn from when it was opened".to_string()
    } else {
        "Start not recorded — drawn from when it was opened".to_string()
    };

    let closed = match life.closed {
        Some(closed) => format!("{closed_word} {}", date_of(closed)),
        None => "Still open".to_string(),
    };

    format!("{opened}\n{started}\n{closed}")
}

fn open_goal(goal: &GoalResponse, status: GoalStatus) {
    crate::dialogs::open(crate::dialogs::DialogState::ViewGoal {
        goal: goal.clone(),
        status: status.title().to_string(),
    });
}

fn day_class(base: &str, weekday: u32, today: bool) -> String {
    let mut class = base.to_string();

    if weekday >= 5 {
        class.push_str(" weekend");
    }
    if today {
        class.push_str(" today");
    }

    class
}

fn weekday_letter(weekday: u32) -> &'static str {
    ["M", "T", "W", "T", "F", "S", "S"][weekday as usize % 7]
}
