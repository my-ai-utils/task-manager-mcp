//! What the timeline is worked out from: the calendar, and where each goal's bar falls on it. No
//! rendering here — every function but [`wall_clock`] is plain arithmetic, tested natively below.

use std::collections::{BTreeMap, HashMap, HashSet};

use task_manager_shared::goals::GoalResponse;
use task_manager_shared::projects::COLUMN_ID_DONE;
use task_manager_shared::tasks::TaskResponse;

use super::super::render::GoalStatus;

pub const DAY: i64 = 24 * 60 * 60;

/// One calendar month on the reader's wall clock.
///
/// **Everything in this module counts WALL-CLOCK seconds**: seconds since 1970-01-01 00:00 in the reader's
/// own zone, so a day boundary is the reader's midnight and "today" is the date on their calendar — a grid
/// in UTC put today on yesterday for the first three hours of every day in Moscow. Stamps — when a goal was
/// opened or closed, when a task was done, now — are moved onto that clock by the component, with
/// [`wall_clock`]. A release's date is not a stamp: it is a calendar date, stored as that midnight in UTC,
/// so it is already the wall-clock midnight of its own date and goes in as it is — moved, it would land on
/// the day before for anybody west of Greenwich. The same reason `moment_for_display` draws it in UTC.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Month {
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
}

impl Month {
    pub fn of(unix_seconds: i64) -> Self {
        let (year, month, _) = civil_from_days(unix_seconds.div_euclid(DAY));
        Self { year, month }
    }

    pub fn prev(self) -> Self {
        if self.month == 1 {
            Self {
                year: self.year - 1,
                month: 12,
            }
        } else {
            Self {
                year: self.year,
                month: self.month - 1,
            }
        }
    }

    pub fn next(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }

    /// Midnight of the 1st, in wall-clock seconds.
    pub fn start(self) -> i64 {
        days_from_civil(self.year, self.month, 1) * DAY
    }

    /// Midnight of the 1st of the NEXT month — where this one stops, exclusive.
    pub fn end(self) -> i64 {
        self.next().start()
    }

    pub fn days(self) -> u32 {
        ((self.end() - self.start()) / DAY) as u32
    }

    /// Which day of this month a moment falls on, from 0 — `None` outside it.
    pub fn day_of(self, unix_seconds: i64) -> Option<u32> {
        if unix_seconds < self.start() || unix_seconds >= self.end() {
            return None;
        }

        Some(((unix_seconds - self.start()) / DAY) as u32)
    }

    /// The weekday of a day of this month, from 0, Monday first.
    pub fn weekday(self, day: u32) -> u32 {
        // 1970-01-01 was a Thursday — three days after a Monday.
        (days_from_civil(self.year, self.month, 1) + day as i64 + 3).rem_euclid(7) as u32
    }

    pub fn title(self) -> String {
        const NAMES: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];

        format!("{} {}", NAMES[(self.month - 1) as usize], self.year)
    }
}

/// Days since 1970-01-01 of a calendar date — Howard Hinnant's `days_from_civil`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = month as i64;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;

    era * 146_097 + doe - 719_468
}

/// The calendar date of a count of days since 1970-01-01 — the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };

    (year, month, day)
}

/// `2026-10-07` — the date a wall-clock moment falls on, which is what a tooltip on this grid says.
pub fn date_of(unix_seconds: i64) -> String {
    let (year, month, day) = civil_from_days(unix_seconds.div_euclid(DAY));
    format!("{year}-{month:02}-{day:02}")
}

/// One stretch of a row inside one month, in whole days.
///
/// A stretch covers every day it touches, both ends included — something started and finished on the same
/// afternoon is one day wide, not invisible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// The first day of the month the stretch covers, from 0.
    pub first: u32,
    /// The last day it covers, from 0, inclusive.
    pub last: u32,
    /// It was already going when the month began — it runs in from the left edge.
    pub cut_before: bool,
    /// It was still going when the month ended — it runs off the right edge.
    pub cut_after: bool,
    /// It has not ended, and today is in this month: it stops at today because that is as far as it has got.
    pub ongoing: bool,
}

