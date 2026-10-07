use std::collections::BTreeSet;

use rust_extensions::date_time::DateTimeAsMicroseconds;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::priority::Priority;
use task_manager_shared::projects::{COLUMN_ID_DONE, COLUMN_ID_TODO};

use super::{
    ARCHIVE_AFTER, Board, BoardInner, ColumnModel, ColumnTemplateModel, GoalModel, KindModel,
    KindTemplateModel, ProjectModel, ReleaseModel, TaskModel, UserModel,
};

const TEMPLATE_ID: &str = "tpl";
const KIND_TEMPLATE_ID: &str = "kinds-tpl";

fn kind_template() -> KindTemplateModel {
    KindTemplateModel {
        id: KIND_TEMPLATE_ID.to_string(),
        name: "Default".to_string(),
        description: String::new(),
        kinds: vec![KindModel {
            id: "bug".to_string(),
            name: "Bug".to_string(),
            description: String::new(),
            color: KindColor::Red,
            icon: "bug".to_string(),
        }],
        created: DateTimeAsMicroseconds::new(0),
    }
}

fn template() -> ColumnTemplateModel {
    ColumnTemplateModel {
        id: TEMPLATE_ID.to_string(),
        name: "Default".to_string(),
        description: String::new(),
        columns: vec![ColumnModel {
            id: "in-progress".to_string(),
            name: "In progress".to_string(),
            description: String::new(),
            order: 10,
        }],
        created: DateTimeAsMicroseconds::new(0),
    }
}

/// A board with the fixture template already installed.
///
/// Every test that puts a project in needs it: a project's columns come from its template, so a board
/// without the template gives every project a bare Todo -> Done board.
fn board() -> Board {
    let board = Board::new();
    board.upsert_column_template(template());
    board.upsert_kind_template(kind_template());
    board
}

fn project(id: &str, prefix: &str, history: &[&str]) -> ProjectModel {
    ProjectModel {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        prefix: prefix.to_string(),
        prefix_history: history.iter().map(|itm| itm.to_string()).collect(),
        column_template_id: Some(TEMPLATE_ID.to_string()),
        kind_template_id: Some(KIND_TEMPLATE_ID.to_string()),
        // Both left empty: the board resolves them from the templates. Setting them here would be a lie
        // the very first rebuild overwrites.
        columns: Vec::new(),
        kinds: Vec::new(),
        members: BTreeSet::new(),
        last_task_number: 0,
        // No window of its own, so the tests measure the seven-day default.
        archive_days: None,
        // Live. The archiving tests below put one away by hand — everything else wants a live project.
        archived_moment: None,
        github_connections: Vec::new(),
        created: DateTimeAsMicroseconds::new(0),
    }
}

fn task(project_id: &str, number: i64, status: &str, depends_on: &[i64]) -> TaskModel {
    TaskModel {
        project_id: project_id.to_string(),
        number,
        text: format!("task {number}"),
        status: status.to_string(),
        // Everything the ordering tests do not care about is Normal, so a list that comes back in number
        // order is the tiebreaker working rather than the priority sort being absent.
        priority: Priority::default(),
        kind: None,
        goal_number: None,
        assignee: None,
        labels: Vec::new(),
        depends_on: depends_on.to_vec(),
        // Nothing here reads a checklist — that is the point of it — so every fixture leaves it empty. Nor a
        // document reference: nothing on the board derives anything from one either, and the documents
        // themselves are not in memory at all.
        subtasks: Vec::new(),
        documents: Vec::new(),
        // Nor a build link, for the third time and the same reason: nothing on the board derives anything
        // from one — it is a record hung on the task, not a fact the board reads.
        gh_actions: Vec::new(),
        comments: Vec::new(),
        created: DateTimeAsMicroseconds::new(0),
        updated: DateTimeAsMicroseconds::new(0),
        close_moment: None,
        deleted_moment: None,
    }
}

fn goal(project_id: &str, number: i64) -> GoalModel {
    GoalModel {
        project_id: project_id.to_string(),
        number,
        name: format!("goal {number}"),
        description: String::new(),
        color: KindColor::default(),
        priority: Priority::default(),
        subtasks: Vec::new(),
        documents: Vec::new(),
        // Shipped in nothing. The release tests below attach them by hand.
        releases: Vec::new(),
        comments: Vec::new(),
        created: DateTimeAsMicroseconds::new(0),
        updated: DateTimeAsMicroseconds::new(0),
        close_moment: None,
        deleted_moment: None,
    }
}

