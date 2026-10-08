use std::sync::Arc;
use std::time::Duration;

use ahash::{AHashMap, AHashSet};
use rust_extensions::date_time::DateTimeAsMicroseconds;

use super::models::{
    ColumnTemplateModel, GoalModel, KindTemplateModel, ProjectModel, ReleaseModel, TaskModel,
    UserModel,
};

/// How long finished work stays on the board before it counts as archived, when a project has not said
/// otherwise.
///
/// Done is the only column that grows for ever, so it is the only one that needs a window. Seven days is
/// long enough to cover "what did we ship this week" and short enough that the column stays readable —
/// and it is what every project did before the window became a per-project setting, which is why a
/// project with no `archive_days` reads exactly as it always has. See `ProjectModel::archive_after`.
pub const ARCHIVE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The whole product state, indexed, as one immutable snapshot.
///
/// Every read serves from here and takes no lock — the wrapper hands out an `Arc` of it. Writes
/// clone it, mutate the clone and swap it in, which is why every collection holds `Arc`s: a clone of
/// this struct copies pointers, not tasks.
///
/// Indexes are derived rather than maintained incrementally ([`BoardInner::rebuild_indexes`] runs
/// after every mutation). At this size that is free, and it removes the entire class of bug where an
/// index and its source disagree.
#[derive(Clone)]
pub struct BoardInner {
    projects: AHashMap<String, Arc<ProjectModel>>,
    /// Template id -> the named set of columns projects follow. Columns are configured here, not on a
    /// project; a project carries only the id of the template it follows.
    column_templates: AHashMap<String, Arc<ColumnTemplateModel>>,
    /// Template id -> the named set of task types projects follow. Same arrangement as the columns above.
    kind_templates: AHashMap<String, Arc<KindTemplateModel>>,
    /// Upper-cased current prefix -> project id. One entry per project: two projects may not hold
    /// the same prefix at the same time, which is exactly what makes this a map and not a multimap.
    prefix_index: AHashMap<String, String>,
    /// Upper-cased prefix -> every project that has *ever* held it, current holder included.
    /// A multimap, because a prefix is free to move on once renamed away from.
    historical_prefix_index: AHashMap<String, Vec<String>>,
    /// project id -> goal number -> goal. Nested exactly like `tasks`, because a goal is identified the
    /// same way a task is and every lookup arrives with the project already in hand: a task names its
    /// goal by number alone, and the number only means anything within its own project.
    goals: AHashMap<String, AHashMap<i64, Arc<GoalModel>>>,
    /// project id -> release number -> release. The same nesting for the same reason: a goal names its
    /// releases by number alone, and that number only means anything within the goal's own project.
    releases: AHashMap<String, AHashMap<i64, Arc<ReleaseModel>>>,
    /// project id -> task number -> task.
    tasks: AHashMap<String, AHashMap<i64, Arc<TaskModel>>>,
    /// Lower-cased email -> user.
    users: AHashMap<String, Arc<UserModel>>,
    /// Cached whole lists so a "give me everything" read clones an `Arc` instead of allocating.
    projects_list: Arc<Vec<Arc<ProjectModel>>>,
    users_list: Arc<Vec<Arc<UserModel>>>,
    column_templates_list: Arc<Vec<Arc<ColumnTemplateModel>>>,
    kind_templates_list: Arc<Vec<Arc<KindTemplateModel>>>,
}

impl BoardInner {
    pub fn new() -> Self {
        Self {
            projects: AHashMap::new(),
            column_templates: AHashMap::new(),
            kind_templates: AHashMap::new(),
            prefix_index: AHashMap::new(),
            historical_prefix_index: AHashMap::new(),
            goals: AHashMap::new(),
            releases: AHashMap::new(),
            tasks: AHashMap::new(),
            users: AHashMap::new(),
            projects_list: Arc::new(Vec::new()),
            users_list: Arc::new(Vec::new()),
            column_templates_list: Arc::new(Vec::new()),
            kind_templates_list: Arc::new(Vec::new()),
        }
    }