/// The stretch from `from` to `until` inside `month`; `until` is `None` for one still going, which runs to
/// `now`.
pub fn span_in_month(month: Month, from: i64, until: Option<i64>, now: i64) -> Option<Span> {
    // Stamps that disagree — an end "before" the start, which only a clock can do — draw as the one day it
    // began on rather than as nothing.
    let to = until.unwrap_or(now).max(from);

    if to < month.start() || from >= month.end() {
        return None;
    }

    let cut_before = from < month.start();
    let cut_after = to >= month.end();

    let first = if cut_before {
        0
    } else {
        ((from - month.start()) / DAY) as u32
    };

    let last = if cut_after {
        month.days() - 1
    } else {
        ((to - month.start()) / DAY) as u32
    };

    Some(Span {
        first,
        last,
        cut_before,
        cut_after,
        ongoing: until.is_none() && !cut_after,
    })
}

/// When something was opened, where its bar begins and when it was closed — on the wall clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Life {
    pub opened: i64,
    /// Where the bar begins: the start on record — or, with none, the moment it was opened, which is the
    /// most the board knows.
    pub started: i64,
    pub closed: Option<i64>,
    /// Whether `started` is a start somebody recorded, rather than the opening standing in for one.
    pub start_on_record: bool,
}

impl Life {
    fn of(
        opened: i64,
        started: Option<i64>,
        closed: Option<i64>,
        wall: &dyn Fn(i64) -> i64,
    ) -> Self {
        Self {
            opened: wall(opened),
            started: wall(started.unwrap_or(opened)),
            closed: closed.map(wall),
            start_on_record: started.is_some(),
        }
    }

    /// Neither end on record: still open, and nobody has said when it started. Its bar — from when it was
    /// opened to today — is a guess, and is drawn as one: dashed and see-through, there to be seen and to
    /// be given its dates.
    pub fn tentative(&self) -> bool {
        !self.start_on_record && self.closed.is_none()
    }
}

pub fn goal_life(goal: &GoalResponse, wall: &dyn Fn(i64) -> i64) -> Life {
    Life::of(
        goal.created_unix_seconds,
        goal.started_unix_seconds,
        goal.closed_unix_seconds,
        wall,
    )
}

pub fn task_life(task: &TaskResponse, wall: &dyn Fn(i64) -> i64) -> Life {
    Life::of(
        task.created_unix_seconds,
        task.started_unix_seconds,
        task.closed_unix_seconds,
        wall,
    )
}

/// What a row draws in one month: the bar, and the wait before it.
///
/// `work` is the bar — from the start to the close, or to today while it goes on. `waiting` is the time
/// from being opened to a start somebody recorded, drawn as a dashed line: how long a thing sat before it
/// was taken up. `tentative` is [`Life::tentative`]: the bar is a guess.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Bars {
    pub waiting: Option<Span>,
    pub work: Option<Span>,
    pub tentative: bool,
}

impl Bars {
    pub fn is_empty(&self) -> bool {
        self.waiting.is_none() && self.work.is_none()
    }
}

pub fn bars_in_month(month: Month, life: Life, now: i64) -> Bars {
    let work = span_in_month(month, life.started, life.closed, now);

    // Only before a start somebody recorded, and only if it came after the opening — a start written down
    // after the fact can be earlier, and then nothing waited.
    let waiting = if life.start_on_record && life.started > life.opened {
        span_in_month(month, life.opened, Some(life.started), now)
    } else {
        None
    };

    // Counted in days the two meet on the day work began, and that day is a day of work: the wait stops
    // the day before, or is not drawn at all when there is no day before it in this month.
    let waiting = match (waiting, work) {
        (Some(mut waiting), Some(work)) if waiting.last >= work.first => {
            if work.first == 0 || waiting.first >= work.first {
                None
            } else {
                waiting.last = work.first - 1;
                waiting.cut_after = false;
                waiting.ongoing = false;
                Some(waiting)
            }
        }
        (waiting, _) => waiting,
    };

    Bars {
        waiting,
        work,
        tentative: life.tentative(),
    }
}

