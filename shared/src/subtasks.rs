use my_http_utils::macros::MyHttpObjectStructure;
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

// One item of a checklist, on a task or on a goal.
//
// Its own module rather than a member of either, because neither owns it: the two carry the identical
// list and a reader of one shape must read the other. The wire name is `subtasks` — what an agent was
// asked for — while the screen says "checklist", the same split `kind` and "task type" already live
// with.
//
// This is NOT a task. It has no number from the project's counter, no status, no assignee and no
// thread, it never appears on a board, and nothing derived reads it: a half-ticked checklist does not
// block landing the work, and does not count towards a goal's progress. It is the breakdown one person
// wrote to keep track of one piece of work — real work that somebody else has to see, schedule or
// depend on is a task under a goal, which is what the board is for.
//
// `id` is a `SortableId`, generated when the item is created, and is never shown to a person: the UI
// draws `title`, and the id exists so an agent can name the item it means to tick. Sortable, so a list
// rebuilt from ids alone is still in the order it was written in — though the stored order is the
// array's own, which is what the reader sees.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct SubtaskResponse {
    pub id: String,
    pub title: String,
    // The longer half — what the item actually involves, as Markdown. Often empty: a checklist whose
    // items are one line each is a normal checklist, and the UI then has nothing to expand.
    pub text: String,
    pub done: bool,
}

/// How many items are done, out of how many. `(0, 0)` for a checklist that does not exist, which reads
/// the same as one nobody has ticked and is why a caller should test the total rather than the ratio.
pub fn subtasks_progress(src: &[SubtaskResponse]) -> (usize, usize) {
    (src.iter().filter(|itm| itm.done).count(), src.len())
}