/// A release dated `day` days after the epoch — the tests order by date, so the fixture takes one.
fn release(project_id: &str, number: i64, day: i64) -> ReleaseModel {
    ReleaseModel {
        project_id: project_id.to_string(),
        number,
        title: format!("release {number}"),
        description: String::new(),
        release_notes: String::new(),
        date: DateTimeAsMicroseconds::new(day * 24 * 60 * 60 * 1_000_000),
        services: Vec::new(),
        // Not on production and nothing said about it: neither is something the board derives from.
        released_on_prod_moment: None,
        comments: Vec::new(),
        created: DateTimeAsMicroseconds::new(0),
        updated: DateTimeAsMicroseconds::new(0),
        deleted_moment: None,
    }
}

/// The same task, marked deleted. A fixture rather than a field on `task()` because most tests want work that
/// exists, and the ones that do not should say so at the call site.
fn deleted(mut task: TaskModel) -> TaskModel {
    task.deleted_moment = Some(DateTimeAsMicroseconds::new(1));
    task
}

fn user(email: &str, name: &str) -> UserModel {
    UserModel {
        email: email.to_string(),
        name: name.to_string(),
        admin: false,
        disabled: false,
        created: DateTimeAsMicroseconds::new(0),
    }
}

/// The whole point of `blocked`: derived on every read, and true while any dependency is not Done.
#[test]
fn blocked_is_derived_against_the_current_statuses() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_task(task("p", 1, COLUMN_ID_TODO, &[]));
    board.upsert_task(task("p", 2, COLUMN_ID_DONE, &[]));

    let read = board.read();

    // Depends on an open task -> blocked.
    assert!(read.is_blocked(&task("p", 10, COLUMN_ID_TODO, &[1])));
    // Depends only on a Done task -> not blocked.
    assert!(!read.is_blocked(&task("p", 11, COLUMN_ID_TODO, &[2])));
    // One open blocker among Done ones is still enough.
    assert!(read.is_blocked(&task("p", 12, COLUMN_ID_TODO, &[2, 1])));
    // No dependencies -> never blocked.
    assert!(!read.is_blocked(&task("p", 13, COLUMN_ID_TODO, &[])));
}

/// A blocker that does not exist must keep the task blocked. The alternative — treating "not found"
/// as satisfied — frees a task because of a typo, silently.
#[test]
fn an_unknown_blocker_still_blocks() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    assert!(
        board
            .read()
            .is_blocked(&task("p", 1, COLUMN_ID_TODO, &[999]))
    );
}

/// Closing the last blocker unblocks the dependent with nothing written by hand — that is what
/// "derived, never stored" buys.
#[test]
fn closing_the_last_blocker_unblocks_on_the_next_read() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_task(task("p", 1, COLUMN_ID_TODO, &[]));
    board.upsert_task(task("p", 2, COLUMN_ID_TODO, &[1]));

    let dependent = board.read().get_task("p", 2).unwrap();
    assert!(board.read().is_blocked(&dependent));

    board.upsert_task(task("p", 1, COLUMN_ID_DONE, &[]));

    assert!(!board.read().is_blocked(&dependent));
}

/// A task in a deleted column reads as Todo — so it also stops counting as Done, and anything
/// depending on it goes back to blocked. Worth pinning: it is the interaction between two lenient
/// rules, and getting it wrong would mark work unblocked because a column was tidied away.
#[test]
fn a_blocker_whose_column_was_deleted_blocks_again() {
    let board = board();
    // `archived` is not among the project's columns, so a task sitting there reads as Todo.
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_task(task("p", 1, "archived", &[]));

    assert!(board.read().is_blocked(&task("p", 2, COLUMN_ID_TODO, &[1])));
}

#[test]
fn blocks_is_the_reverse_edge_and_is_sorted() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_task(task("p", 1, COLUMN_ID_TODO, &[]));
    board.upsert_task(task("p", 3, COLUMN_ID_TODO, &[1]));
    board.upsert_task(task("p", 2, COLUMN_ID_TODO, &[1]));
    board.upsert_task(task("p", 4, COLUMN_ID_TODO, &[]));

    let read = board.read();

    assert_eq!(read.blocks("p", 1), vec![2, 3]);
    assert!(read.blocks("p", 4).is_empty());
    // The two directions are independent: 2 is blocked by 1 and blocks nothing.
    assert!(read.blocks("p", 2).is_empty());
}

/// A stored status naming a column the project no longer has reads as Todo, and the stored value is
/// left alone — so re-creating the column brings the task back to it.
#[test]
fn an_unknown_status_reads_as_todo_without_being_rewritten() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_task(task("p", 1, "gone", &[]));

    let read = board.read();
    let project = read.get_project("p").unwrap();
    let stored = read.get_task("p", 1).unwrap();

    assert_eq!(project.effective_status(&stored.status), COLUMN_ID_TODO);
    assert_eq!(stored.status, "gone", "the stored value must survive");
    assert_eq!(project.effective_status("in-progress"), "in-progress");
    assert_eq!(project.effective_status(COLUMN_ID_DONE), COLUMN_ID_DONE);
}