/// The goals the timeline draws: the live list, and the closed goals that have aged off it.
///
/// The live list is what a push keeps current, so whatever is on it wins. From the full history only what
/// the live list CANNOT carry is taken — a goal that is closed and not on it. A goal that was open when the
/// history was read and has since been deleted is open in that copy, so it is not brought back from there.
pub fn merge_with_archive(live: Vec<GoalResponse>, archive: &[GoalResponse]) -> Vec<GoalResponse> {
    let on_live: HashSet<String> = live.iter().map(|itm| itm.id.clone()).collect();

    let mut result = live;

    for goal in archive {
        if goal.deleted_unix_seconds.is_none()
            && goal.closed_unix_seconds.is_some()
            && !on_live.contains(&goal.id)
        {
            result.push(goal.clone());
        }
    }

    result
}

/// A goal's row: its stretches, and what happened on which day under it.
#[derive(Clone, PartialEq, Debug)]
pub struct GoalLane {
    pub goal: GoalResponse,
    pub status: GoalStatus,
    pub life: Life,
    pub bars: Bars,
    /// Whether its tasks are unfolded underneath.
    pub open: bool,
    /// Day → the releases that went out that day, by title. Ordered, so the marks come out left to right.
    pub releases: BTreeMap<u32, Vec<String>>,
    /// Day → the tasks of this goal done that day, by id.
    pub done: BTreeMap<u32, Vec<String>>,
}

/// A task's row, under its goal.
#[derive(Clone, PartialEq, Debug)]
pub struct TaskLane {
    pub task: TaskResponse,
    /// The palette name of the goal it is under: a task is drawn in its goal's colour.
    pub color: String,
    pub life: Life,
    pub bars: Bars,
}

/// One row of the chart, in the order they are drawn.
#[derive(Clone, PartialEq, Debug)]
pub enum Row {
    Goal(GoalLane),
    Task(TaskLane),
    /// An unfolded goal with nothing under it to draw in this month — said, so an open goal with no rows
    /// under it does not look like a goal that failed to open.
    Nothing {
        goal: String,
    },
}

impl Row {
    /// What the row is known by across a repaint.
    pub fn key(&self) -> String {
        match self {
            Self::Goal(lane) => lane.goal.id.clone(),
            Self::Task(lane) => lane.task.id.clone(),
            Self::Nothing { goal } => format!("{goal}-nothing"),
        }
    }
}

/// Where a row sorts: what has a start on record first, in the order it started — the waterfall a timeline
/// is read as — and everything with no start at the bottom, in the order it was opened. Finished work with
/// no start on record goes there too: its bar begins at the opening, which is not a start anybody gave.
fn waterfall(life: &Life, id: &str) -> (bool, i64, String) {
    (!life.start_on_record, life.started, id.to_string())
}

/// The rows of one month: each goal that has anything to draw in it, and under each unfolded one, its tasks
/// that do.
///
/// `now` is on the wall clock already; `wall` moves a stamp onto it — see [`Month`]. A release's date is
/// the one moment that does not go through it. `hide_done` leaves out the tasks that are done; which goals
/// are here at all is the caller's to decide.
pub fn lay_out(
    month: Month,
    goals: Vec<(GoalResponse, GoalStatus)>,
    of_goal: &HashMap<String, Vec<TaskResponse>>,
    expanded: &[String],
    hide_done: bool,
    now: i64,
    wall: &dyn Fn(i64) -> i64,
) -> Vec<Row> {
    let mut lanes: Vec<GoalLane> = goals
        .into_iter()
        .filter_map(|(goal, status)| {
            let life = goal_life(&goal, wall);
            let bars = bars_in_month(month, life, now);

            if bars.is_empty() {
                return None;
            }

            let mut releases: BTreeMap<u32, Vec<String>> = BTreeMap::new();

            for release in &goal.releases {
                if let Some(day) = month.day_of(release.date_unix_seconds) {
                    releases.entry(day).or_default().push(release.title.clone());
                }
            }

            let mut done: BTreeMap<u32, Vec<String>> = BTreeMap::new();

            for task in tasks_of(of_goal, &goal.id) {
                if let Some(day) = task
                    .closed_unix_seconds
                    .and_then(|at| month.day_of(wall(at)))
                {
                    done.entry(day).or_default().push(task.id.clone());
                }
            }

            Some(GoalLane {
                open: expanded.contains(&goal.id),
                goal,
                status,
                life,
                bars,
                releases,
                done,
            })
        })
        .collect();

    lanes.sort_by_key(|lane| waterfall(&lane.life, &lane.goal.id));

    let mut rows = Vec::new();

    for lane in lanes {
        let open = lane.open;
        let goal = lane.goal.id.clone();
        let color = lane.goal.color.clone();

        rows.push(Row::Goal(lane));

        if !open {
            continue;
        }

        let mut tasks: Vec<TaskLane> = tasks_of(of_goal, &goal)
            .iter()
            .filter(|task| !(hide_done && task.status == COLUMN_ID_DONE))
            .filter_map(|task| {
                let life = task_life(task, wall);
                let bars = bars_in_month(month, life, now);

                (!bars.is_empty()).then(|| TaskLane {
                    task: task.clone(),
                    color: color.clone(),
                    life,
                    bars,
                })
            })
            .collect();

        if tasks.is_empty() {
            rows.push(Row::Nothing { goal });
            continue;
        }

        tasks.sort_by_key(|lane| waterfall(&lane.life, &lane.task.id));
        rows.extend(tasks.into_iter().map(Row::Task));
    }

    rows
}