    /// Build a snapshot out of what the startup load read from Postgres.
    ///
    /// The one place the counter invariant lives: each project's `last_task_number` is floored at the
    /// highest number any of its surviving tasks, **goals or releases** carries. A counter row that is
    /// missing or has fallen behind therefore cannot re-issue a live number — and since the floor is
    /// computed from the rows that actually exist, a number lost to a crash before its row was written is
    /// simply never used.
    ///
    /// Goals and releases count towards that floor because they draw from the same counter. Leaving either
    /// out would let a behind counter hand a task the number one of them already has, and then `RMS-7` and
    /// `RMS-G7` — or `RMS-R7` — would both exist, which is the one assumption everything else here is
    /// built on.
    ///
    /// A task whose project is gone is dropped: there is nothing to compose its handle from and no
    /// board to draw it on. That cannot happen while project deletion is unimplemented, which is
    /// exactly why it is worth handling now rather than when it can.
    pub fn from_loaded(
        projects: Vec<ProjectModel>,
        tasks: Vec<TaskModel>,
        users: Vec<UserModel>,
        column_templates: Vec<ColumnTemplateModel>,
        kind_templates: Vec<KindTemplateModel>,
        goals: Vec<GoalModel>,
        releases: Vec<ReleaseModel>,
    ) -> Self {
        let mut result = Self::new();

        // Before the projects: `rebuild_indexes` resolves each project's columns against these, and a
        // project loaded first would otherwise resolve to an empty board until the next write.
        for template in column_templates {
            result.put_column_template(Arc::new(template));
        }

        for template in kind_templates {
            result.put_kind_template(Arc::new(template));
        }

        let mut highest_number: AHashMap<String, i64> = AHashMap::new();

        for task in &tasks {
            let entry = highest_number.entry(task.project_id.clone()).or_insert(0);
            if task.number > *entry {
                *entry = task.number;
            }
        }

        for goal in &goals {
            let entry = highest_number.entry(goal.project_id.clone()).or_insert(0);
            if goal.number > *entry {
                *entry = goal.number;
            }
        }

        for release in &releases {
            let entry = highest_number
                .entry(release.project_id.clone())
                .or_insert(0);
            if release.number > *entry {
                *entry = release.number;
            }
        }

        for goal in goals {
            result.put_goal(Arc::new(goal));
        }

        for release in releases {
            result.put_release(Arc::new(release));
        }

        for mut project in projects {
            let floor = highest_number.get(&project.id).copied().unwrap_or(0);

            if project.last_task_number < floor {
                project.last_task_number = floor;
            }

            result.put_project(Arc::new(project));
        }

        for task in tasks {
            if result.projects.contains_key(&task.project_id) {
                result.put_task(Arc::new(task));
            }
        }

        for user in users {
            result.put_user(Arc::new(user));
        }

        result.rebuild_indexes();
        result
    }

    // ----------------------------------------------------------------- mutation (crate-internal)

    pub(super) fn put_project(&mut self, project: Arc<ProjectModel>) {
        self.tasks.entry(project.id.clone()).or_default();
        self.projects.insert(project.id.clone(), project);
    }

    pub(super) fn put_column_template(&mut self, template: Arc<ColumnTemplateModel>) {
        self.column_templates.insert(template.id.clone(), template);
    }

    pub(super) fn drop_column_template(&mut self, id: &str) {
        self.column_templates.remove(id);
    }

    pub(super) fn put_kind_template(&mut self, template: Arc<KindTemplateModel>) {
        self.kind_templates.insert(template.id.clone(), template);
    }

    pub(super) fn drop_kind_template(&mut self, id: &str) {
        self.kind_templates.remove(id);
    }

    pub(super) fn put_goal(&mut self, goal: Arc<GoalModel>) {
        self.goals
            .entry(goal.project_id.clone())
            .or_default()
            .insert(goal.number, goal);
    }

    pub(super) fn put_release(&mut self, release: Arc<ReleaseModel>) {
        self.releases
            .entry(release.project_id.clone())
            .or_default()
            .insert(release.number, release);
    }

