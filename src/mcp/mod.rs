use std::sync::Arc;

use mcp_server_middleware::McpMiddleware;

use crate::app::AppContext;

mod briefs_tool_calls;
mod comment_tool_calls;
mod documents_text_tool_calls;
mod documents_tool_calls;
mod github_git_tool_call;
mod github_refresh_tool_call;
mod goals_tool_calls;
mod labels_list_tool_call;
mod projects_list_tool_call;
mod releases_tool_calls;
mod resolve_id_tool_call;
mod tasks_list_tool_call;
mod tasks_search_tool_call;
mod tasks_write_tool_calls;
mod users_list_tool_call;
mod views;

pub use views::*;

use briefs_tool_calls::{DocumentsNextWithoutBriefHandler, DocumentsSetBriefHandler};
use comment_tool_calls::{AddCommentHandler, GetCommentsHandler};
use documents_text_tool_calls::{
    DocumentsDiffHandler, DocumentsEditHandler, DocumentsOutlineHandler, DocumentsSearchHandler,
};
use documents_tool_calls::{
    DocumentsDeleteFolderHandler, DocumentsDeleteHandler, DocumentsGetHandler,
    DocumentsHistoryHandler, DocumentsListHandler, DocumentsRestoreHandler, DocumentsTrashHandler,
    DocumentsUpdatePathHandler, DocumentsUploadHandler,
};
use github_git_tool_call::GithubGitHandler;
use github_refresh_tool_call::GithubRefreshHandler;
use goals_tool_calls::{
    GoalsAddCommentHandler, GoalsCreateHandler, GoalsDeleteHandler, GoalsGetCommentsHandler,
    GoalsListHandler, GoalsUpdateHandler,
};
use labels_list_tool_call::LabelsListHandler;
use projects_list_tool_call::ProjectsListHandler;
use releases_tool_calls::{
    ReleasesAddCommentHandler, ReleasesCreateHandler, ReleasesDeleteHandler,
    ReleasesGetCommentsHandler, ReleasesListHandler, ReleasesUpdateHandler,
};
use resolve_id_tool_call::ResolveIdHandler;
use tasks_list_tool_call::TasksListHandler;
use tasks_search_tool_call::TasksSearchHandler;
use tasks_write_tool_calls::{TasksCreateHandler, TasksDeleteHandler, TasksUpdateHandler};
use users_list_tool_call::UsersListHandler;