fn tasks_of<'a>(of_goal: &'a HashMap<String, Vec<TaskResponse>>, goal: &str) -> &'a [TaskResponse] {
    of_goal.get(goal).map(|itm| itm.as_slice()).unwrap_or(&[])
}

/// A unix stamp moved onto the reader's wall clock — see [`Month`].
///
/// The browser's offset AT that moment rather than today's, so a goal opened before the clocks went back
/// lands on the day it was opened on.
pub fn wall_clock(unix_seconds: i64) -> i64 {
    let at = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        unix_seconds as f64 * 1_000.0,
    ));

    // Minutes BEHIND UTC — negative east of Greenwich — hence the subtraction.
    unix_seconds - (at.get_timezone_offset() as i64) * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-07T00:00:00Z — a Wednesday.
    const OCT_7: i64 = 1_791_331_200;

    fn october() -> Month {
        Month {
            year: 2026,
            month: 10,
        }
    }

    fn goal(id: &str, opened: i64, closed: Option<i64>) -> GoalResponse {
        GoalResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            name: id.to_string(),
            description: String::new(),
            color: String::new(),
            priority: String::new(),
            status: "todo".to_string(),
            tasks_amount: 0,
            done_amount: 0,
            documents: Vec::new(),
            subtasks: Vec::new(),
            releases: Vec::new(),
            comments: Vec::new(),
            created_unix_seconds: opened,
            updated_unix_seconds: opened,
            started_unix_seconds: None,
            closed_unix_seconds: closed,
            deleted_unix_seconds: None,
        }
    }

    #[test]
    fn a_month_knows_its_bounds_its_length_and_its_weekdays() {
        let month = Month::of(OCT_7 + 15 * 3600);
        assert_eq!(month, october());
        assert_eq!(month.start(), OCT_7 - 6 * DAY);
        assert_eq!(month.days(), 31);
        assert_eq!(month.title(), "October 2026");
        assert_eq!(month.weekday(0), 3, "2026-10-01 is a Thursday");
        assert_eq!(month.weekday(6), 2, "the 7th is a Wednesday");

        let february = Month {
            year: 2028,
            month: 2,
        };
        assert_eq!(february.days(), 29, "2028 is a leap year");
        assert_eq!(
            Month {
                year: 2026,
                month: 2
            }
            .days(),
            28
        );
    }

    #[test]
    fn months_roll_over_the_year_both_ways() {
        let january = Month {
            year: 2027,
            month: 1,
        };
        assert_eq!(
            january.prev(),
            Month {
                year: 2026,
                month: 12
            }
        );
        assert_eq!(january.prev().next(), january);
        assert_eq!(january.prev().end(), january.start());
    }

    #[test]
    fn a_goal_inside_the_month_covers_both_its_end_days() {
        let span = span_in_month(
            october(),
            OCT_7 + 10 * 3600,
            Some(OCT_7 + 2 * DAY + 3600),
            0,
        )
        .unwrap();

        assert_eq!((span.first, span.last), (6, 8));
        assert!(!span.cut_before && !span.cut_after && !span.ongoing);

        let same_day = span_in_month(october(), OCT_7 + 3600, Some(OCT_7 + 7200), 0).unwrap();
        assert_eq!(
            (same_day.first, same_day.last),
            (6, 6),
            "opened and closed the same day is one day wide"
        );
    }

    #[test]
    fn an_open_goal_runs_to_today_and_off_the_edge_of_a_past_month() {
        let now = OCT_7 + 3 * DAY + 3600;
        let opened = OCT_7 - 40 * DAY;

        let this_month = span_in_month(october(), opened, None, now).unwrap();
        assert_eq!((this_month.first, this_month.last), (0, 9));
        assert!(this_month.cut_before && !this_month.cut_after && this_month.ongoing);

        let september = october().prev();
        let past = span_in_month(september, opened, None, now).unwrap();
        assert_eq!((past.first, past.last), (0, 29));
        assert!(
            past.cut_after && !past.ongoing,
            "in a past month it is not where it stops, it runs on"
        );

        assert!(
            span_in_month(october().next(), opened, None, now).is_none(),
            "an open goal has not reached a month that has not happened"
        );
    }

    #[test]
    fn a_goal_outside_the_month_is_not_on_it() {
        let september = october().prev();

        assert!(span_in_month(september, OCT_7, Some(OCT_7 + DAY), 0).is_none());
        assert!(span_in_month(october().next(), OCT_7, Some(OCT_7 + DAY), 0).is_none());
        assert!(
            span_in_month(october(), october().end(), None, october().end() + DAY).is_none(),
            "opened at midnight of the 1st of November is November's"
        );
    }

    #[test]
    fn the_archive_adds_only_what_the_live_list_cannot_carry() {
        let live = vec![goal("RMS-G2", OCT_7, None)];

        let mut stale_copy = goal("RMS-G2", OCT_7, None);
        stale_copy.name = "old name".to_string();

        let mut gone = goal("RMS-G4", OCT_7, Some(OCT_7 + DAY));
        gone.deleted_unix_seconds = Some(OCT_7 + 2 * DAY);

        let archive = [
            goal("RMS-G1", OCT_7 - 90 * DAY, Some(OCT_7 - 80 * DAY)),
            stale_copy,
            goal("RMS-G3", OCT_7, None),
            gone,
        ];

        let merged = merge_with_archive(live, &archive);
        let ids: Vec<&str> = merged.iter().map(|itm| itm.id.as_str()).collect();

        assert_eq!(ids, ["RMS-G2", "RMS-G1"]);
        assert_eq!(merged[0].name, "RMS-G2", "the live copy wins");
    }

    fn task(id: &str, goal: &str, status: &str, opened: i64) -> TaskResponse {
        TaskResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            text: id.to_string(),
            status: status.to_string(),
            priority: String::new(),
            kind: None,
            goal: Some(goal.to_string()),
            goal_name: None,
            goal_color: None,
            assignee: None,
            assignee_name: None,
            labels: Vec::new(),
            depends_on: Vec::new(),
            blocks: Vec::new(),
            link_statuses: Vec::new(),
            blocked: false,
            documents: Vec::new(),
            subtasks: Vec::new(),
            gh_actions: Vec::new(),
            comments: Vec::new(),
            created_unix_seconds: opened,
            updated_unix_seconds: opened,
            started_unix_seconds: None,
            closed_unix_seconds: None,
            deleted_unix_seconds: None,
        }
    }

    fn release(title: &str, date: i64) -> task_manager_shared::releases::ReleaseResponse {
        task_manager_shared::releases::ReleaseResponse {
            id: format!("RMS-R{title}"),
            project: "RMS".to_string(),
            title: title.to_string(),
            description: String::new(),
            release_notes: String::new(),
            date_unix_seconds: date,
            services: Vec::new(),
            goals: Vec::new(),
            envs: Vec::new(),
            done_unix_seconds: None,
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
        }
    }

    fn days(span: Option<Span>) -> Option<(u32, u32)> {
        span.map(|itm| (itm.first, itm.last))
    }

    /// Opened on the 1st, started on the 5th, closed on the 9th: four days of waiting, then the work —
    /// meeting, not overlapping, on the day work began.
    #[test]
    fn the_wait_runs_up_to_the_day_work_starts() {
        let first = october().start();
        let life = Life {
            opened: first + 9 * 3600,
            started: first + 4 * DAY + 10 * 3600,
            closed: Some(first + 8 * DAY + 3600),
            start_on_record: true,
        };

        let bars = bars_in_month(october(), life, OCT_7 + 10 * DAY);

        assert_eq!(days(bars.waiting), Some((0, 3)));
        assert_eq!(days(bars.work), Some((4, 8)));

        let same_day = Life {
            started: first + 15 * 3600,
            ..life
        };
        assert_eq!(
            bars_in_month(october(), same_day, OCT_7).waiting,
            None,
            "started the day it was opened: nothing waited"
        );
    }

    /// Neither a start nor an end on record: a bar all the same, from when it was opened to today — marked
    /// as a guess, so it can be seen and given its dates.
    #[test]
    fn what_has_no_dates_is_a_tentative_bar_up_to_today() {
        let life = Life {
            opened: OCT_7,
            started: OCT_7,
            closed: None,
            start_on_record: false,
        };

        let bars = bars_in_month(october(), life, OCT_7 + 3 * DAY);

        assert!(bars.tentative);
        assert_eq!(
            bars.waiting, None,
            "no start on record, so no wait before one"
        );
        assert_eq!(days(bars.work), Some((6, 9)));
        assert!(bars.work.unwrap().ongoing);
    }

    /// Opened in September, started in October: September shows only the wait, running off its edge.
    #[test]
    fn a_wait_that_crosses_a_month_ends_with_it() {
        let life = Life {
            opened: OCT_7 - 17 * DAY,
            started: OCT_7 - 4 * DAY,
            closed: None,
            start_on_record: true,
        };
        let now = OCT_7 + 3 * DAY;

        let september = bars_in_month(october().prev(), life, now);
        assert_eq!(days(september.waiting), Some((19, 29)));
        assert!(september.waiting.unwrap().cut_after);
        assert_eq!(september.work, None);

        let this_month = bars_in_month(october(), life, now);
        assert_eq!(days(this_month.waiting), Some((0, 1)));
        assert_eq!(days(this_month.work), Some((2, 9)));
    }

    /// With no start on record the bar begins where the thing was opened — the most the board knows —
    /// and is a guess for as long as there is no end on record either.
    #[test]
    fn with_no_start_on_record_the_bar_begins_at_the_opening() {
        let identity = |at: i64| at;

        let mut finished_long_ago = task("RMS-1", "RMS-G1", COLUMN_ID_DONE, OCT_7);
        finished_long_ago.closed_unix_seconds = Some(OCT_7 + 2 * DAY);
        let life = task_life(&finished_long_ago, &identity);
        assert_eq!((life.started, life.start_on_record), (OCT_7, false));
        assert!(
            !life.tentative(),
            "it has an end on record, so the bar is no guess"
        );

        let queued = task("RMS-2", "RMS-G1", "todo", OCT_7);
        assert!(task_life(&queued, &identity).tentative());

        let mut recorded = task("RMS-3", "RMS-G1", "in-progress", OCT_7);
        recorded.started_unix_seconds = Some(OCT_7 + DAY);
        let life = task_life(&recorded, &identity);
        assert_eq!((life.started, life.start_on_record), (OCT_7 + DAY, true));
        assert!(!life.tentative());

        let talked_about = goal("RMS-G1", OCT_7, None);
        assert!(goal_life(&talked_about, &identity).tentative());
    }

    #[test]
    fn goals_come_in_the_order_they_started_with_their_days_marked() {
        let mut late = goal("RMS-G1", OCT_7 + 5 * DAY, None);
        late.started_unix_seconds = Some(OCT_7 + 5 * DAY);
        late.releases.push(release("1.0", OCT_7 + 6 * DAY));

        let mut early = goal("RMS-G2", OCT_7 - 20 * DAY, Some(OCT_7 + DAY));
        early.started_unix_seconds = Some(OCT_7);

        let waiting = goal("RMS-G4", OCT_7 - 30 * DAY, None);
        let closed_with_no_start = goal("RMS-G6", OCT_7 - 2 * DAY, Some(OCT_7 + 2 * DAY));
        let elsewhere = goal("RMS-G3", OCT_7 - 60 * DAY, Some(OCT_7 - 50 * DAY));

        let mut closed_task = task(
            "RMS-5",
            "RMS-G1",
            task_manager_shared::projects::COLUMN_ID_DONE,
            OCT_7,
        );
        closed_task.closed_unix_seconds = Some(OCT_7 + 6 * DAY + 60);

        let of_goal = HashMap::from([("RMS-G1".to_string(), vec![closed_task])]);

        let rows = lay_out(
            october(),
            vec![
                (late, GoalStatus::InProgress),
                (waiting, GoalStatus::Todo),
                (closed_with_no_start, GoalStatus::Done),
                (early, GoalStatus::Done),
                (elsewhere, GoalStatus::Done),
            ],
            &of_goal,
            &[],
            false,
            OCT_7 + 10 * DAY,
            &|at| at,
        );

        let keys: Vec<String> = rows.iter().map(Row::key).collect();
        assert_eq!(
            keys,
            ["RMS-G2", "RMS-G1", "RMS-G4", "RMS-G6"],
            "in the order work began; with no start at the bottom, in the order opened — closed or not; and a goal of another month not at all"
        );

        let Row::Goal(marked) = &rows[1] else {
            panic!("a goal row");
        };
        assert_eq!(marked.releases.get(&12), Some(&vec!["1.0".to_string()]));
        assert_eq!(marked.done.get(&12), Some(&vec!["RMS-5".to_string()]));
        assert!(!marked.open, "folded unless somebody unfolded it");
    }

    /// Unfolding a goal puts its tasks under it, in the order they started; Hide done takes the finished
    /// ones out; and a goal with nothing left to show says so rather than opening onto nothing.
    #[test]
    fn an_unfolded_goal_lists_its_tasks_under_it() {
        let mut goal_row = goal("RMS-G1", OCT_7 - 10 * DAY, None);
        goal_row.started_unix_seconds = Some(OCT_7 - 9 * DAY);

        let mut done = task(
            "RMS-11",
            "RMS-G1",
            task_manager_shared::projects::COLUMN_ID_DONE,
            OCT_7 - 9 * DAY,
        );
        done.started_unix_seconds = Some(OCT_7 - 8 * DAY);
        done.closed_unix_seconds = Some(OCT_7 - 2 * DAY);

        let mut going = task("RMS-12", "RMS-G1", "in-progress", OCT_7 - 9 * DAY);
        going.started_unix_seconds = Some(OCT_7 - DAY);

        let queued = task("RMS-13", "RMS-G1", "todo", OCT_7);

        let of_goal = HashMap::from([(
            "RMS-G1".to_string(),
            vec![queued.clone(), going.clone(), done.clone()],
        )]);

        let lay = |hide_done: bool, of_goal: &HashMap<String, Vec<TaskResponse>>| -> Vec<String> {
            lay_out(
                october(),
                vec![(goal_row.clone(), GoalStatus::InProgress)],
                of_goal,
                &["RMS-G1".to_string()],
                hide_done,
                OCT_7 + 3 * DAY,
                &|at| at,
            )
            .iter()
            .map(Row::key)
            .collect()
        };

        assert_eq!(
            lay(false, &of_goal),
            ["RMS-G1", "RMS-11", "RMS-12", "RMS-13"]
        );
        assert_eq!(lay(true, &of_goal), ["RMS-G1", "RMS-12", "RMS-13"]);

        let only_done = HashMap::from([("RMS-G1".to_string(), vec![done])]);
        assert_eq!(lay(true, &only_done), ["RMS-G1", "RMS-G1-nothing"]);
    }
}