#[test]
fn an_unknown_kind_reads_as_no_kind() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let project = board.read().get_project("p").unwrap();

    assert_eq!(project.effective_kind(Some("bug")), Some("bug".to_string()));
    assert_eq!(project.effective_kind(Some("gone")), None);
    assert_eq!(project.effective_kind(None), None);
}

/// The current holder is found by prefix; the history index reports everyone who ever held it. This
/// is what `tasks_resolve_id` reads.
#[test]
fn a_prefix_resolves_to_its_current_holder_and_remembers_the_past_ones() {
    let board = board();
    board.upsert_project(project("a", "TM", &["RMS"]));
    board.upsert_project(project("b", "RMS", &[]));

    let read = board.read();

    assert_eq!(read.get_project_by_prefix("RMS").unwrap().id, "b");
    assert_eq!(read.get_project_by_prefix("TM").unwrap().id, "a");
    assert!(read.get_project_by_prefix("NOPE").is_none());

    let ever: Vec<String> = read
        .projects_ever_holding_prefix("RMS")
        .iter()
        .map(|itm| itm.id.clone())
        .collect();
    assert_eq!(ever, vec!["a".to_string(), "b".to_string()]);
}

/// Free means "nobody holds it *now*". A prefix only in some project's history is takeable — which
/// is precisely why a task's handle is composed on read rather than stored.
#[test]
fn a_prefix_only_in_history_is_free_to_take() {
    let board = board();
    board.upsert_project(project("a", "TM", &["RMS"]));

    let read = board.read();

    assert!(read.is_prefix_free("RMS", None));
    assert!(!read.is_prefix_free("TM", None));
    // A project renaming itself does not collide with its own current prefix.
    assert!(read.is_prefix_free("TM", Some("a")));
}

/// Archiving a project takes it out of the pickers and out of NOTHING the board does.
///
/// Every assertion here is a link that must keep working, written down so a later filter added in the
/// wrong place fails a test instead of silently 404-ing somebody's bookmark. The board deliberately has no
/// opinion about archiving at all: it is the readers — the three dropdowns and the MCP listing — that
/// leave an archived project out, and each of them does it at the point of display.
#[test]
fn an_archived_project_is_hidden_by_its_readers_and_by_nothing_on_the_board() {
    let board = board();

    let mut put_away = project("a", "TM", &["OLD"]);
    put_away.archived_moment = Some(DateTimeAsMicroseconds::new(0));
    put_away.members.insert("yuri@mxtm.ai".to_string());
    board.upsert_project(put_away);

    let read = board.read();

    let found = read.get_project_by_prefix("TM").expect("still resolves");
    assert!(found.is_archived());
    // The direct link by internal id, which is what every per-request auth gate goes through.
    assert!(read.get_project("a").is_some());
    // And the old handle: archiving must not cost `OLD-42` its answer.
    assert_eq!(read.projects_ever_holding_prefix("OLD").len(), 1);

    // THE PREFIX IS STILL HELD. If archiving freed it, a new project could take `TM`, and every link into
    // the archived board would quietly start landing on somebody else's.
    assert!(!read.is_prefix_free("TM", None));

    // Membership is unchanged, so the list endpoint keeps returning it — carrying `archived: true` for the
    // readers to act on. Hiding it here would take it off the setup screen too, which is the one place it
    // can be brought back from.
    assert_eq!(read.projects_visible_to("yuri@mxtm.ai", false).len(), 1);
    assert_eq!(read.projects_visible_to("nobody@mxtm.ai", true).len(), 1);

    // It still follows its template, so the template still cannot be deleted out from under it — a board a
    // link still opens must not lose its columns.
    assert_eq!(read.count_projects_using_template(TEMPLATE_ID), 1);
}

/// A live project answers `is_archived` with false whatever else is true of it — the pairing test for the
/// one above, so neither direction can rot on its own.
#[test]
fn a_project_that_was_never_put_away_is_not_archived() {
    let board = board();
    board.upsert_project(project("a", "TM", &[]));

    assert!(!board.read().get_project("a").unwrap().is_archived());
}

#[test]
fn the_counter_only_moves_forward_and_is_per_project() {
    let board = board();
    board.upsert_project(project("a", "AAA", &[]));
    board.upsert_project(project("b", "BBB", &[]));

    assert_eq!(board.reserve_task_number("a"), Some(1));
    assert_eq!(board.reserve_task_number("a"), Some(2));
    assert_eq!(board.reserve_task_number("b"), Some(1));

    // Deleting is a flag now, so the row does not even leave — which makes the promise easier to keep, not
    // harder. The counter is what guarantees it either way: it only ever moves forward.
    board.upsert_task(deleted(task("a", 2, COLUMN_ID_TODO, &[])));

    assert_eq!(
        board.reserve_task_number("a"),
        Some(3),
        "a deleted number must not be handed out again"
    );

    assert_eq!(board.reserve_task_number("missing"), None);
}