const INSTRUCTIONS: &str = "A task board that agents work and people configure. \
\
THIS IS ALL BUT ONE WAY THE BOARD CHANGES. Projects, their columns, their kinds and who may see them \
are set up by a person in the browser; every task, every assignment, every comment, every goal arrives \
through these tools. So when someone says \"put Yuri on it\" or \"note that we decided X\", there is no \
other route: it happens here or it does not happen.\
\
The exception is worth knowing about because it changes what you can assume between two calls: a person \
can DRAG A CARD BETWEEN COLUMNS in the browser, and can recolour a goal. So a task's status may have \
moved since you last read it, by a hand rather than by a tool — re-read before reasoning about where \
work sits, and treat a status you were told about minutes ago as stale. A landing done that way still \
carries its comment: the browser is made to ask for one, the same rule this tool obeys.\
\
CALL projects_list FIRST. It is the only tool that reveals what exists, and it returns the five \
vocabularies every other call is written in: the project prefixes that name a board, the column ids a \
status must be one of, the kind ids with what each one means ON THAT BOARD, the people who may be \
assigned work on it, and the goals its work is organised under. Nothing here is global — two projects \
can have completely different columns and completely different kinds, so what you learned about one \
board tells you nothing about the next. One conversation normally works on one project, so \
`projects_list` with that prefix is the call to make.\
\
WORK IS ORGANISED BY GOAL, NOT BY TASK. A goal is a container — an epic: the outcome being pursued, the \
thread where it is discussed, and the tasks that came out of that discussion. When a conversation is \
about an outcome rather than one piece of work, open a goal with goals_create, keep the reasoning on its \
thread with goals_add_comment, and create the tasks under it as they become clear — that order is the \
point, not a nicety. A goal is named by a handle like `RMS-G7`, which is what goes in `goal` on a task; \
its number comes from the same counter task numbers come from, so `RMS-7` and `RMS-G7` are never both \
real. A goal has no columns: it is open or closed. Its progress is counted from its tasks, so there is \
nothing to keep in sync — and closing it is refused until every one of those tasks is `done`, and \
requires a resolution comment, for the same reason landing a task does. Goals are never deleted. A task \
with no goal is a perfectly normal thing and not an unfinished one — but if a goal fits, put it there, \
because a board of loose tasks is a board nobody can see the shape of.\
\
EVERYTHING IS NAMED BY ITS HUMAN HANDLE. A project is its prefix, `RMS`. A task is `RMS-42` — the \
number as it reads, no leading zeros, and that is the form every tool hands back. The zero-padded \
`RMS-000042` is an older spelling of the same id and is still accepted on the way in, so an id quoted \
out of an old chat resolves; nothing produces it any more. Never name a task by its text: two tasks \
may read alike, and the text is the one part of a task that gets rewritten.\
\
AN ID FROM OUTSIDE THIS CONVERSATION IS NOT SAFE TO TRUST — USE tasks_resolve_id. A project's prefix \
can be renamed, and the freed prefix can then be taken by a different project, so `RMS-42` can mean \
one task today and a different one in the message you are reading it from. When an id comes from a \
tool call you just made, it is current and fine. When it comes from an older conversation, a commit \
message or a pasted link, resolve it first — the tool reports what the id means now AND which projects \
used to answer to it, and if there is any ambiguity you should say so before acting on it.\
\
COLUMNS ARE DATA, NOT A FIXED LIST. Every project has `todo` at one end and `done` at the other, \
always; everything between them is configured per project and has its id typed in by a person. A task \
whose stored status names a column that no longer exists reads as `todo` — the value is left alone, so \
the task returns to its column if that column is ever re-created. Practical consequence: filter and \
report using the ids projects_list gave you for THAT board, and do not assume a column called \
`in-progress` exists anywhere.\
\
KINDS ARE ALSO PER PROJECT, AND THEIR DESCRIPTIONS ARE THE POINT. `bug` on one board may be \
\"a customer-visible defect\" and on another \"anything that used to work\". Read the description \
before classifying, and if none of them fit, leave the kind off — it is optional. A task pointing at a \
kind that was deleted reads as having no kind.\
\
LABELS ARE JUST WORDS, AND THE SPELLING IS THE WHOLE CONTRACT. There are no definitions and no \
list to maintain: a label exists for exactly as long as some task wears it, a new one is created by \
using it, and the last task dropping it removes it. Which is why you must call labels_list (or read \
`labels` from projects_list) BEFORE tagging: `net-summary` and `netsummary` are two different tags, \
and once both exist every filter on either one is quietly wrong about half the board.\
\
AN ASSIGNEE IS AN EMAIL, NEVER A NAME. The board displays names and stores addresses, so a first name \
written into `assignee` names nobody at all. When someone is named in conversation — \"assign it to \
Yuri\" — call users_list and match the name to an address; pass the project so you only consider \
people who may actually see that board. If more than one person fits, ask which. Do not guess.\
\
A TASK CAN BE PUT ON AN AGENT. The literal `AI` is a valid assignee on EVERY board and means the task \
is an agent's to do rather than a person's — assign work to yourself with that, never with an address. \
It is not a user: it has no row on the roster, is never disabled, and needs no membership anywhere, \
which is why `users_list` always returns it first with `reserved` true. Neutral on purpose, so it does \
not have to change when whatever does the work does. Written in any case — `ai` reads the same as `AI`.\
\
DEPENDENCIES ORDER THE WORK, AND THEY DO NOT CROSS PROJECTS. A task lists the tasks blocking it in \
`depends_on` (ids of tasks on the same board; bare numbers work too). From that, every read derives \
`blocked` — true while any blocker is not `done`, INCLUDING an id that matches no task, so a mistyped \
or deleted blocker keeps the task blocked rather than silently freeing it. Do not start a blocked \
task, and do not move it into a working column. Nothing needs updating when a blocker lands: closing \
the last one clears `blocked` on the next read. Depending on a task that is already `done` blocks \
nothing.\
\
`blocks` IS THE HALF YOU CANNOT SEE FROM THE TASK. It is derived from the rest of the board and lists \
the tasks waiting on this one. Check it before parking, re-scoping or deleting something — \
`depends_on` tells you what is in your way, `blocks` tells you who you are in the way of, and only the \
second one is invisible unless you look.\
\
MARKDOWN IS THE FORMAT. A task's text and a comment's text are both rendered, so write them in it: \
**bold** for the headline of the work, `code` for identifiers, paths and commands, `-` bullets for a \
short checklist. Keep a task to a sticker's worth — a line or two, not a document. What does not fit \
belongs in the thread.\
\
THE THREAD IS FOR WHAT YOU LEARNED; THE TEXT IS FOR WHAT IS TO BE DONE. Findings, decisions, dead \
ends and questions go on as comments with tasks_add_comment — passing `who` (an email, or `AI`) and \
Markdown text; the moment is stamped for you. Rewrite the task's own text only when the work itself \
changed. A comment deliberately does not move the task's `updated`, so a busy thread does not read as \
active work. Every task reports `comments_amount`; when it is not zero, tasks_get_comments is worth \
reading before picking the task up — the reason the work is shaped the way it is usually lives there.\
\
PRIORITY IS WHAT DECIDES THE ORDER, AND THE ORDER IS WHAT YOU ARE GIVEN. A task and a goal each carry one \
of five values — `super-high`, `high`, `normal`, `low`, `super-low` — and every list comes back most urgent \
first, oldest first within one priority. That is the same order the board draws its columns in, so the top \
of `tasks_list` is the top of the column a person is looking at: to pick up the next thing, take the first \
one that is not `blocked`.\
\
`normal` IS THE DEFAULT AND MOST WORK SHOULD STAY THERE. The scale only says something while most tasks sit \
in the middle of it — a board where everything is `high` is a board with no priorities at all. Set one when \
the work is genuinely out of the ordinary, and ASK rather than guess when what you were told is vague: \
\"important\" and \"soon\" are not priorities. Re-ranking is a normal thing to do and takes effect on every \
open screen at once; there is no way to clear a priority, because `normal` is what having none means. A \
goal's priority is its own — it is not computed from its tasks, and a task does not inherit it.\
\
A CHECKLIST BREAKS ONE PIECE OF WORK DOWN INSIDE IT — IT IS NOT A SECOND BOARD. A task and a goal each \
carry `subtasks`: an ordered list of items, each with a one-line `title`, an optional longer `text`, and \
`done`. Write one with `add_subtasks` once you have read a task and worked out what it involves, and tick \
items with `check_subtasks` as you go — that is how the next reader sees where you got to without reading \
the whole thread. Every item is named by the `id` its checklist reports, never by its title, and an id \
that names no item is refused rather than ignored, because the alternative is reporting a tick that never \
happened.\
\
NOTHING IS DERIVED FROM A CHECKLIST, ON PURPOSE. An unticked item does not make a task `blocked`, does \
not stop it moving to `done`, and does not hold a goal open — a goal still closes on whether its TASKS \
are done. So the line is: if a step only matters to whoever is doing this one task, it is a checklist \
item; if somebody else has to see it, schedule it, be assigned it or depend on it, it is a task of its \
own under the same goal. Putting real work in a checklist hides it from the board, which is the one \
thing the board is for.\
\
A DOCUMENT IS A TEXT THE BOARD POINTS AT, NOT A LONGER TASK. Anything that outlives the work — a \
specification, a decision written up, a piece of reference — is a document: it lives at a path like \
`docs/design/system.md`, it belongs to one project, and tasks and goals REFERENCE it instead of copying \
it into their own text. That is the point: one place that is edited, rather than three copies that drift \
apart in silence. Write one with documents_upload, find one with documents_list, read one with \
documents_get, and attach it with add_documents on a task or a goal.\
\
A REFERENCE IS A URL, AND IT NAMES EITHER KIND OF DOCUMENT. What a task or a goal stores, and what \
every tool here accepts wherever it takes a document's `id`, is one of two forms:\
\
* `raw/{project}/document/{id}` — a document of the project's own, by the id that survives it being \
moved;\
* `raw/{project}/github/{repository}/{path}` — a file in a connected repository, by its path.\
\
BOTH ARE ATTACHABLE, which is the half that used to be missing: a specification living in a connected \
repository is attached to the work exactly as an uploaded one is, with no copying it in first. A url \
rather than a bare id because a file in a repository HAS no id — it is a working copy on disk, not a row \
— and because a reference then survives being written down: paste one into a CLAUDE.md, an issue or a \
message, hand it back to documents_get, and it reads. It is also the live address of the bytes, give or \
take a leading slash. You may still pass a bare id or the `id` and `path` a listing reports; what comes \
back is always the url.\
\
EVERY DOCUMENT CARRIES A BRIEF, AND SCANNING BRIEFS IS HOW YOU FIND ONE. A brief is a few sentences \
somebody wrote after reading a document — what it is, what it covers, which systems and decisions are \
named in it. It comes back on every documents_list row and every documents_get, so 'which document \
covers the settlement retries' is usually answered by ONE listing rather than by opening five \
documents. An empty brief means nobody has read that one for you yet, not that it is empty.\
\
IT IS FILED BY THE HASH OF THE CONTENT, WHICH IS WHY IT STAYS HONEST. The key is `content_hash`, so the \
same text stored twice — a file in a connected repository and a copy synced into a project — is briefed \
once and found by both, and an edit produces different bytes and therefore a document nobody has \
briefed again. A brief can never describe a text that has since changed.\
\
A DOCUMENT TOO LONG TO HAND OVER WHOLE COMES BACK AS ITS OPENING PLUS ITS OUTLINE, and the brief is \
still about the whole of it: the headings say what it covers end to end, `read_on_from_line` says where \
documents_get carries on, and two or three sections read deliberately are what a good brief of a long \
specification is made of.\
\
WRITING THEM IS PART OF THE WORK, NOT A CHORE AFTER IT. documents_upload and documents_edit hand you \
the new `content_hash`: file a brief with documents_set_brief in the same breath, while you still have \
the text in front of you. To catch up a board, documents_list reports `without_brief` and \
documents_next_without_brief hands them over one at a time until it says `none_left`. A repository that \
github_refresh has just cloned is the usual place to start — it arrives as a folder of texts nothing \
here knows anything about.\
\
A LARGE DOCUMENT IS WORKED ON IN PIECES, AND FOUR TOOLS EXIST FOR NOTHING ELSE. A specification can be \
tens of kilobytes; reading one whole to change a line, or reading five to find which mentions a thing, \
spends the context the work needed. So:\
\
* documents_search finds WHICH document, and on which line;\
* documents_outline turns one document into a kilobyte of headings, each with the line range of its \
section;\
* documents_get takes `from_line` / `to_line` — the outline's numbers, verbatim — and reads that \
section alone. It reports `truncated`, and a truncated read supports neither an edit nor a conclusion \
that something is not mentioned;\
* documents_edit changes the parts that change and leaves the rest untouched. Prefer it over \
documents_upload for anything you did not write in this conversation.\
\
DOCUMENTS_EDIT REFUSES RATHER THAN GUESSES, AND BOTH REFUSALS ARE THE POINT. Text that appears more \
than once is ambiguous and the whole batch fails, naming the count — quietly changing all seven \
occurrences of a word in an architecture document is how one goes wrong in a place nobody reads again. \
And `expected_version` refuses when the document moved since you read it: pass it whenever you read the \
document earlier in the conversation, because a person can upload over one from the browser at any \
moment and your edits would otherwise be spliced into a text you have never seen. Either way NOTHING is \
written — an edit batch is all of it or none of it, one new version per call.\
\
AFTER A WRITE, DIFF IT. documents_diff compares two versions and returns neither, so checking that a \
write did what you meant costs a few hundred bytes rather than two full texts. It is also what tells \
you what somebody else changed when an edit was refused on its version.\
\
THE PATH IS THE KEY; THE ID IS THE IDENTITY. Uploading to a path that is taken does not create a second \
document — it writes a new version of the one that lives there, keeping its id, its history and every \
reference to it. So documents_list BEFORE uploading: an upload to a path you did not mean to touch \
replaces what somebody put there. The id, on the other hand, never changes for as long as the document \
exists, which is why moving a document with documents_update_path breaks nothing, and why references are \
stored as ids and never as paths. Moving is a separate call from uploading on purpose — a history that \
could not tell a rewrite from a move would not answer the question it exists for.\
\
FOLDERS ARE NOT REAL. They are read off the paths of the documents in them, so an empty folder cannot \
exist and renaming one means moving every document under it, one call each.\
\
NOTHING ABOUT A DOCUMENT IS LOST. Every version is kept whole — documents_history says what happened, \
when, and who did it, for a live document and a deleted one alike. documents_delete puts a document in \
the trash rather than destroying it; documents_restore brings it back, by default exactly where it was. \
References to a deleted document are deliberately NOT cleaned up, because restoring is one call away and \
a reference quietly dropped would not come back with it. The trash is invisible in the browser: \
documents_trash is the only way to see what is in it.\
\
A CONNECTED REPOSITORY IS A REAL CLONE, AND IT IS READ-ONLY. A project may connect GitHub \
repositories; each appears in that project's documents as `github/<connection>/…`, and what is behind \
those paths is a working copy on this server. Reading works exactly as it does for the project's own \
documents: documents_list shows them side by side, documents_get reads one, documents_outline outlines \
one. documents_search does not reach them (it searches this product's own texts — use `git grep` \
through github_git for a repository), and documents_upload, documents_edit, documents_delete, \
documents_delete_folder and documents_update_path REFUSE them.\
\
THE REASON IS WHAT A REFRESH DOES: it clones the repository again and REPLACES the folder with the new \
copy. That is what makes a connection honest — what you read is what GitHub has, not what has \
accumulated on a disk — and it is why nothing may be written there: a file written into that folder \
would disappear at the next refresh with nothing left to say it had been there. To change one of these \
files, change it in the repository. To have a copy this board owns, with an id, versions and a history, \
sync it into the project's own documents. github_git still runs any git command in the clone — \
`git log`, `git diff`, `git show`, `git blame` — and anything it commits lives only until the next \
refresh, so push what is meant to last.\
\
A FILE IN A REPOSITORY HAS NO VERSION HERE, AND THAT IS NOT A GAP. documents_history, documents_diff \
and documents_restore refuse one, and say which git command answers the same question — `git log`, \
`git diff`, `git checkout`. Its history is the repository's and is complete; this product simply is not \
the thing keeping it. `expected_version` means nothing on one for the same reason: every read reports \
version 0, and `git status` is what tells you whether it moved under you.\
\
A BUILD IS RECORDED ON THE TASK THAT PRODUCED IT. When a change made in a task is built, put the \
GitHub Actions run on that task with `add_gh_actions` on tasks_update: the `url` of the run and a `title` \
saying what shipped — `my-service v1.2.3`. It is the reverse of a document: a document is what the work \
was done AGAINST, a build is what came OUT of it, and nothing else in this service records that \
connection. Record it as the build happens, ideally in the same call that lands the task; a link nobody \
attached is one somebody has to go and find in a CI history later. The same url twice is one build, the \
moment is stamped for you, and no check is made against GitHub — what is stored is what you said, so say \
it accurately.\
\
A RELEASE IS THE RECORD THAT A FEATURE WENT OUT, AND IT BELONGS TO THE PROJECT. Not to a task and not \
to a goal: it is a thing of its own, named `RMS-R12` — a number out of the same counter tasks and goals \
draw from, so `RMS-12`, `RMS-G12` and `RMS-R12` are never more than one real thing. Record one with \
releases_create when work ships. ONE release is one feature across however many microservices it \
touched: the release carries what changed — `title`, `description`, `release_notes` and the `date` it \
went out — and `services` carries one entry per microservice: its `microservice_id`, the `version` that \
went out, the `git_hash` that version was built from, and `datetime` — and, when CI built it, a \
`release_link` to the GitHub release or the workflow run that produced the image. A build link on a \
task says a change was built; a release says what is OUT, and it is the first thing to read when \
somebody asks which version of a service is deployed.\
\
A GOAL LISTS THE RELEASES IT WENT OUT IN. The goal is the description of the feature and the release is \
the record of it shipping, so pass `goal` to releases_create and the two are joined in that same call; \
a release recorded without one is attached later with add_releases on goals_update. Every goal then \
reports its `releases` whole. Normally a release ships one goal, and nothing enforces that — attach it \
to the goal it is actually about rather than to every goal it brushed against.\
\
A SETTINGS CHANGE IS WRITTEN WHERE IT CANNOT BE MISSED. Each service in a release has a \
`settings_update_note`, separate from its `description`: if rolling that service out needs its settings \
changed — a key added, a value changed, a secret supplied — that goes there and nowhere else, because \
it is the one part of a release somebody has to ACT on. Empty means the settings do not change. Read it \
before deploying a recorded version anywhere, and write it in the same call that records the service. \
A release names a microservice ONCE: passing one it already has to releases_update corrects that entry \
rather than adding a second.\
\
WHERE A RELEASE IS OUT IS A LIST OF LABELS ON IT, NOT A SECOND RELEASE. A release is recorded when it \
ships somewhere — usually a test stand first — so the list of releases is everything that went out, not \
everything that is live. Each release carries `envs`: the environments it is on, as one-word labels like \
`Dev` and `Prod`, in the order it reached them. Say where it is as you record it with `envs` on \
releases_create, and when the same versions are rolled out further add the label with `add_envs` on \
releases_update; do not record them again. releases_list filters by `env` — which, together with \
`microservice_id`, is how 'which version of this service is on prod' is answered in one row — and by \
`not_on_env` for what has not got there yet. Which environments exist is the project's own business: \
releases_list reports the labels a project uses in its own `envs`, and those are the spellings to \
reuse. Add a label when the rollout has HAPPENED, and take it off with `remove_envs` if the release is \
pulled back from there.\
\
A RELEASE IS CLOSED WHEN ITS ROLLOUT IS OVER. Once it has reached every environment it is going to, pass \
`done: true` to releases_update — normally in the call that adds the last environment. Every release \
reports `done`, and `done: false` on releases_list is the short list of releases somebody still has to \
do something about. It is a statement, not something worked out from `envs`: nothing here knows which \
environments a project has. Closing is not deleting and not a lock — a closed release stays on every \
list, and `done: false` reopens it.\
\
A RELEASE HAS A THREAD, AND IT IS FOR WHAT HAPPENED. `release_notes` say what changed; \
releases_add_comment is where the rollout itself is written down — it went clean, a setting was missed \
and added by hand, it was pulled back and why. `comment` on releases_update does the same in the call \
that adds the environment. A comment does not move the release's `updated`.\
\
A RELEASE IS NOT DELETED FOR BEING ROLLED BACK. It happened; take it off the environment it was pulled \
back from, and say what became of it on its thread. releases_delete is for one recorded by mistake, and like \
every deletion here it is a flag — `deleted: false` on releases_update brings it back, onto the goals \
that listed it too.\
\
MOVING A TASK TO `done` REQUIRES A COMMENT, AND THE MOVE IS REFUSED WITHOUT ONE. Pass `comment` and \
`comment_by` to tasks_update in the same call as the status change. Say what was actually done — what \
changed, and anything the next person should know — not that it is finished, which the column already \
says. The Done column is the whole reason a board is worth reading months later, and \"moved to done\" \
records nothing anybody can use. Only the transition INTO Done needs this: a task already there can be \
re-labelled or reassigned freely. `comment` is available on any update, not just this one — it is simply \
optional everywhere else.\
\
FINISHED WORK IS NOT DELETED. It moves to `done`, which is what keeps a board readable as a history. \
tasks_delete and goals_delete are for something that should never have been created — and deleting a goal \
is not closing it: closing says how it went, deleting says it should not be there.\
\
DELETION IS A FLAG, NOT A REMOVAL. What is deleted leaves every board, list and count, and stays exactly \
where it was: searching for its id still finds it and reports it as deleted. That is the point — an id \
coming back as \"no such task\" would be indistinguishable from a typo and from another board's id. Undo \
it with `deleted: false` on tasks_update or goals_update. A number is never reused either way.\
\
A NEW TASK ALWAYS STARTS IN `todo`. tasks_create takes no status — moving work on is tasks_update's \
job, which is also where landing it has to be explained. There is deliberately no way to create a task \
straight into Done.\
\
THE BOARD IS THE LAST FEW DAYS OF DONE, NOT ALL OF IT. Work closed longer ago than the project's \
archive window — seven days unless that project says otherwise — counts as archived: tasks_list leaves \
it out, and so does the board a person looks at. A closed goal ages off the same way, on the same clock. Done is the only column that \
grows for ever, and one nobody can read is one nobody looks at. Nothing is deleted — an archived task is \
still reachable by its id, and `include_archived` on tasks_list brings the history back when you are \
deliberately looking backwards. Practical consequence: \"this board has 12 tasks\" means twelve live ones, \
and a task you cannot find by listing may still exist.\
\
LIST BEFORE YOU CREATE, AND SEARCH BEFORE YOU LIST. tasks_create adds a task unconditionally, so \
calling it twice for the same work leaves two of them on a board a person reads by eye. tasks_search \
finds a near-duplicate filed under a wording you would not have guessed, and finds it across archived \
work too — \"how did we solve this last time\" is a question about work that has already landed.\
\
THIS BOARD IS A PLACE TO LOOK THINGS UP, NOT ONLY A PLACE TO FILE THEM. Two tools search, and reaching \
for them first is what stops a conversation being reconstructed from scratch every time:\
\
* tasks_search — over tasks, goals AND their comment threads. IT IS THE ONLY WAY TO SEE INSIDE A \
THREAD without already knowing which card to open: every listing reports `comments_amount` and not one \
comment, so without this the reasoning behind every decision here is reachable only by opening threads \
one at a time. The card says what is to be done; the thread says what was learned, what was tried and \
why the work is shaped this way — and that is usually the thing you came for.\
* documents_search — over the texts of a project's documents, returning matching lines and never a \
whole document. \"Which of these twenty-three documents mentions X\" is one call; reading them to find \
out spends the context the work needed on the twenty-one that do not.\
\
Both return LINES rather than objects, on purpose: a search you can afford is a search you make before \
guessing, and one that returned whole tasks or whole documents would cost what it was meant to save.\
\
ERRORS ARE TEXT, AND THEY ARE NEVER AN EMPTY RESULT. An unknown prefix, a column that does not exist \
on this board, an id that is not there — each comes back as a message that names the real options. An \
empty list means the board really is empty, and can be reported as such.";

