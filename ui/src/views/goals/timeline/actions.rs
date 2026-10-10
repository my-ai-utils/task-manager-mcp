//! What the timeline is worked out from: the calendar, and where each goal's bar falls on it. No
//! rendering here — every function but [`wall_clock`] is plain arithmetic, tested natively below.

use std::collections::{BTreeMap, HashMap, HashSet};

use task_manager_shared::goals::GoalResponse;
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

/// The part of a goal's life that falls inside one month, in whole days.
///
/// A goal lives from when it was opened to when it was closed; an open one, until now. A bar is drawn over
/// every day it was alive on, both ends included — a goal opened and closed on the same afternoon is one day
/// wide, not invisible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// The first day of the month the bar covers, from 0.
    pub first: u32,
    /// The last day it covers, from 0, inclusive.
    pub last: u32,
    /// It was already open when the month began — the bar runs in from the left edge.
    pub cut_before: bool,
    /// It was still open when the month ended — the bar runs off the right edge.
    pub cut_after: bool,
    /// It is open, and today is in this month: the bar stops at today because that is as far as it has got.
    pub ongoing: bool,
}

pub fn span_in_month(month: Month, opened: i64, closed: Option<i64>, now: i64) -> Option<Span> {
    // A goal with no close moment is alive up to now. One whose stamps disagree — closed "before" it was
    // opened, which only a clock can do — is drawn as the day it was opened rather than as nothing.
    let until = closed.unwrap_or(now).max(opened);

    if until < month.start() || opened >= month.end() {
        return None;
    }

    let cut_before = opened < month.start();
    let cut_after = until >= month.end();

    let first = if cut_before {
        0
    } else {
        ((opened - month.start()) / DAY) as u32
    };

    let last = if cut_after {
        month.days() - 1
    } else {
        ((until - month.start()) / DAY) as u32
    };

    Some(Span {
        first,
        last,
        cut_before,
        cut_after,
        ongoing: closed.is_none() && !cut_after,
    })
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

/// One row of the chart: a goal, its bar, and what happened on which day under it.
#[derive(Clone, PartialEq, Debug)]
pub struct Lane {
    pub goal: GoalResponse,
    pub status: GoalStatus,
    pub span: Span,
    /// Day → the releases that went out that day, by title. Ordered, so the markers come out left to right.
    pub releases: BTreeMap<u32, Vec<String>>,
    /// Day → the tasks of this goal closed that day, by id.
    pub done: BTreeMap<u32, Vec<String>>,
}

/// The rows of one month, oldest goal first — the waterfall a timeline is read as.
///
/// `now` is on the wall clock already; `wall` moves a stamp onto it — see [`Month`]. A release's date is
/// the one moment that does not go through it.
pub fn lay_out(
    month: Month,
    goals: Vec<(GoalResponse, GoalStatus)>,
    of_goal: &HashMap<String, Vec<TaskResponse>>,
    now: i64,
    wall: &dyn Fn(i64) -> i64,
) -> Vec<Lane> {
    let mut lanes: Vec<Lane> = goals
        .into_iter()
        .filter_map(|(goal, status)| {
            let span = span_in_month(
                month,
                wall(goal.created_unix_seconds),
                goal.closed_unix_seconds.map(wall),
                now,
            )?;

            let mut releases: BTreeMap<u32, Vec<String>> = BTreeMap::new();

            for release in &goal.releases {
                if let Some(day) = month.day_of(release.date_unix_seconds) {
                    releases.entry(day).or_default().push(release.title.clone());
                }
            }

            let mut done: BTreeMap<u32, Vec<String>> = BTreeMap::new();

            for task in of_goal
                .get(&goal.id)
                .map(|itm| itm.as_slice())
                .unwrap_or(&[])
            {
                if let Some(day) = task
                    .closed_unix_seconds
                    .and_then(|at| month.day_of(wall(at)))
                {
                    done.entry(day).or_default().push(task.id.clone());
                }
            }

            Some(Lane {
                goal,
                status,
                span,
                releases,
                done,
            })
        })
        .collect();

    lanes.sort_by(|left, right| {
        left.goal
            .created_unix_seconds
            .cmp(&right.goal.created_unix_seconds)
            .then_with(|| left.goal.id.cmp(&right.goal.id))
    });

    lanes
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

    #[test]
    fn lanes_come_oldest_first_with_their_days_marked() {
        let mut late = goal("RMS-G1", OCT_7 + 5 * DAY, None);
        late.releases
            .push(task_manager_shared::releases::ReleaseResponse {
                id: "RMS-R1".to_string(),
                project: "RMS".to_string(),
                title: "1.0".to_string(),
                description: String::new(),
                release_notes: String::new(),
                date_unix_seconds: OCT_7 + 6 * DAY,
                services: Vec::new(),
                goals: Vec::new(),
                envs: Vec::new(),
                done_unix_seconds: None,
                comments: Vec::new(),
                created_unix_seconds: 0,
                updated_unix_seconds: 0,
                deleted_unix_seconds: None,
            });

        let early = goal("RMS-G2", OCT_7, Some(OCT_7 + DAY));
        let elsewhere = goal("RMS-G3", OCT_7 - 60 * DAY, Some(OCT_7 - 50 * DAY));

        let closed_task = TaskResponse {
            id: "RMS-5".to_string(),
            project: "RMS".to_string(),
            text: "RMS-5".to_string(),
            status: task_manager_shared::projects::COLUMN_ID_DONE.to_string(),
            priority: String::new(),
            kind: None,
            goal: Some("RMS-G1".to_string()),
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
            created_unix_seconds: OCT_7,
            updated_unix_seconds: OCT_7,
            closed_unix_seconds: Some(OCT_7 + 6 * DAY + 60),
            deleted_unix_seconds: None,
        };

        let of_goal = HashMap::from([("RMS-G1".to_string(), vec![closed_task])]);

        let lanes = lay_out(
            october(),
            vec![
                (late, GoalStatus::InProgress),
                (early, GoalStatus::Done),
                (elsewhere, GoalStatus::Done),
            ],
            &of_goal,
            OCT_7 + 10 * DAY,
            &|at| at,
        );

        let ids: Vec<&str> = lanes.iter().map(|itm| itm.goal.id.as_str()).collect();
        assert_eq!(
            ids,
            ["RMS-G2", "RMS-G1"],
            "oldest first, and a goal of another month is not here"
        );

        assert_eq!(lanes[1].releases.get(&12), Some(&vec!["1.0".to_string()]));
        assert_eq!(lanes[1].done.get(&12), Some(&vec!["RMS-5".to_string()]));
        assert!(lanes[0].releases.is_empty() && lanes[0].done.is_empty());
    }
}