#[test]
fn a_projects_labels_are_the_distinct_labels_its_tasks_carry() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let mut first = task("p", 1, COLUMN_ID_TODO, &[]);
    first.labels = vec!["ui".to_string(), "mt4".to_string()];
    board.upsert_task(first);

    let mut second = task("p", 2, COLUMN_ID_TODO, &[]);
    second.labels = vec!["mt4".to_string()];
    board.upsert_task(second);

    assert_eq!(
        board.read().labels_of_project("p"),
        vec!["mt4".to_string(), "ui".to_string()]
    );

    // Dropping it from the last task carrying it removes the label from the vocabulary — there is no labels
    // table for it to linger in. A DELETED task wears nothing, which is what this checks: the rows are still
    // there and the labels are gone with them.
    let mut first = task("p", 1, COLUMN_ID_TODO, &[]);
    first.labels = vec!["ui".to_string(), "mt4".to_string()];
    board.upsert_task(deleted(first));

    let mut second = task("p", 2, COLUMN_ID_TODO, &[]);
    second.labels = vec!["mt4".to_string()];
    board.upsert_task(deleted(second));

    assert!(board.read().labels_of_project("p").is_empty());
}

#[test]
fn visibility_follows_membership_and_an_admin_sees_everything() {
    let board = board();

    let mut with_member = project("a", "AAA", &[]);
    with_member.members.insert("yuri@mxtm.ai".to_string());
    board.upsert_project(with_member);
    board.upsert_project(project("b", "BBB", &[]));

    let read = board.read();

    let visible: Vec<String> = read
        .projects_visible_to("yuri@mxtm.ai", false)
        .iter()
        .map(|itm| itm.id.clone())
        .collect();
    assert_eq!(visible, vec!["a".to_string()]);

    assert!(read.projects_visible_to("nobody@mxtm.ai", false).is_empty());
    assert_eq!(read.projects_visible_to("nobody@mxtm.ai", true).len(), 2);

    // Case and stray space come from hand-typed membership lists, not from a bug.
    assert_eq!(read.projects_visible_to(" YURI@MXTM.AI ", false).len(), 1);
}

/// An assignee with no user row, and `claude`, have no display name — the caller shows the raw value
/// rather than inventing one.
#[test]
fn a_display_name_resolves_only_for_a_known_user_with_a_name() {
    let board = board();
    board.upsert_user(user("yuri@mxtm.ai", "Yuri"));
    board.upsert_user(user("noname@mxtm.ai", ""));

    let read = board.read();

    assert_eq!(
        read.display_name_of("yuri@mxtm.ai"),
        Some("Yuri".to_string())
    );
    assert_eq!(read.display_name_of("noname@mxtm.ai"), None);
    // The reserved assignee resolves to itself rather than to nothing: it has no user row by design, and
    // a sticker showing whatever case an agent typed would be worse than showing "AI".
    assert_eq!(read.display_name_of("AI"), Some("AI".to_string()));
    assert_eq!(read.display_name_of("ai"), Some("AI".to_string()));
    assert_eq!(read.display_name_of("nobody@x.io"), None);
    assert_eq!(read.display_name_of("stranger@mxtm.ai"), None);
}

/// Closed work leaves the board after the window, and nothing else does. Getting this wrong either buries
/// the board under years of finished tasks or hides work that is still live.
#[test]
fn only_work_closed_longer_ago_than_the_window_is_archived() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let now = DateTimeAsMicroseconds::now();
    let window = ARCHIVE_AFTER.as_micros() as i64;

    let mut just_closed = task("p", 1, COLUMN_ID_DONE, &[]);
    just_closed.close_moment = Some(DateTimeAsMicroseconds::new(
        now.unix_microseconds - 60_000_000,
    ));

    let mut long_closed = task("p", 2, COLUMN_ID_DONE, &[]);
    long_closed.close_moment = Some(DateTimeAsMicroseconds::new(
        now.unix_microseconds - window - 60_000_000,
    ));

    // A task still being worked on is never archived, however old it is.
    let mut old_and_open = task("p", 3, COLUMN_ID_TODO, &[]);
    old_and_open.created = DateTimeAsMicroseconds::new(0);

    board.upsert_task(just_closed.clone());
    board.upsert_task(long_closed.clone());
    board.upsert_task(old_and_open.clone());

    let read = board.read();

    assert!(
        !read.is_archived(&just_closed),
        "closed a minute ago is live"
    );
    assert!(
        read.is_archived(&long_closed),
        "closed past the window is archived"
    );
    assert!(!read.is_archived(&old_and_open), "open work never archives");
}