    pub(super) fn put_task(&mut self, task: Arc<TaskModel>) {
        self.tasks
            .entry(task.project_id.clone())
            .or_default()
            .insert(task.number, task);
    }

    pub(super) fn put_user(&mut self, user: Arc<UserModel>) {
        self.users.insert(user.email.clone(), user);
    }

    /// Recompute everything derived. Called once at the end of every mutation and once after the
    /// startup load.
    pub(super) fn rebuild_indexes(&mut self) {
        self.prefix_index.clear();
        self.historical_prefix_index.clear();

        self.resolve_project_columns();

        for project in self.projects.values() {
            self.prefix_index
                .insert(project.prefix.clone(), project.id.clone());

            // The current prefix counts as history too, so `tasks_resolve_id` can report the live
            // holder and the past ones through one index.
            for prefix in std::iter::once(&project.prefix).chain(project.prefix_history.iter()) {
                let owners = self
                    .historical_prefix_index
                    .entry(prefix.clone())
                    .or_default();

                if !owners.contains(&project.id) {
                    owners.push(project.id.clone());
                }
            }
        }

        // Sorted so two identical reads produce identical output — an unordered map would reshuffle
        // between them and read as a change that did not happen.
        for owners in self.historical_prefix_index.values_mut() {
            owners.sort_unstable();
        }

        let mut projects: Vec<Arc<ProjectModel>> = self.projects.values().cloned().collect();
        projects.sort_by_key(|itm| itm.name.to_lowercase());
        self.projects_list = Arc::new(projects);

        let mut users: Vec<Arc<UserModel>> = self.users.values().cloned().collect();
        users.sort_by(|l, r| l.email.cmp(&r.email));
        self.users_list = Arc::new(users);

        let mut templates: Vec<Arc<ColumnTemplateModel>> =
            self.column_templates.values().cloned().collect();
        templates.sort_by_key(|itm| itm.name.to_lowercase());
        self.column_templates_list = Arc::new(templates);

        let mut kind_templates: Vec<Arc<KindTemplateModel>> =
            self.kind_templates.values().cloned().collect();
        kind_templates.sort_by_key(|itm| itm.name.to_lowercase());
        self.kind_templates_list = Arc::new(kind_templates);
    }

    /// Copy each project's columns down from the template it follows.
    ///
    /// The single place the template indirection is resolved. Everything downstream — `has_column`,
    /// `effective_status`, the board read, the MCP tools — keeps asking a project for its own columns and
    /// never learns templates exist.
    ///
    /// A project following no template, or one naming a template that is gone, gets an empty list: its
    /// board is Todo → Done. Tasks parked in a column that just disappeared are not touched; they read as
    /// Todo through `effective_status` and come back if the template returns. Same rule as a deleted
    /// column has always had, which is why deleting a template in use is refused rather than silently
    /// emptying boards.
    fn resolve_project_columns(&mut self) {
        for project in self.projects.values_mut() {
            let columns = project
                .column_template_id
                .as_ref()
                .and_then(|id| self.column_templates.get(id))
                .map(|template| template.columns.clone())
                .unwrap_or_default();

            let kinds = project
                .kind_template_id
                .as_ref()
                .and_then(|id| self.kind_templates.get(id))
                .map(|template| template.kinds.clone())
                .unwrap_or_default();

            if project.columns == columns && project.kinds == kinds {
                continue;
            }

            // Both in one pass, so a project is cloned at most once per rebuild. `make_mut` clones only
            // when this project is shared with a snapshot a reader still holds — so a rebuild that changes
            // nothing copies nothing.
            let project = Arc::make_mut(project);
            project.columns = columns;
            project.kinds = kinds;
        }
    }

    // ---------------------------------------------------------------------------------- reading

    pub fn get_column_template(&self, id: &str) -> Option<Arc<ColumnTemplateModel>> {
        self.column_templates.get(id).cloned()
    }

    /// Every template, by name. Cheap — clones one `Arc`.
    pub fn get_column_templates(&self) -> Arc<Vec<Arc<ColumnTemplateModel>>> {
        self.column_templates_list.clone()
    }

