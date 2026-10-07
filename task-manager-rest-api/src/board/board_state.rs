use std::sync::Arc;

use arc_swap::ArcSwap;

use super::board_inner::BoardInner;
use super::models::{
    ColumnTemplateModel, GoalModel, KindTemplateModel, ProjectModel, ReleaseModel, TaskModel,
    UserModel,
};

/// The product state, held entirely in memory.
///
/// Read constantly and written at human/agent rate, which is the textbook `ArcSwap` case: a reader
/// takes no lock at all — it is an atomic load of an `Arc` — and a writer never blocks one. Writes
/// are copy-on-write: clone the snapshot, mutate the clone, swap it in. The `Mutex<()>` exists only
/// so two concurrent writers cannot both read-modify-write the same snapshot and lose one of the two
/// changes; readers never touch it.
///
/// The lock is `parking_lot` and every critical section here is synchronous. That is load-bearing: a
/// `parking_lot` guard is `!Send`, so an `.await` inside one of these methods would not compile,
/// which is exactly the mistake worth making impossible. Postgres writes therefore happen *outside*
/// — in `scripts/`, before the memory change — and this type never knows the database exists.
pub struct Board {
    inner: ArcSwap<BoardInner>,
    write_lock: parking_lot::Mutex<()>,
}

impl Board {
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::from_pointee(BoardInner::new()),
            write_lock: parking_lot::Mutex::new(()),
        }
    }

    /// The current snapshot. Every read path starts here.
    pub fn read(&self) -> Arc<BoardInner> {
        self.inner.load_full()
    }

    /// Clone the snapshot, apply `mutation`, rebuild the derived indexes, publish it.
    fn mutate<TMutation: FnOnce(&mut BoardInner)>(&self, mutation: TMutation) {
        let _guard = self.write_lock.lock();

        let mut next = self.inner.load().as_ref().clone();
        mutation(&mut next);
        next.rebuild_indexes();

        self.inner.store(Arc::new(next));
    }

    /// Install the state read from Postgres at startup, replacing whatever is there.
    ///
    /// Takes an already-built `BoardInner` rather than applying row by row so the service never
    /// serves a half-loaded board: until this returns, readers see the previous snapshot (empty, on a
    /// cold start).
    pub fn replace_all(&self, mut loaded: BoardInner) {
        let _guard = self.write_lock.lock();

        loaded.rebuild_indexes();
        self.inner.store(Arc::new(loaded));
    }

    pub fn upsert_project(&self, project: ProjectModel) {
        self.mutate(|inner| inner.put_project(Arc::new(project)));
    }

    /// Save a template. Every project following it picks the change up in the same swap, because
    /// `rebuild_indexes` re-resolves their columns.
    pub fn upsert_column_template(&self, template: ColumnTemplateModel) {
        self.mutate(|inner| inner.put_column_template(Arc::new(template)));
    }

    pub fn remove_column_template(&self, id: &str) {
        self.mutate(|inner| inner.drop_column_template(id));
    }

    /// Save a task-type template. Every project following it picks the change up in the same swap.
    pub fn upsert_kind_template(&self, template: KindTemplateModel) {
        self.mutate(|inner| inner.put_kind_template(Arc::new(template)));
    }

    pub fn remove_kind_template(&self, id: &str) {
        self.mutate(|inner| inner.drop_kind_template(id));
    }

    /// Save a goal. There is no removal counterpart: a goal is closed, never deleted.
    pub fn upsert_goal(&self, goal: GoalModel) {
        self.mutate(|inner| inner.put_goal(Arc::new(goal)));
    }

    /// Save a release. No removal counterpart either: one recorded by mistake is flagged deleted.
    pub fn upsert_release(&self, release: ReleaseModel) {
        self.mutate(|inner| inner.put_release(Arc::new(release)));
    }

    pub fn upsert_task(&self, task: TaskModel) {
        self.mutate(|inner| inner.put_task(Arc::new(task)));
    }

    pub fn upsert_user(&self, user: UserModel) {
        self.mutate(|inner| inner.put_user(Arc::new(user)));
    }

    /// Put a whole batch of goals, releases and tasks in, in ONE snapshot swap.
    ///
    /// The bulk counterpart of [`Self::upsert_goal`], [`Self::upsert_release`] and [`Self::upsert_task`], and
    /// it exists for the one caller that writes hundreds of rows in a single request: importing a project.
    /// Done one at a time, that is one clone of the board and one `rebuild_indexes` per row — quadratic in
    /// the size of the board for no reason, since every screen only ever sees the snapshot at the end of it
    /// either way.
    ///
    /// Releases are in the batch rather than swapped in beside it, because the goals in it list them by
    /// number: one swap means no reader ever holds a snapshot with those goals in it and the releases they
    /// name not yet there.
    ///
    /// Nothing about the semantics differs from calling the three singular methods in a loop: the batch is
    /// applied in order, and the indexes are rebuilt once at the end, which is exactly what they would be.
    pub fn upsert_goals_releases_and_tasks(
        &self,
        goals: Vec<GoalModel>,
        releases: Vec<ReleaseModel>,
        tasks: Vec<TaskModel>,
    ) {
        self.mutate(|inner| {
            for goal in goals {
                inner.put_goal(Arc::new(goal));
            }

            for release in releases {
                inner.put_release(Arc::new(release));
            }

            for task in tasks {
                inner.put_task(Arc::new(task));
            }
        });
    }

    /// Hand out the next task number for a project, moving its counter.
    ///
    /// This is the one place memory is written *before* Postgres, and it is safe because the counter
    /// only ever moves forward: a number handed out and then lost to a crash before the task row was
    /// written is simply never used again. The startup load floors the counter at the highest number
    /// any surviving task carries, so a stale counter row can never re-issue a live number either.
    ///
    /// `None` when the project does not exist — the caller reports that rather than inventing a task.
    pub fn reserve_task_number(&self, project_id: &str) -> Option<i64> {
        self.reserve_task_numbers(project_id, 1)?.into_iter().next()
    }

    /// Hand out `amount` consecutive task numbers, moving the counter once.
    ///
    /// The same promise [`Self::reserve_task_number`] makes, kept for a batch: an import needs every number
    /// before it can write anything — a task's dependencies and its goal are named by handles that may point
    /// forwards in the file — and taking them one at a time would swap the snapshot once per card.
    ///
    /// `None` when the project does not exist, or when `amount` is not positive: a reservation of nothing is
    /// a caller that has miscounted, and an empty `Vec` would let it carry on believing otherwise.
    pub fn reserve_task_numbers(&self, project_id: &str, amount: i64) -> Option<Vec<i64>> {
        if amount < 1 {
            return None;
        }

        let _guard = self.write_lock.lock();

        let mut next = self.inner.load().as_ref().clone();

        let project = next.get_project(project_id)?;
        let mut project = project.as_ref().clone();

        let first = project.last_task_number + 1;
        project.last_task_number += amount;

        next.put_project(Arc::new(project));
        next.rebuild_indexes();
        self.inner.store(Arc::new(next));

        Some((first..first + amount).collect())
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}