/// A task sitting in Done with no close moment must not vanish. It should not happen — the moment is
/// written on the way in — but being lenient means a gap in the data cannot silently swallow work.
#[test]
fn done_without_a_close_moment_stays_visible() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let orphan = task("p", 1, COLUMN_ID_DONE, &[]);
    board.upsert_task(orphan.clone());

    assert!(orphan.close_moment.is_none());
    assert!(!board.read().is_archived(&orphan));
}

/// The archive window is measured against the EFFECTIVE status, so a task whose column was deleted reads
/// as Todo and therefore cannot be archived — even if it still carries an old close moment.
#[test]
fn a_task_whose_column_was_deleted_is_not_archived() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let mut orphaned_column = task("p", 1, "gone", &[]);
    orphaned_column.close_moment = Some(DateTimeAsMicroseconds::new(1));

    board.upsert_task(orphaned_column.clone());

    assert!(!board.read().is_archived(&orphaned_column));
}

/// Columns come from the template, and a project following none has a bare Todo -> Done board.
///
/// The whole point of the indirection, and the case that used to be impossible: a project with no
/// columns configured is legitimate rather than broken.
#[test]
fn a_project_following_no_template_has_no_middle_columns() {
    let board = board();

    let mut without = project("p", "RMS", &[]);
    without.column_template_id = None;
    board.upsert_project(without);

    let project = board.read().get_project("p").unwrap();

    assert!(project.columns.is_empty());
    assert!(project.has_column(COLUMN_ID_TODO));
    assert!(project.has_column(COLUMN_ID_DONE));
    assert!(!project.has_column("in-progress"));
}

/// Editing a template moves every project that follows it, in the same swap.
///
/// This is what the resolved-on-rebuild cache buys, and what would silently rot if a project kept its
/// own copy of the columns instead.
#[test]
fn editing_a_template_changes_every_project_following_it() {
    let board = board();
    board.upsert_project(project("one", "AAA", &[]));
    board.upsert_project(project("two", "BBB", &[]));

    let mut edited = template();
    edited.columns.push(ColumnModel {
        id: "review".to_string(),
        name: "Review".to_string(),
        description: String::new(),
        order: 20,
    });
    board.upsert_column_template(edited);

    let read = board.read();

    for id in ["one", "two"] {
        let project = read.get_project(id).unwrap();

        assert_eq!(
            project
                .columns
                .iter()
                .map(|itm| itm.id.as_str())
                .collect::<Vec<&str>>(),
            vec!["in-progress", "review"],
            "project {id} should follow the edited template"
        );
    }
}

/// A task parked in a column the template no longer has reads as Todo, and its stored status survives —
/// so putting the column back brings it home. Same rule a deleted column always had.
#[test]
fn dropping_a_column_from_a_template_parks_its_tasks_in_todo() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_task(task("p", 1, "in-progress", &[]));

    let mut emptied = template();
    emptied.columns.clear();
    board.upsert_column_template(emptied);

    let read = board.read();
    let project = read.get_project("p").unwrap();
    let stored = read.get_task("p", 1).unwrap();

    assert_eq!(
        stored.status, "in-progress",
        "the stored value must survive"
    );
    assert_eq!(project.effective_status(&stored.status), COLUMN_ID_TODO);

    // Put it back: the task returns to the column it never actually left.
    board.upsert_column_template(template());

    let read = board.read();
    let project = read.get_project("p").unwrap();
    assert_eq!(project.effective_status("in-progress"), "in-progress");
}

/// The count that makes a template refusable to delete, and shows an edit's blast radius.
#[test]
fn a_template_knows_how_many_projects_follow_it() {
    let board = board();
    board.upsert_project(project("one", "AAA", &[]));
    board.upsert_project(project("two", "BBB", &[]));

    let mut alone = project("three", "CCC", &[]);
    alone.column_template_id = None;
    board.upsert_project(alone);

    let read = board.read();

    assert_eq!(read.count_projects_using_template(TEMPLATE_ID), 2);
    assert_eq!(
        read.count_projects_using_template("nothing-follows-this"),
        0
    );
}

/// Tasks and goals draw from ONE counter, so the startup floor has to account for both.
///
/// Without the goals half, a counter row that had fallen behind would hand a task the number a goal
/// already holds — and then `RMS-7` and `RMS-G7` would both exist, which is the single assumption the
/// handles are built on.
#[test]
fn the_counter_floor_counts_goals_as_well_as_tasks() {
    let mut behind = project("p", "RMS", &[]);
    behind.last_task_number = 1;

    let inner = BoardInner::from_loaded(
        vec![behind],
        vec![task("p", 2, COLUMN_ID_TODO, &[])],
        Vec::new(),
        vec![template()],
        vec![kind_template()],
        vec![goal("p", 9)],
        Vec::new(),
    );

    assert_eq!(
        inner.get_project("p").unwrap().last_task_number,
        9,
        "the floor is the highest number in use, whichever kind of thing holds it"
    );
}