    /// How many projects follow this template. What makes deleting it refusable, and what the setup
    /// screen shows so an edit's blast radius is visible before you make it.
    pub fn count_projects_using_template(&self, template_id: &str) -> usize {
        self.projects
            .values()
            .filter(|itm| itm.column_template_id.as_deref() == Some(template_id))
            .count()
    }

    pub fn get_kind_template(&self, id: &str) -> Option<Arc<KindTemplateModel>> {
        self.kind_templates.get(id).cloned()
    }

    pub fn get_kind_templates(&self) -> Arc<Vec<Arc<KindTemplateModel>>> {
        self.kind_templates_list.clone()
    }

    pub fn count_projects_using_kind_template(&self, template_id: &str) -> usize {
        self.projects
            .values()
            .filter(|itm| itm.kind_template_id.as_deref() == Some(template_id))
            .count()
    }

    /// One goal — **`None` for a deleted one**, which is what makes every read that goes through here forget
    /// it: a task under a deleted goal reads as standalone, and closing, progress and the Goals screen stop
    /// seeing it. Search is the one caller that must not forget, and it has its own door:
    /// [`Self::get_goal_including_deleted`].
    pub fn get_goal(&self, project_id: &str, number: i64) -> Option<Arc<GoalModel>> {
        let goal = self.goals.get(project_id)?.get(&number)?;

        if goal.is_deleted() {
            return None;
        }

        Some(goal.clone())
    }

    /// One goal, deleted or not. For search, which has to find what was deleted and say so — an id that comes
    /// back as "no such goal" is indistinguishable from a typo, and that is exactly the confusion a deletion
    /// that leaves nothing behind creates.
    pub fn get_goal_including_deleted(
        &self,
        project_id: &str,
        number: i64,
    ) -> Option<Arc<GoalModel>> {
        self.goals.get(project_id)?.get(&number).cloned()
    }

    /// A project's goals, deleted ones included, in no particular order.
    ///
    /// The counterpart of [`Self::get_goal_including_deleted`], and it has exactly one caller for exactly the
    /// same reason: an export carries the whole board, and a deleted goal left out of it would make exporting
    /// a quiet way of losing the record. Every screen and every derived answer goes through
    /// [`Self::goals_of_project`], which forgets them.
    pub fn goals_of_project_including_deleted(&self, project_id: &str) -> Vec<Arc<GoalModel>> {
        let Some(of_project) = self.goals.get(project_id) else {
            return Vec::new();
        };

        of_project.values().cloned().collect()
    }

    /// A project's goals, **most urgent first and oldest first within one priority** — the same order tasks
    /// come back in, because a screen that ranked one and not the other would be two different rules on one
    /// product. Within a priority, the number is the tiebreaker, which is also oldest-first since the counter
    /// only goes up.
    pub fn goals_of_project(&self, project_id: &str) -> Vec<Arc<GoalModel>> {
        let Some(of_project) = self.goals.get(project_id) else {
            return Vec::new();
        };

        let mut result: Vec<Arc<GoalModel>> = of_project
            .values()
            .filter(|goal| !goal.is_deleted())
            .cloned()
            .collect();

        result.sort_by_key(|itm| (itm.priority.order(), itm.number));
        result
    }

    /// One release — **`None` for a deleted one**, the same forgetting [`Self::get_goal`] does and for the
    /// same reason: every read that goes through here stops seeing it, so a goal that lists a deleted
    /// release simply shows one fewer. Resolving an id somebody quoted is the one caller that must not
    /// forget, and it has its own door: [`Self::get_release_including_deleted`].
    pub fn get_release(&self, project_id: &str, number: i64) -> Option<Arc<ReleaseModel>> {
        let release = self.releases.get(project_id)?.get(&number)?;

        if release.is_deleted() {
            return None;
        }

        Some(release.clone())
    }

    /// One release, deleted or not — for the tools that act on a release by its id. Undeleting one has to
    /// be able to find it, and an id that came back as "no such release" would read as a typo.
    pub fn get_release_including_deleted(
        &self,
        project_id: &str,
        number: i64,
    ) -> Option<Arc<ReleaseModel>> {
        self.releases.get(project_id)?.get(&number).cloned()
    }