/// The MCP surface, mounted on the same service-sdk HTTP server as the REST controllers.
///
/// Unlike the REST side this one **writes**: it is the whole mutation surface of the product. It has no
/// authorization in this version — it sees every project and writes to any of them, closed by the
/// perimeter alone. That asymmetry (reads gated by Google sign-in and project membership, writes gated
/// by nothing) is recorded in `TODO.md` as the first thing to fix.
pub fn build_middleware(app: Arc<AppContext>) -> McpMiddleware {
    let mut mcp = McpMiddleware::new(
        "/mcp",
        crate::app::APP_NAME,
        crate::app::APP_VERSION,
        INSTRUCTIONS,
    );

    // Registered in the order they are meant to be called, which is also the order they read in a
    // client's tool list.
    mcp.register_tool_call(Arc::new(ProjectsListHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(UsersListHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(LabelsListHandler::new(app.clone())));

    mcp.register_tool_call(Arc::new(GoalsListHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(TasksListHandler::new(app.clone())));
    // Beside the listing, because it is the other half of the same act: a listing answers "what is in this
    // state", a search answers "where was this discussed", and an agent arriving at a board needs both.
    mcp.register_tool_call(Arc::new(TasksSearchHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(ResolveIdHandler::new(app.clone())));
    // The last of the reads, because it is the last question asked of a piece of work: not what it is or
    // where it stands, but whether it is out.
    mcp.register_tool_call(Arc::new(ReleasesListHandler::new(app.clone())));

    mcp.register_tool_call(Arc::new(GoalsCreateHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(GoalsUpdateHandler::new(app.clone())));

    mcp.register_tool_call(Arc::new(TasksCreateHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(TasksUpdateHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(TasksDeleteHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(GoalsDeleteHandler::new(app.clone())));

    // After everything that changes the work, in the order the work itself goes: a goal is opened, tasks
    // are done under it, and then it ships.
    mcp.register_tool_call(Arc::new(ReleasesCreateHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(ReleasesUpdateHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(ReleasesDeleteHandler::new(app.clone())));

    // Documents. After the board tools, because a document is read in the course of doing work rather than
    // to find out what the work is — and in the order a large one is actually approached: find which
    // document, see its shape, read the part that matters, then write.
    mcp.register_tool_call(Arc::new(DocumentsListHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsSearchHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsOutlineHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsGetHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsHistoryHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsDiffHandler::new(app.clone())));

    // The edit before the upload, which is the order they should be reached for: an upload replaces a whole
    // text, and on anything large that is the expensive way to change a sentence.
    mcp.register_tool_call(Arc::new(DocumentsEditHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsUploadHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsUpdatePathHandler::new(app.clone())));

    // The single deletion first, and the folder one straight after it: the second is the first repeated,
    // and a reader of this list should meet them in that order rather than discover the bulk one on its own.
    mcp.register_tool_call(Arc::new(DocumentsDeleteHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsDeleteFolderHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsTrashHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsRestoreHandler::new(app.clone())));

    // The briefing loop, after the tools that produce the documents it reads: hand me the next unread
    // one, and here is what it says.
    mcp.register_tool_call(Arc::new(DocumentsNextWithoutBriefHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(DocumentsSetBriefHandler::new(app.clone())));

    // After the documents tools, because that is the order the work happens in: a change is made with
    // those, and this is what records and sends it. It is also the only tool here that does not act on
    // the board at all.
    mcp.register_tool_call(Arc::new(GithubGitHandler::new(app.clone())));

    // And the one that brings a repository down again, which is where a board's briefing usually starts:
    // a folder of files nothing here has read yet.
    mcp.register_tool_call(Arc::new(GithubRefreshHandler::new(app.clone())));

    mcp.register_tool_call(Arc::new(GoalsAddCommentHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(GoalsGetCommentsHandler::new(app.clone())));

    mcp.register_tool_call(Arc::new(ReleasesAddCommentHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(ReleasesGetCommentsHandler::new(app.clone())));

    mcp.register_tool_call(Arc::new(AddCommentHandler::new(app.clone())));
    mcp.register_tool_call(Arc::new(GetCommentsHandler::new(app)));

    mcp
}

#[cfg(test)]
mod tests {
    /// The checklist fields are the first **nested objects** on this surface — every other input is a scalar
    /// or a list of strings. Nothing in the service exercises schema generation, so a shape the derive
    /// cannot describe would first be noticed by a client asking for the tool list, which is a long way from
    /// here. Proved in a test instead, on both tools that take one.
    #[tokio::test]
    async fn the_write_tools_describe_their_checklist_fields() {
        let tasks = super::tasks_write_tool_calls::TasksUpdateInput::get_json_schema(false)
            .await
            .build();

        let goals = super::goals_tool_calls::GoalsUpdateInput::get_json_schema(false)
            .await
            .build();

        for schema in [tasks, goals] {
            for expected in [
                "add_subtasks",
                "check_subtasks",
                "uncheck_subtasks",
                "edit_subtasks",
                "remove_subtasks",
                // From the nested item itself, which is the half a flat-only schema would lose.
                "title",
            ] {
                assert!(
                    schema.contains(expected),
                    "the schema does not mention {expected}: {schema}"
                );
            }
        }
    }

    /// A release's services are a nested object too, and the one this surface can least afford to lose: a
    /// client that could not see `git_hash` or `settings_update_note` would record releases with neither,
    /// and the tool would either refuse every call or — worse — accept a release that says nothing.
    /// Checked on both tools that take the list, and on the goal tool that takes the references.
    #[tokio::test]
    async fn the_release_tools_describe_their_service_objects() {
        let create = super::releases_tool_calls::ReleasesCreateInput::get_json_schema(false)
            .await
            .build();

        let update = super::releases_tool_calls::ReleasesUpdateInput::get_json_schema(false)
            .await
            .build();

        // Where a release is out is said on both tools, under different names: the first states the
        // environments it starts on, the second adds to them and takes away.
        for (schema, list, envs) in [
            (create, "services", "envs"),
            (update, "add_services", "add_envs"),
        ] {
            for expected in [
                list,
                // From the nested item itself, which is the half a flat-only schema would lose.
                "microservice_id",
                "version",
                "git_hash",
                "release_link",
                "datetime",
                "settings_update_note",
                envs,
                // Closing is on both: almost never on the first, the whole point of the second.
                "done",
            ] {
                assert!(
                    schema.contains(expected),
                    "the schema does not mention {expected}: {schema}"
                );
            }
        }

        let goals = super::goals_tool_calls::GoalsUpdateInput::get_json_schema(false)
            .await
            .build();

        for expected in ["add_releases", "remove_releases"] {
            assert!(
                goals.contains(expected),
                "the schema does not mention {expected}: {goals}"
            );
        }
    }

    /// The deepest nesting on this surface, and it is on the way OUT: a project carries its goals, a goal
    /// its releases, a release its services — four objects deep, on the answer to the very first call an
    /// agent makes. If the derive could not describe that, it would be `projects_list` that broke, which is
    /// the one tool nothing else works without.
    #[tokio::test]
    async fn a_release_is_described_wherever_it_is_nested() {
        let projects = super::projects_list_tool_call::ProjectsListResponse::get_json_schema(false)
            .await
            .build();

        let goals = super::goals_tool_calls::GoalsListResponse::get_json_schema(false)
            .await
            .build();

        let releases = super::releases_tool_calls::ReleasesListResponse::get_json_schema(false)
            .await
            .build();

        for schema in [projects, goals, releases] {
            for expected in [
                "releases",
                "release_notes",
                // From the innermost object, which is the one a shallower schema would lose.
                "microservice_id",
                "git_hash",
                "release_link",
                "settings_update_note",
                // And the one list on a release that is of plain strings.
                "envs",
            ] {
                assert!(
                    schema.contains(expected),
                    "the schema does not mention {expected}: {schema}"
                );
            }
        }
    }

    /// `edits` is the third nested object on this surface, and the one where a schema the derive cannot
    /// describe would be worst: a client that could not see `old_string` and `new_string` would send
    /// something shaped differently, and the tool would refuse every call for a reason nobody could read
    /// from the tool list. Same failure mode as the two above, one tool.
    #[tokio::test]
    async fn documents_edit_describes_its_edit_objects() {
        let schema = super::documents_text_tool_calls::DocumentsEditInput::get_json_schema(false)
            .await
            .build();

        for expected in [
            "edits",
            "expected_version",
            // From the nested item itself, which is the half a flat-only schema would lose.
            "old_string",
            "new_string",
            "replace_all",
        ] {
            assert!(
                schema.contains(expected),
                "the schema does not mention {expected}: {schema}"
            );
        }
    }

    /// The build links are the other nested object here, and unlike the checklist they are on ONE tool — a
    /// task produces builds, a goal does not — so they get their own test rather than a line in the loop
    /// above. Same failure mode either way: a shape the derive cannot describe is first noticed by a client
    /// asking for the tool list.
    #[tokio::test]
    async fn tasks_update_describes_its_build_fields() {
        let schema = super::tasks_write_tool_calls::TasksUpdateInput::get_json_schema(false)
            .await
            .build();

        for expected in [
            "add_gh_actions",
            "remove_gh_actions",
            // From the nested item's own `url` property, which is the half a flat-only schema would lose —
            // this phrase appears nowhere else.
            "actions/runs/<id>",
        ] {
            assert!(
                schema.contains(expected),
                "the schema does not mention {expected}: {schema}"
            );
        }
    }
}