/// Releases are the third thing to draw from that counter, so they hold the floor up too — a release is
/// very often the newest thing on a project, which makes it the row most likely to carry the highest
/// number.
#[test]
fn the_counter_floor_counts_releases_too() {
    let mut behind = project("p", "RMS", &[]);
    behind.last_task_number = 1;

    let inner = BoardInner::from_loaded(
        vec![behind],
        vec![task("p", 2, COLUMN_ID_TODO, &[])],
        Vec::new(),
        vec![template()],
        vec![kind_template()],
        vec![goal("p", 9)],
        vec![release("p", 14, 1)],
    );

    assert_eq!(inner.get_project("p").unwrap().last_task_number, 14);
}

/// Newest first by the DATE of the release, not by the order they were written down in: a release
/// recorded late, with an earlier date, belongs below the ones that went out after it.
#[test]
fn releases_come_back_newest_first_by_their_date() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    board.upsert_release(release("p", 1, 10));
    board.upsert_release(release("p", 2, 30));
    // Written down last, went out first.
    board.upsert_release(release("p", 3, 5));
    // Two on one day: the later number is the one recorded later, so it reads as the newer.
    board.upsert_release(release("p", 4, 30));

    let read = board.read();

    let numbers: Vec<i64> = read
        .releases_of_project("p")
        .iter()
        .map(|itm| itm.number)
        .collect();

    assert_eq!(numbers, vec![4, 2, 1, 3]);
}

/// A deleted release leaves every list, and the goal that lists it is NOT edited: the number stays where
/// it was, is read past, and shows again the moment the release comes back. An undo that had to remember
/// which goals to re-attach to would not be an undo.
#[test]
fn a_goal_reads_past_a_release_that_is_deleted_or_not_there() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    board.upsert_release(release("p", 2, 10));
    board.upsert_release(release("p", 3, 20));

    let mut deleted_release = release("p", 4, 30);
    deleted_release.deleted_moment = Some(DateTimeAsMicroseconds::new(1));
    board.upsert_release(deleted_release.clone());

    let mut shipped = goal("p", 1);
    // 99 names nothing at all — a gap in the data reads as "not there" rather than as an error.
    shipped.releases = vec![2, 4, 99, 3];
    board.upsert_goal(shipped.clone());

    let read = board.read();

    let listed: Vec<i64> = read
        .releases_of_goal(&shipped)
        .iter()
        .map(|itm| itm.number)
        .collect();

    assert_eq!(listed, vec![3, 2], "newest first, the deleted one skipped");
    assert!(read.get_release("p", 4).is_none());
    assert!(
        read.get_release_including_deleted("p", 4).is_some(),
        "and it is still there for whoever names it"
    );
    assert_eq!(read.releases_of_project("p").len(), 2);
    assert_eq!(read.releases_of_project_including_deleted("p").len(), 3);

    // Brought back: nothing about the goal changes, and it lists the release again.
    deleted_release.deleted_moment = None;
    board.upsert_release(deleted_release);

    let read = board.read();

    assert_eq!(read.releases_of_goal(&shipped).len(), 3);
}

/// The other direction, which nothing stores: a release does not know what it shipped, so the goals are
/// found by asking each one. Normally exactly one answers.
#[test]
fn the_goals_of_a_release_are_the_ones_that_list_it() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_release(release("p", 10, 1));
    board.upsert_release(release("p", 11, 2));

    let mut feature = goal("p", 1);
    feature.releases = vec![10];

    // Nothing stops a second goal listing the same release.
    let mut also = goal("p", 2);
    also.releases = vec![10, 11];

    // A deleted goal lists it too, and does not count: nothing can open it.
    let mut gone = goal("p", 3);
    gone.releases = vec![10];
    gone.deleted_moment = Some(DateTimeAsMicroseconds::new(1));

    board.upsert_goal(feature);
    board.upsert_goal(also);
    board.upsert_goal(gone);

    let read = board.read();

    let of = |number: i64| -> Vec<i64> {
        read.goals_of_release("p", number)
            .iter()
            .map(|itm| itm.number)
            .collect()
    };

    assert_eq!(of(10), vec![1, 2]);
    assert_eq!(of(11), vec![2]);
    assert!(of(12).is_empty(), "attached to nothing is a normal state");
}