    /// A project's releases, **newest first** — by the date of the release, not by when it was written
    /// down, and by number within one date so the order is total.
    ///
    /// Newest first because that is the question a list of releases is asked: what went out last. Unlike
    /// goals and tasks there is no archive window here — a release does not age off, the list IS the
    /// history.
    pub fn releases_of_project(&self, project_id: &str) -> Vec<Arc<ReleaseModel>> {
        let Some(of_project) = self.releases.get(project_id) else {
            return Vec::new();
        };

        let mut result: Vec<Arc<ReleaseModel>> = of_project
            .values()
            .filter(|release| !release.is_deleted())
            .cloned()
            .collect();

        sort_newest_first(&mut result);
        result
    }

    /// Every environment a project's releases name, once each, as the project spells it.
    ///
    /// Nothing stores this list — an environment exists for as long as some live release carries its
    /// label, the way a board's labels are read off its tasks. It is what a new label is spelled after
    /// when it is added (see `EnvsPatch::apply`), and what a caller is shown so that it can reuse a
    /// spelling instead of inventing a second one.
    ///
    /// Sorted ignoring case, so the answer is the same whichever order the releases were recorded in. Two
    /// spellings of one environment cannot normally exist; where an import brought one in, the spelling
    /// on the newest release is the one kept.
    pub fn envs_of_project(&self, project_id: &str) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();

        for release in self.releases_of_project(project_id) {
            for env in &release.envs {
                if !result
                    .iter()
                    .any(|itm| task_manager_shared::releases::same_env(itm, env))
                {
                    result.push(env.clone());
                }
            }
        }

