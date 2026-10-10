use std::collections::HashMap;

use dioxus::prelude::*;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::tasks::TaskResponse;

use super::super::render::GoalStatus;
use super::actions::{DAY, Lane, Month, date_of, lay_out, wall_clock};

/// The goals of one month as a Gantt chart: a row per goal, a column per day.
///
/// `month` is `None` for "this month", which is what the screen opens on and what `Today` goes back to —
/// held as `None` rather than as the month it is now, so a tab left open over midnight on the 31st moves
/// on with the calendar.
#[component]
pub fn RenderTimeline(
    goals: Vec<(GoalResponse, GoalStatus)>,
    of_goal: HashMap<String, Vec<TaskResponse>>,
    month: Option<Month>,
    wanted: Option<GoalStatus>,
    on_month: EventHandler<Option<Month>>,
) -> Element {
    let now = wall_clock(js_sys::Date::now() as i64 / 1_000);
    let this_month = Month::of(now);
    let shown = month.unwrap_or(this_month);

    let lanes = lay_out(shown, goals, &of_goal, now, &wall_clock);

    let days = shown.days();
    let today = shown.day_of(now);

    // The template, inline, because the number of columns is the month's and the number of rows is the
    // data's. Every day the same width, which is what lets a lane's day lines be one repeating gradient.
    let grid_style = format!(
        "grid-template-columns: var(--gantt-label) repeat({days}, minmax(var(--gantt-day), 1fr)); \
         grid-template-rows: var(--gantt-head) repeat({}, var(--gantt-row));",
        lanes.len()
    );
    let lines_style = format!("background-size: calc(100% / {days}) 100%;");

    let empty_note = if lanes.is_empty() {
        Some(match wanted {
            Some(status) => format!("No {} goal was open in {}.", status.title(), shown.title()),
            None => format!("No goal was open in {}.", shown.title()),
        })
    } else {
        None
    };

    let rows_end = lanes.len() + 2;

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

                    for (row , lane) in lanes.iter().enumerate() {
                        RenderLane {
                            key: "{lane.goal.id}",
                            lane: lane.clone(),
                            row: row + 2,
                            lines_style: lines_style.clone(),
                            month_start: shown.start(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenderLane(lane: Lane, row: usize, lines_style: String, month_start: i64) -> Element {
    let goal = &lane.goal;
    let span = lane.span;
    let hex = KindColor::parse_or_default(&goal.color).hex();
    let status = lane.status;

    let opened = date_of(wall_clock(goal.created_unix_seconds));
    let until = match goal.closed_unix_seconds {
        Some(closed) => format!("closed {}", date_of(wall_clock(closed))),
        None => "still open".to_string(),
    };
    let tooltip = format!(
        "{} · {}\nOpened {opened}, {until}\n{} / {} tasks done",
        goal.id, goal.name, goal.done_amount, goal.tasks_amount
    );

    let mut bar_class = "gantt-bar".to_string();
    if span.cut_before {
        bar_class.push_str(" cut-before");
    }
    if span.cut_after {
        bar_class.push_str(" cut-after");
    }
    if span.ongoing {
        bar_class.push_str(" ongoing");
    }

    // An open goal's bar fades out at today rather than ending square: it stops there because that is as
    // far as time has got, not because the goal did.
    let bar_background = if span.ongoing {
        format!(
            "background: linear-gradient(90deg, {hex} 0, {hex} calc(100% - 18px), {hex}33 100%)"
        )
    } else {
        format!("background: {hex}")
    };

    // The count rides inside the bar only where there is room for it — two days and up.
    let wide = span.last > span.first;
    let closed_here = goal.closed_unix_seconds.is_some() && !span.cut_after;

    let date_of_day = |day: u32| date_of(month_start + day as i64 * DAY);

    let done_marks: Vec<(u32, String)> = lane
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

    let release_marks: Vec<(u32, String)> = lane
        .releases
        .iter()
        .map(|(day, titles)| {
            (
                *day,
                format!("Released {}: {}", date_of_day(*day), titles.join(", ")),
            )
        })
        .collect();

    let label_goal = goal.clone();
    let bar_goal = goal.clone();

    rsx! {
        div {
            class: "gantt-label",
            style: "grid-row: {row}; grid-column: 1",
            title: "{tooltip}",
            onclick: move |_| open_goal(&label_goal, status),
            span { class: "gantt-swatch", style: "background: {hex}" }
            span { class: "goal-id", "{goal.id}" }
            span { class: "gantt-name", "{goal.name}" }
            span { class: "goal-status {status.key()}", title: "{status.hint()}", "{status.title()}" }
        }

        div {
            class: "gantt-lane",
            style: "grid-row: {row}; grid-column: 2 / -1; {lines_style}",
        }

        div {
            class: "{bar_class}",
            style: "grid-row: {row}; grid-column: {span.first + 2} / {span.last + 3}; {bar_background}",
            title: "{tooltip}",
            onclick: move |_| open_goal(&bar_goal, status),
            if span.cut_before {
                span { class: "gantt-bar-cut", "‹" }
            }
            if wide {
                span { class: "gantt-bar-text", "{goal.done_amount} / {goal.tasks_amount}" }
            }
            if closed_here {
                span { class: "gantt-bar-end", "✓" }
            }
            if span.cut_after {
                span { class: "gantt-bar-cut after", "›" }
            }
        }

        for (day , hint) in done_marks {
            div {
                key: "done-{day}",
                class: "gantt-mark done",
                style: "grid-row: {row}; grid-column: {day + 2}",
                span { class: "gantt-done", title: "{hint}" }
            }
        }

        for (day , hint) in release_marks {
            div {
                key: "release-{day}",
                class: "gantt-mark release",
                style: "grid-row: {row}; grid-column: {day + 2}",
                span { class: "gantt-release", title: "{hint}" }
            }
        }
    }
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