/// A goal's progress counts archived work. A goal only closes once every task is done, and by then the
/// oldest of them have aged off the board — a count that skipped those would report finished work as
/// half-finished, and the goal would read as abandoned rather than landed.
#[test]
fn goal_progress_counts_archived_tasks() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_goal(goal("p", 1));

    let now = DateTimeAsMicroseconds::now();
    let window = ARCHIVE_AFTER.as_micros() as i64;

    let mut long_closed = task("p", 2, COLUMN_ID_DONE, &[]);
    long_closed.goal_number = Some(1);
    long_closed.close_moment = Some(DateTimeAsMicroseconds::new(
        now.unix_microseconds - window - 60_000_000,
    ));

    let mut open = task("p", 3, COLUMN_ID_TODO, &[]);
    open.goal_number = Some(1);

    // Somebody else's work: it must not be counted at all.
    board.upsert_task(task("p", 4, COLUMN_ID_DONE, &[]));

    board.upsert_task(long_closed.clone());
    board.upsert_task(open);

    let read = board.read();

    assert!(read.is_archived(&long_closed), "the fixture is archived");
    assert_eq!(read.goal_progress("p", 1), (2, 1));
    assert_eq!(read.tasks_of_goal("p", 1).len(), 2);
}

/// The list under a goal and the counter beside it have to agree, so both include archived work — and
/// neither includes a task pointing at a different goal.
#[test]
fn open_tasks_of_a_goal_are_what_blocks_closing_it() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_goal(goal("p", 1));

    let mut done = task("p", 2, COLUMN_ID_DONE, &[]);
    done.goal_number = Some(1);

    let mut open = task("p", 3, "in-progress", &[]);
    open.goal_number = Some(1);

    board.upsert_task(done);
    board.upsert_task(open);

    let read = board.read();
    let blocking = read.open_tasks_of_goal("p", 1);

    assert_eq!(blocking.len(), 1);
    assert_eq!(blocking[0].number, 3);
}

/// A goal archives on the same clock a task does, and an OPEN goal never archives however old it is — an
/// epic that has run for a year is late, not history.
#[test]
fn a_goal_archives_by_its_close_moment_and_the_projects_window() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let now = DateTimeAsMicroseconds::now();
    let window = ARCHIVE_AFTER.as_micros() as i64;

    let mut just_closed = goal("p", 1);
    just_closed.close_moment = Some(DateTimeAsMicroseconds::new(
        now.unix_microseconds - 60_000_000,
    ));

    let mut long_closed = goal("p", 2);
    long_closed.close_moment = Some(DateTimeAsMicroseconds::new(
        now.unix_microseconds - window - 60_000_000,
    ));

    let ancient_but_open = goal("p", 3);

    let read = board.read();

    assert!(!read.is_goal_archived(&just_closed));
    assert!(read.is_goal_archived(&long_closed));
    assert!(!read.is_goal_archived(&ancient_but_open));
}

/// The window is the project's, not a constant. A project that says two days archives work the default
/// seven-day window would still be showing.
#[test]
fn a_projects_own_archive_window_is_what_counts() {
    let board = board();

    let mut impatient = project("p", "RMS", &[]);
    impatient.archive_days = Some(2);
    board.upsert_project(impatient);

    let now = DateTimeAsMicroseconds::now();
    let three_days = 3 * 24 * 60 * 60 * 1_000_000_i64;

    let mut closed_three_days_ago = task("p", 1, COLUMN_ID_DONE, &[]);
    closed_three_days_ago.close_moment = Some(DateTimeAsMicroseconds::new(
        now.unix_microseconds - three_days,
    ));

    board.upsert_task(closed_three_days_ago.clone());

    assert!(
        board.read().is_archived(&closed_three_days_ago),
        "three days is past a two-day window, even though it is inside the default seven"
    );
}

/// A window that makes no sense reads as the default. Writes validate; the read side is lenient, so one
/// bad row cannot empty a board.
#[test]
fn a_nonsense_archive_window_falls_back_to_the_default() {
    let mut zero = project("p", "RMS", &[]);
    zero.archive_days = Some(0);
    assert_eq!(zero.archive_after(), ARCHIVE_AFTER);

    let mut negative = project("p", "RMS", &[]);
    negative.archive_days = Some(-5);
    assert_eq!(negative.archive_after(), ARCHIVE_AFTER);

    let mut two = project("p", "RMS", &[]);
    two.archive_days = Some(2);
    assert_eq!(two.archive_after().as_secs(), 2 * 24 * 60 * 60);
}

/// The order IS the feature. A column is drawn straight off this list, so the most urgent has to arrive
/// first — and within one priority the board's old oldest-first order has to survive, or cards would
/// shuffle on every repaint for no reason a reader could see.
#[test]
fn tasks_come_back_most_urgent_first_and_oldest_first_within_a_priority() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    // Inserted in an order that has nothing to do with the answer: numbers ascending, priorities scrambled.
    for (number, priority) in [
        (1, Priority::Normal),
        (2, Priority::SuperLow),
        (3, Priority::SuperHigh),
        (4, Priority::Normal),
        (5, Priority::High),
        (6, Priority::Low),
    ] {
        let mut itm = task("p", number, COLUMN_ID_TODO, &[]);
        itm.priority = priority;
        board.upsert_task(itm);
    }

    let numbers: Vec<i64> = board
        .read()
        .tasks_of_project("p")
        .iter()
        .map(|itm| itm.number)
        .collect();

    assert_eq!(numbers, vec![3, 5, 1, 4, 6, 2]);
}