        result.sort_by_key(|itm| itm.to_lowercase());
        result
    }

    /// A project's releases, deleted ones included, in no particular order. One caller, the export, for
    /// the reason given on [`Self::goals_of_project_including_deleted`].
    pub fn releases_of_project_including_deleted(
        &self,
        project_id: &str,
    ) -> Vec<Arc<ReleaseModel>> {
        let Some(of_project) = self.releases.get(project_id) else {
            return Vec::new();
        };

        of_project.values().cloned().collect()
    }

    /// The releases a goal went out in, newest first — the same order the whole list is in, so a goal's
    /// dialog and the Releases screen never disagree about which came last.
    ///
    /// A number naming no release, or a deleted one, is skipped rather than reported: the stored list is
    /// left alone, so undeleting the release puts it back on the goal. The same leniency a task's goal
    /// gets from [`Self::effective_goal`].
    pub fn releases_of_goal(&self, goal: &GoalModel) -> Vec<Arc<ReleaseModel>> {
        let mut result: Vec<Arc<ReleaseModel>> = goal
            .releases
            .iter()
            .filter_map(|number| self.get_release(&goal.project_id, *number))
            .collect();

        sort_newest_first(&mut result);
        result
    }

    /// The reverse edge: the goals that list this release, by number.
    ///
    /// Nothing stores this — a release does not know what it shipped, the goal says so. Normally one
    /// goal; none for a release nobody has attached yet, and more than one is allowed without meaning
    /// much. A deleted goal is not counted, since nothing can open it.
    pub fn goals_of_release(&self, project_id: &str, number: i64) -> Vec<Arc<GoalModel>> {
        let Some(of_project) = self.goals.get(project_id) else {
            return Vec::new();
        };

        let mut result: Vec<Arc<GoalModel>> = of_project
            .values()
            .filter(|goal| !goal.is_deleted() && goal.releases.contains(&number))
            .cloned()
            .collect();

        result.sort_by_key(|itm| itm.number);
        result
    }

    /// The goal a task should be *read* as part of.
    ///
    /// `None` when it has none, and also when its number names no goal — the stored value is left alone,
    /// exactly as with a status or a task type. Nothing removes a goal in this version, so the second case
    /// should not arise; being lenient about it means a gap in the data reads as "standalone" rather than
    /// making the task unreadable.
    pub fn effective_goal(&self, task: &TaskModel) -> Option<Arc<GoalModel>> {
        self.get_goal(&task.project_id, task.goal_number?)
    }

    /// How many of a goal's tasks there are, and how many are done.
    ///
    /// The whole of a goal's progress — derived, never stored, so it cannot disagree with the board.
    /// Archived tasks are counted: a goal closes only once every one of its tasks is done, and by then the
    /// oldest of them have aged out of the board, so a count that skipped them would report a finished
    /// goal as half-finished. What the counter measures is the epic, not this week.
    pub fn goal_progress(&self, project_id: &str, number: i64) -> (usize, usize) {
        let Some(of_project) = self.tasks.get(project_id) else {
            return (0, 0);
        };

        let mut total = 0;
        let mut done = 0;

        for task in of_project.values() {
            if task.goal_number != Some(number) {
                continue;
            }

            // A deleted task is not work this goal is judged by — it is work that should never have been
            // counted at all. Leaving it in would hold a goal open on a card nobody can see.
            if task.is_deleted() {
                continue;
            }

            total += 1;

            if task.status == task_manager_shared::projects::COLUMN_ID_DONE {
                done += 1;
            }
        }

        (total, done)
    }

    /// The tasks of one goal, most urgent first and oldest first within one priority — the same order the
    /// board uses. Includes archived work, for the reason given on [`BoardInner::goal_progress`] — the list
    /// and the counter have to agree.
    pub fn tasks_of_goal(&self, project_id: &str, number: i64) -> Vec<Arc<TaskModel>> {
        let Some(of_project) = self.tasks.get(project_id) else {
            return Vec::new();
        };

        let mut result: Vec<Arc<TaskModel>> = of_project
            .values()
            .filter(|task| task.goal_number == Some(number) && !task.is_deleted())
            .cloned()
            .collect();

        result.sort_by_key(|itm| (itm.priority.order(), itm.number));
        result
    }

    /// Whether a closed goal has aged out of the screen.
    ///
    /// The same rule and the same window as a task, read off the goal's own project: a goal is history
    /// once it has been closed for longer than the project's archive window. An open goal never archives,
    /// however old it is — an epic that has run for a year is not history, it is late.
    pub fn is_goal_archived(&self, goal: &GoalModel) -> bool {
        let Some(closed) = goal.close_moment else {
            return false;
        };

        let Some(project) = self.projects.get(&goal.project_id) else {
            return false;
        };

        let now = DateTimeAsMicroseconds::now();

        now.unix_microseconds - closed.unix_microseconds > project.archive_after().as_micros() as i64
    }

    /// Whether every task of this goal is done — the question that decides if it may be closed.
    ///
    /// Counts archived work as done, which it is: a task archives only from Done.
    pub fn open_tasks_of_goal(&self, project_id: &str, number: i64) -> Vec<Arc<TaskModel>> {
        self.tasks_of_goal(project_id, number)
            .into_iter()
            .filter(|task| task.status != task_manager_shared::projects::COLUMN_ID_DONE)
            .collect()
    }

    pub fn get_project(&self, project_id: &str) -> Option<Arc<ProjectModel>> {
        self.projects.get(project_id).cloned()
    }

    /// The project holding this prefix **right now**. `prefix` must already be upper-cased — a
    /// parsed handle is.
    pub fn get_project_by_prefix(&self, prefix: &str) -> Option<Arc<ProjectModel>> {
        let project_id = self.prefix_index.get(prefix)?;
        self.projects.get(project_id).cloned()
    }

    /// Every project that has ever held this prefix, including the current holder, sorted by id.
    pub fn projects_ever_holding_prefix(&self, prefix: &str) -> Vec<Arc<ProjectModel>> {
        let Some(owners) = self.historical_prefix_index.get(prefix) else {
            return Vec::new();
        };

        owners
            .iter()
            .filter_map(|id| self.projects.get(id).cloned())
            .collect()
    }

    /// Whether a prefix can be taken. Free means nobody holds it as their current prefix — a prefix
    /// only in some project's history is fair game, which is the decision that made task handles
    /// composed-on-read rather than stored.
    pub fn is_prefix_free(&self, prefix: &str, ignoring_project_id: Option<&str>) -> bool {
        match self.prefix_index.get(prefix) {
            None => true,
            Some(holder) => Some(holder.as_str()) == ignoring_project_id,
        }
    }

    pub fn projects(&self) -> Arc<Vec<Arc<ProjectModel>>> {
        self.projects_list.clone()
    }

    pub fn users(&self) -> Arc<Vec<Arc<UserModel>>> {
        self.users_list.clone()
    }

    pub fn get_user(&self, email: &str) -> Option<Arc<UserModel>> {
        self.users.get(&email.trim().to_lowercase()).cloned()
    }

    /// The display name for an assignee value, when there is one.
    ///
    /// `AI` resolves to itself; `None` for an email with no user row — the caller then shows the raw value,
    /// which is more honest than inventing a name for it.
    pub fn display_name_of(&self, assignee: &str) -> Option<String> {
        // The reserved assignee has no user row by design, so it would otherwise fall through and a
        // sticker would show whatever case the agent happened to write.
        if task_manager_shared::users::is_ai_assignee(assignee) {
            return Some(task_manager_shared::users::ASSIGNEE_AI.to_string());
        }

        let user = self.get_user(assignee)?;

        if user.name.trim().is_empty() {
            None
        } else {
            Some(user.name.clone())
        }
    }

    /// One task — **`None` for a deleted one.**
    ///
    /// Which is what makes a deleted blocker keep its dependents blocked: `is_blocked` treats an id that names
    /// no task as unsatisfied, deliberately, so that a typo does not silently free work. A deletion lands in
    /// exactly the same place, and it should: deleting a blocker is not the same statement as finishing it.
    pub fn get_task(&self, project_id: &str, number: i64) -> Option<Arc<TaskModel>> {
        let task = self.tasks.get(project_id)?.get(&number)?;

        if task.is_deleted() {
            return None;
        }

        Some(task.clone())
    }

    /// One task, deleted or not — for search, which is the one thing a deleted task is kept for.
    pub fn get_task_including_deleted(
        &self,
        project_id: &str,
        number: i64,
    ) -> Option<Arc<TaskModel>> {
        self.tasks.get(project_id)?.get(&number).cloned()
    }

    /// A project's tasks, **most urgent first and oldest first within one priority**. Not capped: a board is
    /// a hand-written list.
    ///
    /// **Deleted work is INCLUDED here, and that is deliberate.** This is what fills the snapshot the browser
    /// holds, and the browser is where a search happens — filtering here would make a deleted task
    /// unfindable, which is the one thing keeping the row was for. Every screen draws only what is not
    /// deleted; the search box is the one that does not.
    ///
    /// Ordered here rather than by each reader, which is what keeps the board, the Goals screen and
    /// `tasks_list` from disagreeing about what is at the top of a column. The number is the tiebreaker
    /// because it is the only total order there is — and within one priority it means oldest first, which is
    /// what the board did before priorities existed.
    pub fn tasks_of_project(&self, project_id: &str) -> Vec<Arc<TaskModel>> {
        let Some(of_project) = self.tasks.get(project_id) else {
            return Vec::new();
        };

        let mut tasks: Vec<Arc<TaskModel>> = of_project.values().cloned().collect();
        tasks.sort_by_key(|itm| (itm.priority.order(), itm.number));
        tasks
    }

    pub fn tasks_amount(&self, project_id: &str) -> usize {
        self.tasks
            .get(project_id)
            .map(|of_project| of_project.values().filter(|itm| !itm.is_deleted()).count())
            .unwrap_or(0)
    }

    /// The projects this person may see. An admin sees every one without being a member of any.
    pub fn projects_visible_to(&self, email: &str, is_admin: bool) -> Vec<Arc<ProjectModel>> {
        if is_admin {
            return self.projects_list.as_ref().clone();
        }

        let email = email.trim().to_lowercase();

        self.projects_list
            .iter()
            .filter(|project| project.is_member(&email))
            .cloned()
            .collect()
    }

    /// A project's label vocabulary: the distinct labels its tasks currently carry, sorted.
    ///
    /// Not stored anywhere — a label exists exactly as long as a task wears it. That is the whole
    /// reason there is no labels table.
    pub fn labels_of_project(&self, project_id: &str) -> Vec<String> {
        let Some(of_project) = self.tasks.get(project_id) else {
            return Vec::new();
        };

        // A label exists for as long as a task wears it — and a deleted task wears nothing. Without this the
        // last task carrying a tag could be deleted and the tag would stay in every filter, naming nothing.
        let unique: AHashSet<&String> = of_project
            .values()
            .filter(|task| !task.is_deleted())
            .flat_map(|task| task.labels.iter())
            .collect();

        let mut labels: Vec<String> = unique.into_iter().cloned().collect();
        labels.sort();
        labels
    }

    // ------------------------------------------------------------------------------- derivations

    /// Whether a finished task has aged out of the board.
    ///
    /// Done is the one column that grows without bound, and a board nobody can read is a board nobody
    /// looks at — so work closed longer ago than the project's archive window stops being shown. That
    /// window is [`ARCHIVE_AFTER`] unless the project says otherwise. Nothing is deleted: the task is
    /// still there, still reachable by its id, and `tasks_list` can ask for it explicitly.
    ///
    /// A task in Done with no `close_moment` is treated as **not** archived. That should not occur — the
    /// moment is written on the way in — and being lenient about it means a gap in the data cannot make
    /// work silently vanish from the board.
    pub fn is_archived(&self, task: &TaskModel) -> bool {
        let Some(project) = self.projects.get(&task.project_id) else {
            return false;
        };

        if project.effective_status(&task.status) != task_manager_shared::projects::COLUMN_ID_DONE {
            return false;
        }

        let Some(closed) = task.close_moment else {
            return false;
        };

        let now = DateTimeAsMicroseconds::now();

        now.unix_microseconds - closed.unix_microseconds > project.archive_after().as_micros() as i64
    }

    /// Whether a task is blocked: any id in `depends_on` naming a task that is not Done.
    ///
    /// A number matching no task counts as **still blocking**. That is deliberate — a mistyped or
    /// deleted blocker keeps the task blocked rather than silently freeing it, which is the failure
    /// mode nobody would notice.
    pub fn is_blocked(&self, task: &TaskModel) -> bool {
        if task.depends_on.is_empty() {
            return false;
        }

        let Some(project) = self.projects.get(&task.project_id) else {
            return true;
        };

        task.depends_on.iter().any(|blocker| {
            match self.get_task(&task.project_id, *blocker) {
                None => true, // unknown blocker blocks
                Some(blocker) => {
                    project.effective_status(&blocker.status)
                        != task_manager_shared::projects::COLUMN_ID_DONE
                }
            }
        })
    }

    /// The reverse edge: numbers of the tasks naming this one in their `depends_on`.
    ///
    /// Nothing stores this. A task says what is in its way; only the rest of the board knows who is
    /// waiting on it, which is the half you need before parking or re-scoping something.
    pub fn blocks(&self, project_id: &str, number: i64) -> Vec<i64> {
        let Some(of_project) = self.tasks.get(project_id) else {
            return Vec::new();
        };

        let mut dependents: Vec<i64> = of_project
            .values()
            .filter(|task| !task.is_deleted() && task.depends_on.contains(&number))
            .map(|task| task.number)
            .collect();

        dependents.sort_unstable();
        dependents
    }
}

impl Default for BoardInner {
    fn default() -> Self {
        Self::new()
    }
}

/// Newest release first, by its date; the higher number first within one date.
///
/// One function for the two lists of releases there are, which is what keeps them in the same order. The
/// number breaks a tie because several releases recorded with a bare date all land on the same midnight,
/// and a later number is the one written down later.
fn sort_newest_first(releases: &mut [Arc<ReleaseModel>]) {
    releases.sort_by_key(|itm| {
        (
            std::cmp::Reverse(itm.date.unix_microseconds),
            std::cmp::Reverse(itm.number),
        )
    });
}