/// A goal is ranked the same way and read the same way. Two rules on one product — one for tasks and
/// another for goals — is the thing this test exists to prevent.
#[test]
fn goals_come_back_most_urgent_first_too() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    for (number, priority) in [
        (1, Priority::Low),
        (2, Priority::SuperHigh),
        (3, Priority::Normal),
    ] {
        let mut itm = goal("p", number);
        itm.priority = priority;
        board.upsert_goal(itm);
    }

    let numbers: Vec<i64> = board
        .read()
        .goals_of_project("p")
        .iter()
        .map(|itm| itm.number)
        .collect();

    assert_eq!(numbers, vec![2, 3, 1]);
}

/// The whole shape of a deletion, in one place: gone from everything derived, still there to be found.
///
/// Each half is a separate way to get this wrong. Leaving deleted work in a count holds a goal open on a card
/// nobody can see; taking it out of `tasks_of_project` would make it unfindable, which is the one thing
/// keeping the row was for.
#[test]
fn a_deleted_task_leaves_every_derived_answer_and_stays_findable() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));
    board.upsert_goal(goal("p", 1));

    let mut live = task("p", 2, COLUMN_ID_TODO, &[]);
    live.goal_number = Some(1);
    live.labels = vec!["kept".to_string()];
    board.upsert_task(live);

    let mut gone = task("p", 3, COLUMN_ID_TODO, &[]);
    gone.goal_number = Some(1);
    gone.labels = vec!["lost".to_string()];
    board.upsert_task(deleted(gone));

    let read = board.read();

    assert_eq!(read.goal_progress("p", 1), (1, 0), "counted as one, not two");
    assert_eq!(read.tasks_of_goal("p", 1).len(), 1);
    assert_eq!(read.open_tasks_of_goal("p", 1).len(), 1, "a deleted task cannot hold a goal open");
    assert_eq!(read.tasks_amount("p"), 1);
    assert_eq!(read.labels_of_project("p"), vec!["kept".to_string()]);

    assert!(read.get_task("p", 3).is_none(), "hidden from the ordinary lookup");
    assert!(
        read.get_task_including_deleted("p", 3).is_some(),
        "and found by the one search uses"
    );

    // In the snapshot, because the browser is where a search over text happens — the screen hides it there.
    assert_eq!(
        read.tasks_of_project("p").len(),
        2,
        "the snapshot carries deleted work so it can be searched"
    );
}

/// A deleted BLOCKER keeps its dependents blocked, which falls out of the lookup returning `None` — the same
/// place a typo lands. Deleting a blocker is not a statement that the work is done.
#[test]
fn a_deleted_blocker_still_blocks() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    board.upsert_task(deleted(task("p", 1, COLUMN_ID_DONE, &[])));

    let waiting = task("p", 2, COLUMN_ID_TODO, &[1]);
    board.upsert_task(waiting.clone());

    // And a deleted task that depends on something: it must not show up as waiting on it.
    board.upsert_task(deleted(task("p", 3, COLUMN_ID_TODO, &[4])));
    board.upsert_task(task("p", 4, COLUMN_ID_TODO, &[]));

    let read = board.read();

    assert!(
        read.is_blocked(&waiting),
        "a blocker that was deleted is not a blocker that was finished"
    );

    assert!(
        read.blocks("p", 4).is_empty(),
        "a deleted task is not somebody who is waiting on you"
    );
}

/// A deleted goal disappears, and the work that pointed at it reads as standalone rather than pointing at
/// something nobody can open. The tasks themselves survive — whether work outlives its container is a
/// decision, not a side effect.
#[test]
fn a_deleted_goal_disappears_and_its_tasks_do_not() {
    let board = board();
    board.upsert_project(project("p", "RMS", &[]));

    let mut gone = goal("p", 1);
    gone.deleted_moment = Some(DateTimeAsMicroseconds::new(1));
    board.upsert_goal(gone);

    let mut orphan = task("p", 2, COLUMN_ID_TODO, &[]);
    orphan.goal_number = Some(1);
    board.upsert_task(orphan.clone());

    let read = board.read();

    assert!(read.goals_of_project("p").is_empty());
    assert!(read.get_goal("p", 1).is_none());
    assert!(
        read.get_goal_including_deleted("p", 1).is_some(),
        "still findable by id"
    );

    assert!(
        read.effective_goal(&orphan).is_none(),
        "the task reads as standalone rather than pointing at a goal nobody can open"
    );
    assert_eq!(read.tasks_amount("p"), 1, "the task itself is untouched");
}
