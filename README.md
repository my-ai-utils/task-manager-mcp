# task-manager-mcp

A task board that **agents work and humans configure**. Every task mutation arrives through MCP,
with two named exceptions the browser owns: **moving a card between columns**, and **a goal's
colour**. Nothing else is edited with a mouse — not a task's text, not its type, not who is on it,
not a thread.

That split is the whole design. It is not a Jira with an MCP bolted on — the MCP surface is the
primary interface, and the UI exists because projects, columns, kinds and people have to be
configured by a person, and because someone wants to see the board.

The two exceptions are deliberate and each is one field wide. A colour is presentation rather than
state, and task types have always been coloured with a mouse. A move is the one gesture a board is
expected to have; refusing it taught people the screen was broken rather than that it was a viewer.
Both go through the same scripts and the same validation an MCP call does — a landing in `done`
still owes a comment — and the move signs that comment with the session, which is the one thing this
door does better than MCP, where the author is a string the caller passes.

Product namespace on the host: `task-manager-mcp`. Successor to `rms/development-tasks-mcp`, which
this replaces once the first version lands.

## One service, three crates

One repository, one deployable, one container. The layout is `my-no-sql-server`'s:

```
Cargo.toml, src/   the server — crate `task-manager`
shared/            the wire models, a path dependency of both sides
ui/                the browser client, a Dioxus crate
wwwroot/           the client, BUILT — committed, and what the image serves
Dockerfile         the binary plus wwwroot
build-ui.sh        ui/ -> wwwroot/
release/           docker-compose, the settings template, the proxy config
```

| | |
|---|---|
| **`task-manager`** (the root) | service-sdk HTTP. Three surfaces on one port: `/api/v1/*` (reads for the UI, configuration CRUD, and the two writes the board owns — a task's column and a goal's colour), `/mcp` (every other task mutation), `/ws` (the whole board, pushed). Owns Postgres. And it serves the client: everything that is not one of those is the bundle in `wwwroot/`. |
| **`task-manager-ui`** (`ui/`) | Dioxus CSR (`dioxus/web`), a static bundle. Talks to the REST API through `flurl`, on relative URLs — it is served by the server it calls. |
| **`task-manager-shared`** (`shared/`) | Wire models, shared verbatim by both. WASM-clean by default; a `server` feature gates `MyHttpInput` / `MyHttpObjectStructure`. |

There is no workspace: the three are separate crates with their own `target/`, and `cargo build` at the
root builds the server alone.

**The client was a container of its own, and is not any more.** It used to ship as a second image on
`web-app-host`, behind a second port, with a reverse proxy sending `/api`, `/ws` and `/mcp` one way and
everything else the other. But it is a folder of static files, and the server is already listening on
the origin those files call back to — so it serves them, and the product is one image, one port, one
upstream and one version number.

**`UiMiddleware` is what serves it, and it is not the stock middleware bare.** The client is a
single-page app, so a path that names no file — `/goals`, `/releases`, Google's redirect to `/authorized`
— has to answer with `index.html`. `StaticFilesMiddleware` does that for *every* request it is shown,
whatever the method; and service-sdk runs custom middlewares **before** the controllers, where
`my-no-sql-server`, which assembles its server by hand, puts the static files last. Registered bare it
would answer `POST /api/tasks/v1/list` with a page of HTML and a 200 — every call succeeding and none of
them parsing. So the wrapper decides two things first: only a `GET` is a browser fetching a page, and a
path under `/api`, `/mcp`, `/ws`, `/raw`, `/swagger` or `/metrics` is the server's own to answer,
including to answer 404. The list is of the server's paths rather than the client's on purpose: a new
screen is added far more often than a new surface, and a screen that had to be registered on the server
to survive a reload would be forgotten the first time. Files go out with an `ETag` and
`Cache-Control: no-cache`, so a visit costs a `304` rather than the whole wasm and a deploy is picked up
by the next reload. It is tested over a real socket against the committed bundle, with a stand-in for
the controllers placed where service-sdk puts them.

**Deliberate deviation from `architect-playbook`.** By the archetype table an internal employee
admin panel is `admin-ui` (Dioxus fullstack, server functions in the deployable), and the
`rest-api` + CSR-`ui` pair is reserved for external `client-ui`. We take the pair anyway — as two
crates in one deployable — and the server owns Postgres directly rather than fronting a `grpc-flows`
domain-owner. Reason: there is exactly one consumer plus the MCP surface, both in the same process, and
`rms/dashboards-rest-api` + `dashboards-ui` is the same shape already in production. Splitting a
single responsibility by transport would buy a proto file and two layers of mappers and nothing
else.

## Everything lives in memory

Postgres is where the data is **durable**. Memory is where it is **read**. On startup the service
loads all of it — projects, columns, kinds, tasks, comments, users, membership — builds its
indexes, and from then on no read touches the database.

**`scripts/` is the only write path, and the order inside it is fixed: Postgres first, then
memory.** If the database write fails, memory is left alone. The reverse order would produce a
state that exists in the process and not on disk — and it would survive right up to the next
restart, where it would silently roll back.

What this buys, beyond speed: the id counter has no race (it is a field under a lock, not
`MAX(number)+1`), prefix uniqueness and the `RMS-42` → project resolution are in-process lookups,
and `blocked` / `blocks` / the label set are derived on every read for free.

**There is one exception, and it is narrower than it sounds: a document's PAYLOAD.** Everything *about* a
document — its path, size, kind and id — is held in memory like everything else, in its own collection beside
the board. What is not is the bytes: a document can be a PDF, and the board is pushed *whole* down a WebSocket
on every change, so caching payloads would ship every file on the board to every open screen every time
anybody moved a sticker. Payloads are read from Postgres one document at a time, when somebody opens one. See
**Documents** below.

What it costs: **the service is single-instance.** State is authoritative in the
process and the WebSocket fan-out is in-process, so a second replica would both diverge and fail
to notify. This is the playbook default for state-bearing services; here it is a constraint, not
a default.

## Project

The unit of everything. A task cannot exist outside one.

| Field | |
|---|---|
| `id` | `rust_extensions::SortableId` — `{unix_micros}-{uuid}` truncated to 25 chars. Chronologically sortable, never shown to a user. |
| `name`, `description` | Free text. Editable. |
| `prefix` | The human-facing half of a task id: `RMS` → `RMS-42`. Renameable. |
| `prefix_history` | Every prefix this project has ever had. |
| `columns` | Ordered. See below. |
| `kinds` | See below. |
| `members` | Users with access. Edited here, from the project's side. |
| `archived_moment` | When the project was put away, or absent for one that is not. See **Archiving a project is a flag too**. |

**Two projects may not hold the same prefix at the same time** — the second is refused. A prefix is
renameable, and one that has been renamed away from is free for another project to take. Every
prefix a project ever carried is kept, so `RMS-42` still resolves after a rename: the resolver looks
for a project holding `RMS` **now**, and only then falls back to prefix history.

**Which is why a task's human id is not stored.** A per-project counter plus a reusable prefix means
two projects can both produce `RMS-1` — project A while it held `RMS`, project B after taking it
over. Materialising that string would put two different tasks under one id. So the row holds only
`number`, and `RMS-42` is composed on read from the project's *current* prefix. Nothing can
collide, "the current holder wins" is automatic rather than a rule to enforce, and the cost is
explicit: after a rename A's tasks read as `TM-1`, and the old `RMS-1` link dies at the moment
another project takes `RMS` over.

For the same reason **`depends_on` holds task numbers, not id strings** — a rename would otherwise
break every dependency in the project at once.

Deleting a project is not implemented, and is not going to be — prefix history does not protect
against it, and a board's tasks, goals, comments and documents all hang off it. What exists instead
is **archiving**: a project is put away rather than removed. See **Archiving a project is a flag
too** for exactly what that hides and what it deliberately does not.

### Columns

`todo` (leftmost) and `done` (rightmost) exist in every project and are not configurable.
Everything between them is: the user enters the column's id by hand, plus a name, a description
and its position in the order.

- **A column id is never renamed.** Only deleted.
- **A column is deleted freely**, whatever is in it.
- **A task whose status matches no column of its project reads as `todo`.**

That last rule is why there is **no foreign key** from a task's status to a column: an orphaned
status has to survive, not be rejected by the database. Side effect worth knowing — the stored
status is untouched, so re-creating a column with the same id brings its tasks back to it.

### Kinds — "task types" on screen

Same shape as columns — id entered by hand, name, description — plus a **colour from a preset set** (an
enum in `task-manager-shared`, validated by the server) and an optional **icon**. Both are drawn by the UI
as clickable choices rather than dropdowns of names, because a picker that shows the thing beats one that
spells it. Unlike columns, kinds have no mandatory anchors: a project may have none.

The icons are SVG files in `ui/public/assets/images/task-icons`, and the list the picker
offers is **generated by `build.rs` from that directory** — dropping a file in and rebuilding is the whole
job. A kind stores only the file's stem (`bug`, `tech-debt`), and the server never validates it against a
list: the files ship in the UI bundle, so an unknown name draws as no icon, the same leniency an unknown
colour or column id gets. On a tag the icon is painted white with `brightness(0) invert(1)`, so one icon
set reads correctly on all eight tag colours without editing any file's fill.

The wire name stays `kind` everywhere — the API, the models, the MCP tools — while the screen says "task
type". Deliberate: `kind` is a contract with every agent already calling `tasks_create` with it.

### Labels

Per project, and nothing more than a name — no description, no rules, no meanings. A task picks
an existing one or introduces a new one by typing it. Consequently there is no label table: a
project's label set is the distinct labels in use across its tasks, and a label stops existing
when the last task drops it.

This is a deliberate simplification of `development-tasks-mcp`, where labels were a shared
vocabulary with written definitions, a `used_by` count and an "in use but undefined" report. That
machinery is gone.

### The reserved assignee `AI`

One assignee is not a person and has no user row: the literal `AI`, meaning the task is an agent's to do.
Neutral on purpose, so it does not have to be renamed when whatever does the work changes.

Deliberately not a row on the roster — a row would have to be added to every project's membership to be
assignable, and disabling it would read as disabling a colleague. It is assignable on every board, always,
and `users_list` returns it first with `reserved` true, which is how an agent learns it may put work on
itself.

Stored exactly as declared whatever case it arrives in, while an address is lower-cased. One function
(`normalise_actor`) does both for an assignee and for a comment's author, because a comment signed `AI` and
a task assigned `AI` have to be the same string or a filter on one would miss the other.

## Task

| Field | |
|---|---|
| `project_id` | Owner. |
| `number` | Sequential within the project, from the project's own counter. Not reused after a delete. |
| — | The human id `PREFIX-42` is **composed on read**, never stored — see above. Not padded: the number as it reads. The zero-padded `PREFIX-000042` this board used to hand out still parses, so an id quoted from an old chat or link resolves. |
| `text` | Markdown — the UI renders it. |
| `status` | A column id of this project. Unknown → reads as `todo`. **Always `todo` on creation** — `tasks_create` takes no status. |
| `priority` | One of five: `super-high`, `high`, `normal`, `low`, `super-low`. Decides where the card sits in its column. See below. |
| `kind` | A kind id of this project. Optional. |
| `assignee` | An email, or the reserved `AI`. |
| `labels` | Free tags, lowercased and de-duplicated. |
| `depends_on` | **Numbers** of blocking tasks, within the same project. |
| `subtasks` | A checklist. Each item: a `SortableId`, a one-line `title`, a longer Markdown `text`, and `done`. See below. |
| `gh_actions` | The builds this work produced. Each one: the `url` of a GitHub Actions run, a `title`, and when it was attached. See below. |
| `comments` | A thread. Each comment: moment, `who`, Markdown text. |
| `close_moment` | When the task landed in Done; absent whenever it is not there. Cleared on re-open, so a re-closed task is dated by its latest close. |
| `created`, `updated` | A comment does not move `updated` — the thread is a separate record from the work. |

**Landing work has to say what was done.** Moving a task into Done without a `comment` is refused — the
Done column is the whole reason a board is worth reading months later, and "moved to done" records nothing
anybody can use. Only the *transition* is gated: a task already there can be re-labelled or reassigned
freely. And since `tasks_create` cannot set a status, creating a task straight into Done is not a way
around it.

**The board is the last seven days of Done, not all of it.** Work closed longer ago than that counts as
archived: it is left out of the board and out of `tasks_list`. Done is the only column that grows for
ever, and one nobody can read is one nobody looks at. Nothing is deleted — an archived task is still
reachable by its id, and `include_archived` brings the history back. The window is measured from
`close_moment`, not `updated`, so editing an old finished task does not drag it back onto the board.

**`blocked` and `blocks` are derived, never stored.** `blocked` is true while any id in
`depends_on` names a task that is not `done`; an id matching no task counts as still-blocking, so
a typo or a deleted blocker does not silently free the task. `blocks` is the reverse edge, read
off the rest of the board — the only way to see who is waiting on you. Closing the last blocker
clears `blocked` on the next read, with nothing to update by hand.

## Priority — five steps, and it decides the order

A task **and a goal** each carry one of `super-high`, `high`, `normal`, `low`, `super-low`. A fixed scale
rather than a number: a number invites 7-vs-8 arguments and drifts upward until everything is a 9, whereas
five named steps make somebody choose between *this* and *that*. Unlike columns and kinds it is **not**
per-project — urgency does not mean something different on another board, and one scale is what lets two
boards be read side by side.

**The order is the whole feature.** Every list the server hands out — the board read, the WebSocket push,
`tasks_list`, `goals_list`, a goal's own task list — comes back **most urgent first, oldest first within one
priority**. Sorted in `BoardInner` rather than by each reader, so the board, the Goals screen and every agent
agree about what is at the top of a column, and a client that only groups the list into columns is already
right. Within one priority the tiebreaker is the task number, which is the order the board had before
priorities existed — so nothing moved for work nobody has ranked.

On the board, priority **outranks the goal grouping**: a Super High card is at the top of its column, not at
the top of its own group. The grouping (cards under a goal above the loose pile, cards of one goal adjacent)
is what breaks a tie between two cards ranked the same. The alternative would make Super High mean "near the
top of its own group", which is not a priority at all.

`normal` is the default, and a row written before the field existed reads as `normal` — as does a value this
build does not recognise, the same leniency a colour gets. **A write is the opposite: an unreadable priority
is refused**, naming the five, because a write is somebody deciding and quietly turning a typo into Normal is
how work nobody meant to deprioritise sinks. Spelling is lenient in both directions (`Super High`,
`super_high`, `superhigh`); the refusal is for values that mean nothing.

There is no way to clear it. `normal` *is* the absence of a ranking, so a field that could also be empty would
have two spellings for one state.

**A card shows the badge only when the priority is not Normal** — the position in the column already says it,
and a tag repeated on every card is a tag nobody reads. The two dialogs show it always, Normal included: that
is where a fact is looked up, and a row that vanishes is ambiguous where one saying "Normal" is not.

A goal's priority is **its own**, not derived from its tasks: an epic can be urgent while none of its work has
started, and a number computed from the work would take the decision away from the person making it. Nor does
a task inherit its goal's.

## The checklist — subtasks that never leave the task

A task **and a goal** each carry an ordered list of checklist items: an id, a one-line `title`, a longer
Markdown `text`, and `done`. It lives in a `jsonb` column on the row it belongs to, exactly like the
thread and for the same reason — ticking an item is then one atomic upsert of one row.

**Nothing is derived from it, and that is the whole design.** An unticked item does not make a task
`blocked`, does not stop it moving to `done`, and does not hold a goal open — a goal still closes on
whether its *tasks* are done. So a checklist somebody abandoned half-way is not a state the board has to
have an opinion about, and the rule an agent has to learn is one line: if a step only matters to whoever is
doing this one task, it is a checklist item; if somebody else has to see it, schedule it, be assigned it or
depend on it, it is a task of its own under the same goal. The failure this prevents is real work hidden
inside a card, which is the one thing a board exists to stop.

Which is also why it is **not** folded into a goal's `done_amount` / `tasks_amount`. Those count tasks and
are what decides whether the goal may close; mixing a private breakdown into them would make one counter
mean two things.

**An item is named by its id, never by its title** — two items may read alike, and a title is the part that
gets rewritten. The id is a `SortableId` minted server-side, is not a task handle, resolves nowhere else,
and is never shown to a person: the screen draws the title. It is not reused after a removal, so an id
quoted from a stale read names nothing rather than the wrong item.

**Edits are operations, not a new list.** `tasks_update` / `goals_update` take `add_subtasks`,
`edit_subtasks`, `check_subtasks`, `uncheck_subtasks` and `remove_subtasks`, applied in that order — the
same shape and the same reason as `add_labels` / `remove_labels`. A whole-list write would silently drop
whatever the caller did not know about, which on a list two people are adding to is a lost item nobody
notices. An id naming no item is **refused**, unlike a label that is not there: an unknown id means the
caller is working from a read that has moved on, and quietly doing nothing would report a tick that never
happened.

Ticking an item **does** move `updated` — a checklist is the work, not the conversation about it, which is
what separates it from a comment.

In the browser it is read-only, like everything else about a task: the dialog draws the titles under the
text, greys out what is done and marks it with a tick, and a click expands one item's `text` (an item with
no text is drawn as a plain row, so there is no affordance for an expansion that would show nothing).

**Both jsonb columns are nullable**, and not because "no checklist" needs its own spelling — an empty array
says that perfectly well. The columns arrive on tables that already have rows, and the schema generator
derives a column's nullability from the Rust type: a non-`Option` field would emit
`alter table … add subtasks jsonb not null`, which Postgres refuses on a populated table. `NULL` reads as an
empty checklist and every write puts a real array in. Same arrangement, and same reason, as `goals.color`.

## Builds — the link from the work to what shipped

A task carries `gh_actions`: the GitHub Actions runs that came out of it, oldest first. Each entry is a
run `url`, a `title` — `my-service v1.2.3` — and the moment the link was attached. It lives in a `jsonb`
column on the task row, exactly like the thread and the checklist, so recording a build is one atomic
upsert of one row and the links ride along in the board snapshot with nothing to resolve.

**It is a document reference pointing the other way, and that is why the two are drawn next to each
other.** A document is what the work was done *against*; a build is what came *out* of it. Nothing else
in this service connects a piece of work to the artefact it produced, and the question it answers —
"this change, did it ship, and as what" — is asked long after the task is closed, when the CI history is
the only other place to look and nobody remembers which run it was.

**The url is the identity.** A run already has an id and it is in the url, so nothing is minted here:
adding a url the task already carries is *not* a duplicate and not an error — the same build re-reported
is what a re-run looks like from here. The existing entry keeps its moment, because that is when this
build was recorded against the work, and takes the new title if one was given, since a caller bothering
to name it a second time is correcting the first. Removing names the url; one that is not there is not
an error, exactly as with a label.

**Nothing is fetched.** A build link is stored exactly as the caller said it, and nothing here ever asks
GitHub about it — not even now that connected repositories give this service a way to. The two are
unrelated on purpose: one is a clone of a repository somebody chose, the other is a url on a card. So
there is no run status here, nothing goes stale, and the moment is when the link was *attached* rather
than when GitHub ran anything: a time read off a url nobody fetched would be a guess dressed as a
record. For the same reason the host is not checked against `github.com` — an enterprise install answers
on its own domain, and refusing a real build over its hostname would be a cosmetic rule with a real cost.
What *is* checked is that the value is an `http(s)` link at all: a bare run number stored here would draw
a row nobody can click.

**A title is optional and never empty.** When nobody writes one, it is worked out from the url —
`…/<owner>/<repo>/actions/runs/<id>` becomes `my-service #18423`, and anything else falls back to the
host and the last segment. Requiring one would only have produced names copied out of the url by
whoever was in a hurry; a derived name says the same thing and is honest about being derived.

**Only `tasks_update` writes them, through `add_gh_actions` / `remove_gh_actions`** — the same add/remove
shape as labels and document references, and for the same reason: a task collects builds one at a time
over the life of the work, so a whole-list write would drop whatever the caller had not read.
`tasks_create` deliberately takes none: a build is what comes out of the work, so it cannot exist before
the task that produced it.

In the browser the task dialog draws them under the documents, one per line: the title as a link, the
date beside it, the url as the tooltip. They are the one thing on that screen that leaves the product, so
they open in a new tab — the board stays where it was, and the back button is not the way home from a CI
log. Most tasks produced no builds and draw nothing at all rather than an empty heading.

## Releases — the record of what went out

A build link says a change was *built*. A release says a feature is **out**, and in what: which version
of which microservice, built from which commit, and whether anything has to change in its settings. It is
the third kind of thing on a board, beside tasks and goals, and unlike a build link it is a thing of its
own rather than a line on a card.

**A release belongs to the project, not to a goal.** Its own table, `releases`, keyed `(project_id,
number)` exactly as a task and a goal are, with the number drawn from the same per-project counter — so
`RMS-12`, `RMS-G12` and `RMS-R12` are never more than one real thing, a bare number still names exactly
one, and `tasks_resolve_id` answers for all three. The handle `RMS-R12` is composed on read and is not
stored, for the reason a task's is not: a prefix moves between projects.

**One release is one feature, across however many microservices it touched.** The release carries what
changed for a reader — `title`, `description`, `release_notes` and the `date` it went out — and
`services` carries what was deployed, one entry per microservice:

| | |
|---|---|
| `microservice_id` | The name the service is deployed under. The identity of the entry within its release. |
| `version` | Which version of it went out. |
| `git_hash` | The commit that version was built from. |
| `release_link` | Where the build of that version can be looked at — the GitHub release, or the run that built the image. Empty when there is none. |
| `datetime` | When this service went out. |
| `settings_update_note` | What has to change in this service's settings. Empty when nothing does. |
| `description` | Anything else worth saying about this service's part of the release. |

The services ride on the release row as `jsonb`, for the reason a checklist rides on its task: adding a
service is one atomic upsert of one row, and nothing is ever asked of them across releases that the
in-memory board does not answer.

**The goal lists its releases; the release does not know its goal.** A goal is the description of a
feature and a release is the record of it shipping, so a goal row carries `releases` — the numbers of the
releases it went out in — and every read of a goal hands them back whole. The link is on the goal rather
than on the release so that a release stays a plain log entry: one written down before anybody decided
which goal it belongs under is still a release, and it is on the Releases screen either way. Normally a
release ships exactly one goal. **Nothing enforces that** — the same release can be put on a second
goal's list — because a rule that guessed which of two goals a release "really" belongs to would be
wrong exactly when it mattered. The other direction, which goals list a release, is derived on read and
stored nowhere.

**A settings change has a field of its own, and that is the reason the field exists.** Whether a service's
settings have to change is the one fact about a release somebody *acts* on while rolling it out; a fact
that has to be found in prose gets missed. So it is `settings_update_note` on the service it concerns and
not a line in the notes, the folded row on the Releases screen carries a flag when any service has one,
and the open release draws it as a block of its own under that service. Everything else goes in
`description`.

**Where a release is out is a list of labels on it, not a second release.** A release is written down
when it ships *somewhere* — usually a test stand first — so the list of releases is everything that went
out, not everything that is live. Each release carries `envs`: the environments it is on, as labels —
`Dev`, `Prod` — in the order it reached them. When the same versions of the same services are rolled out
further, the label is added to the release that is already there (`add_envs` on `releases_update`), and
`remove_envs` takes one off for a release pulled back. Those labels are what "what is actually on prod"
is filtered by: `releases_list` takes `env` and `not_on_env`, and the Releases screen offers each
environment both ways round. With a microservice picked beside it, the top row is the version of that
service users are on.

Labels and not a flag per environment, because which environments exist is the project's own business:
one has Dev and Prod, another a stand per client. Nothing stores the list — an environment exists for as
long as some release carries its label, the way a board's labels are read off its tasks — and
`releases_list` reports the ones a project uses so that a caller can reuse them. Three rules keep a
free-text field from drifting. A label is **one word**: it is an identity, what a list is filtered by,
and `Pre Prod` typed once and `Pre-Prod` the next time would be two environments for one stand. A label
is **one environment however it is cased**, on a release and in every filter. And there is **one
spelling per project**: a label that matches one the project's releases already carry is stored in
*that* spelling, so three agents writing `Prod`, `prod` and `PROD` come out as one chip.

The column is `envs`, `jsonb`, and nullable — it arrived on a table 0.2.0 had already created. That
build kept a single fact instead, a moment in `released_on_prod_moment`; a row it marked and nobody has
written since still has that and a NULL `envs`, and it loads as the label `Prod`. Every write stores a
real array and empties the old column, so it drains as rows are touched, and the schema sync never drops
a column, so the field can simply be deleted from the row model once nothing predates `envs`.

**`release_link` is a link somebody pasted, and it is held to being one.** When CI built what went out,
the service entry carries where that build can be looked at — the url of the GitHub release, or of the
workflow run that built the image — so the build is one click from the record of it. Nothing here talks
to GitHub: it says where to look, not whether the build passed. It has to be an `http(s)` address with no
whitespace in it, because whatever is stored is drawn as an anchor; it is **not** held to `github.com`,
for the reason a task's build link is not — an enterprise install and another CI are builds too.

**A release has a thread, and it is for what happened.** `release_notes` say what *changed*; the thread
says how the rollout went — clean, a setting missed and added by hand, pulled back and why. It is the
same shape as a task's and a goal's and rides on the release row as `jsonb` for the same reason: one
atomic write per comment. `releases_add_comment` writes one, `comment` on `releases_update` writes one in
the same call that adds an environment, and neither moves the release's `updated`. An author is demanded,
as on a goal: MCP has no session to take one from.

**`git_hash` has to be a hash.** 7 to 64 hex characters, lower-cased on the way in; a branch or a tag is
refused. A version is not held to anything, and the difference is deliberate: the commit is the half of
the entry that cannot have moved since. `main` or `v1.2.3` stored here would be a record that reads as
precise and names nothing in particular.

**A release names a microservice once.** Passing one it already has is a correction of that entry, in
place: the version and the commit are replaced — stating them is what the call is for — and
`release_link`, `datetime`, `settings_update_note` and `description` only when they are passed, so fixing
a mistyped version does not wipe a note somebody wrote. An empty string is how a note, or the link, is
cleared on purpose.

**Its dates are statements, not stamps — and that is the one place a caller hands this service a
time.** A release is written down after the fact, so `date` and each service's `datetime` are what
somebody *said*: `2026-10-07`, or `2026-10-07T14:30:00+03:00`. They are read by a parser of this
service's own (`scripts/caller_moment.rs`) rather than by the library one that reads the transfer files,
because that one reads the clock at fixed offsets and **ignores a zone offset entirely** — it would file
`14:30+03:00` as 14:30 UTC, three hours off and without a word. A bare date is midnight UTC, no zone
means UTC, and a unix timestamp is refused: a bare number does not say its unit. On the way out the two
are RFC 3339 strings on the MCP surface — the spelling that goes in — while every stamp the server put
on stays unix seconds; the browser draws them in UTC and says so, since a date that reads as the 6th to
anybody west of Greenwich is a different date for the same release depending on who is looking.

**The list is ordered by `date`, newest first, and has no archive window.** Not by when the record was
written — a release recorded late, with an earlier date, belongs below the ones that went out after it.
And nothing ages off: a board is the last few days of work, a list of releases *is* the history. That is
why `releases_list` is capped by a `limit` where `tasks_list` is not, and why the board snapshot carries
every release of the project while it carries only the live goals.

**Deleting is a flag, and the goals are not edited.** `releases_delete` is for a release recorded by
mistake; it leaves every list and every goal that listed it, and stays reachable by its id. The goals keep
the number in their lists and read past it, so `deleted: false` puts the release back on them as well —
an undo that had to remember which goals to re-attach to would not be an undo. A release that was rolled
back is **not** deleted: it happened, so it is taken off the environment it was pulled back from, and
what became of it goes on its thread.

**Recording a release with a `goal` writes two rows**, the release and then the goal that now lists it,
with no transaction around them. The order is the safe one: a crash between the two leaves a release
nobody has attached yet, which is a legitimate state and visible on the Releases screen — the other order
would leave a goal pointing at a number that names nothing.

In the browser there is a **Releases** tab: one row per release, newest first, folded to its id, title,
the id of the goal it shipped (in that goal's colour), a chip per environment it is out on — production's
filled green, the rest outlined — a settings flag when one is due, up to three `service version` chips, a
count of the notes on its thread, and the date; a click opens the goal's name, the services table, the
notes and the thread. Two filters narrow the list — to one microservice, and to what is or is not on an
environment — and together they put the version of a service that is live at the top. A goal's dialog
shows the releases it went out in under its text, open, and a goal that has shipped carries a `🚀` count
on the Goals screen. All of it read-only — a release is recorded, labelled and commented through `/mcp`.

**The goal a release shipped can be opened from the release.** The chip carries the eye a goal's own row
has, and it opens the same dialog. A release holds its goal by id, name and colour only, so the rest
comes from the board: from the push the screen is already drawn from when the goal is on it, and from the
server — archive included — when it is not, which is the ordinary state of a goal whose release is a
month old. A release is exactly the thing that outlives its goal.

**A release has an address: `/release/{project}/{release}`.** The Releases tab remembers its board rather
than naming it in the url, so there was nothing to paste into a chat that would land somebody on *this
one*. The page at that address is one release drawn with the pieces the list and a goal's dialog use,
read cold by its two halves — the board's prefix, then the release's id (`RMS-R12`) or its bare number —
through `POST /api/releases/v1/get`. Every row on the Releases tab, and every release under a goal, ends
in a `↗` that opens it in a new tab; the address in that tab's bar is the link to share, and the tab is
named after the release. A link to a release that has since been deleted still opens it, and says so:
"no such release" would read like a typo. The page is live like every other screen, and it sets the
board the tabs remember, so going from a release to Releases lands on that release's project.

## Searching the board

Every other read here is a **filter**: a column, a kind, an assignee, a goal. Those answer "what is in this
state", which is what a board is for — and none of them answers the question an agent actually arrives
with, which is *"where did we discuss this"*. The text is the only thing that knows, and most of it lives on
threads that no listing returns: `tasks_list` reports `comments_amount` and not one comment, for exactly
the context reason `documents_list` reports no texts.

So without `tasks_search` the reasoning behind every decision on this board is reachable only by listing it
and then opening threads one call at a time until the right one turns up. It searches a task's text, a
goal's name and description, both checklists and **both comment threads**, and each match says which of
those it came from — because the distinction is what the caller does next with it. The card says what is to
be done; the thread says what was learned, what was tried and why the work is shaped this way.

It returns a **headline and the matching lines**, never whole tasks. A search that cost what a listing
costs would not be reached for before guessing, which is the only time it is worth anything. Results are
sorted **most-matched first** rather than by priority: a card mentioning the subject six times is more
likely the one that was meant than one mentioning it in passing.

Served entirely from memory — comments ride inside the task's own row and are already in the snapshot, so
searching the threads is a walk over what is loaded rather than a query. Archived and deleted work is left
out by default, matching `tasks_list`, so "search finds a task the board does not show" cannot happen by
accident; `include_archived` is worth passing more often here than on a listing, because "how did we solve
this last time" is a question about work that has already landed.

## Documents

A **document** is a text that outlives the work: a specification, a decision written up, a piece of
reference. It belongs to one project, it lives at a path — `docs/design/system.md` — and tasks and goals
**reference** it instead of copying it into their own text. That is the whole point: one place that gets
edited, rather than three copies that drift apart in silence.

**A document is text OR bytes.** A specification in Markdown and an uploaded PDF are both documents at both
paths, and exactly one of two nullable columns holds each: `content` for text, `binary_content` — a `bytea` —
for a file. Not an enum, because a column cannot be one; the invariant is established on the write path and
every read decides which kind it is holding by asking which column is filled. `content_type` sits beside them
as metadata (worked out from the extension when nobody says), and `content_size` is stored rather than computed
— a listing wants a size, and computing one means reading the payload.

**base64 appears in exactly one place: an MCP argument.** JSON cannot carry bytes, so `documents_upload` takes
`binary_base64` and `documents_get` hands one back, decoded and encoded at that boundary and nowhere else. The
browser never sees base64 at all — it points an `<img>` or an `<iframe>` at `/raw/{prefix}/{path}`, which serves
the real bytes with the document's own `Content-Type`, so a PDF opens in the browser's own viewer.

**That route is a PATH mirroring the tree, not a query, and that is what makes a framed html page work.** A page
asks for its own `style.css` with a relative url, which the browser resolves against the address the page came
from: served as `/raw/TM/docs/page.html`, `style.css` beside it resolves to `/raw/TM/docs/style.css` and
arrives. From a query url it would resolve back onto the api route and arrive as nothing. A path of arbitrary
depth is not something the routing macro can express, which is why this one is a middleware.

**Arriving is not enough — the type has to be right, and for three kinds of file the browser is unforgiving.**
A stylesheet whose `Content-Type` is not `text/css` is *not applied*; a script that is not a script type is
*not run*; a font that is not a font type is *not loaded*. Nothing fails: the request is a 200, the bytes are
there, and the page renders bare with two lines in a console nobody has open. That was a real bug, and its
cause was the extension table knowing a dozen extensions and calling everything else `text/markdown` — so a
mirrored `design-system.css` was served as Markdown and silently dropped. The table now covers what a page is
made of (css, js/mjs, json, wasm, xml, the `font/*` family, every image), the source and configuration files a
repository is mostly made of as `text/plain`, and the names that carry their type without an extension at all
— `Makefile`, `LICENSE`, `.gitignore`. It is a superset of `my_http_server`'s
`WebContentType::detect_by_extension`, deliberately: the static middleware and this route serve the same kinds
of file, and a browser happy with one and not the other would be a difference nobody could explain.

**What is left over falls back to `application/octet-stream`, not to text.** Claiming a type nobody checked is
how the failure above got to be silent; bytes make the browser offer a download, which somebody can see. The
same honesty is why a stored `text/markdown` on a path that plainly says otherwise gives way to the path: that
value is not a declaration, it is what every unknown extension became before the table grew, and it is sitting
in rows those builds wrote.

**A mirror answers the two questions differently, on purpose.** What a connected repository's file is *offered
as* is that honest type; whether it is *read as text* is decided from the table alone, so an extension nobody
has a type for is still read as text first — a file with an unfamiliar suffix in a repository is somebody's own
convention far more often than it is a binary, and the read checks the bytes before calling anything text.

Text types go out with `charset=utf-8` appended, on the wire only. Without it the browser's default for
`text/html` and `text/css` is the locale's encoding rather than UTF-8, which turns every Cyrillic character in
a mirrored page into mojibake — a document that reads as broken rather than as a missing header. It is not
stored: the charset is how the bytes travel, not what the document is, and putting it in the column would
leave every screen comparing `text/markdown` against something that no longer equals it.

**The path is the key; the id is the identity.** `documents_upload` takes a project, a path and a payload, and
looks the path up: found, and it writes a *new version* of the document that lives there, keeping its id and
its whole history; not found, and it creates one. So "upload the file again" is the entire editing story, and
a caller holding a file and a path needs to know nothing else. The id, by contrast, is a `SortableId` minted
once and never changed — not by a rewrite, not by a move — which is what makes the history complete and what
makes a reference survive somebody tidying the tree.

Which is why **moving is a separate call**, `documents_update_path`. If an upload could carry a new path
*and* new text, the history could not tell "somebody rewrote it" from "somebody moved it", and those are the
two questions a history exists to answer.

**Folders are not real.** They are read off the paths of the documents in them, on both sides — the server
sorts the index by path, the browser walks the segments into a tree. So an empty folder cannot exist, and
renaming one means moving every document under it, one call each. A folder record would have been a second
source of truth about the structure, and it would have drifted from the paths.

### Every document says what is in it — the brief

A listing is paths. `docs/design/system.md` tells a reader almost nothing, so finding "the document about the
settlement retries" meant opening four documents to rule out three — which on a specification of seventy
kilobytes is the whole of somebody's context spent on the search rather than on the work.

So every document carries a **brief**: a few sentences written by whoever read it, saying what it is, what it
covers, and which systems, decisions and names are in it. It rides on every `documents_list` row and every
`documents_get`, and on the browser it is the tooltip of a row in the tree and a line above the viewer. An
empty brief means nobody has read that document for you yet — which is a different thing from an empty
document, and the browser says so by drawing no line at all.

**It is filed by the hash of the content, not against the document, and that one decision is the whole
design.** The key is the sha256 of the bytes, so:

- the same specification stored as a project's document and as a file in a connected repository is briefed
  **once** and found through either — and syncing a copy into a third board finds it there too;
- an edit produces different bytes, therefore a different key, therefore a document nobody has briefed
  **again** — a brief can never describe a text that has since changed, because there is nowhere for it to
  outlive one;
- a document restored from the trash finds its brief waiting, because the bytes came back with it;
- and the table is global rather than per project, since a text does not belong to a board.

**A file is never waiting for a brief.** There is no text in a PNG this product could write one from, so
binaries are not counted as unread — a to-do list that cannot be finished is not one.

The loop an agent runs is three tools. `documents_next_without_brief` hands over the next unread document of
a board — the project's own and its repositories' files together, in path order — with its text and the hash
to file under. `documents_set_brief` files what was learned. `github_refresh` re-clones a connected
repository, waits for it, and answers with how many of the files that arrived nobody has read, which is where
the loop usually starts. `documents_list` reports the same number for a whole board as `without_brief`, and
it counts **contents**: a licence file in nine folders is one brief away from done, and a count of nine would
send an agent round a loop that finishes in one.

**A document too long to hand over whole is briefed from its outline, not from its opening.** The loop
returns the first 64 KB plus every heading with the lines its section spans, and the line to carry on from
— so the brief describes the document rather than whatever it happens to start with, and the sections that
decide what it IS are two `documents_get` calls away. The one document that cannot be read on from is the
one whose first LINE is longer than the budget: a minified bundle, a one-line JSON. `slice_text` reports
that honestly as a `to_line` before `from_line` — "read on from `to_line` + 1" would ask for the same
bytes for ever — so the loop says there is no next line rather than handing back an instruction that
loops.

**The hash is stored rather than computed on demand**, on the document row and on the mirror listing, because
the question "which of these has nobody read?" is asked of a whole project at once — and answering it by
reading every payload is exactly what a brief exists to avoid. Documents written before the column existed
are hashed by a startup pass that writes one column and no history. Files of a repository are hashed by the
listing walk, which reuses the hash it has whenever a file's size and mtime are the pair it hashed last time
— so a ten-minute tick over a repository nobody pushed to opens nothing, and the pass after a re-clone reads
it whole exactly once.

Briefs travel with a project: `briefs.yaml` in the export carries the ones belonging to the documents in the
archive, and the import files them before the documents land, so no document is ever on the receiving board
without the brief it came with. Keyed by content, they need no remapping — ids are renumbered on import and
hashes are not. **A text this instance has already briefed keeps the brief it has**: an import only adds,
and since a brief is filed under a hash rather than under a project, copying a board beside its original
finds every one of them already there — writing them again would only restamp each as rephrased today. One
that cannot be filed — a hash that is not a hash, a text over the cap — comes back in the import's `skipped`
with the reason, and so does a `briefs.yaml` that will not parse; neither stops the import.

### A large document is worked on in pieces

Everything above is complete **at one size** — the size where reproducing a document verbatim to change a
line is affordable, and where reading one to find out whether it mentions something is affordable. Past
that, the surface quietly becomes read-only in practice: `platform-architecture.md` at 76 KB does not fit
in a tool result at all, so it could be read and never written back, and finding which of twenty-three
documents mentions `min_statistics` meant pulling each of them in whole. Two of them, 24 KB and 33 KB,
turned out to be clean — about fifteen thousand tokens spent learning that nothing was there.

Four tools exist for nothing but that, and they compose in one direction:

**`documents_search`** asks the whole project at once and never returns a text — only matching lines and a
couple of lines either side. Line-oriented like grep, so a match carries a `line_number` that goes straight
into a read. The texts come from Postgres (the index deliberately holds no payloads) through a select list
that **does not name the blob column at all**, so a project holding forty PDFs costs the same to search as
one holding none. `matches_total` keeps counting past every cap, which is what makes "nothing matched" a
usable answer rather than a possibly-truncated one.

**`documents_outline`** turns a 76 KB document into about a kilobyte of headings, each with the line range
of the section it opens. Headings inside fenced code blocks are skipped — a specification full of shell
examples has a `# comment` on nearly every page, and an outline naming those would be longer than the real
structure and would point at sections that do not exist. A section **contains its subsections**
(`end_line` runs to the next heading at the same level or shallower), so slicing one entry gives the whole
section rather than its first paragraph.

**`documents_get` takes `from_line` / `to_line` / `max_bytes`** — the outline's numbers verbatim. Bounds
are 1-based and inclusive, `max_bytes` cuts at a line boundary wherever one fits, and `truncated` says the
content is not the whole document. A file is *refused* rather than sliced: lines are a property of text,
and half a PDF is not a smaller PDF — cutting one would hand back base64 that decodes to a corrupt file,
which is worse than a refusal because it looks like it worked.

**`documents_edit`** sends the two hundred bytes that change instead of the seventy-six kilobytes that do
not. Three rules carry the weight, and each is a refusal:

- **an ambiguous match is refused.** If `old_string` appears more than once and `replace_all` was not
  asked for, the call fails and names the count. Silently taking the first occurrence, or silently taking
  all of them, is exactly how a 76 KB architecture document ends up subtly wrong in a place nobody reads
  again;
- **all of the edits or none of them.** The new text is built in memory and only then written, so a
  failure at edit five leaves nothing behind from edits one to four — a half-applied batch is a document
  in a state nobody asked for, and the caller cannot tell how far it got without reading the whole thing
  back. Edits apply **in order, each against the result of the last**, so a later edit may match text an
  earlier one wrote, and one whose match an earlier edit destroyed is an error rather than a skip;
- **`expected_version` is an optimistic lock** — the only concurrency control in this feature, and it
  earns its place because a person can upload over a document from the browser at any moment. A caller
  that read version 4 and now edits had better still be editing version 4: the edits would very likely
  still *apply*, and would land a document mixing two intentions.

One new version per **call**, not per edit: seven edits are one history entry, because seven entries would
describe seven documents nobody ever intended. The event is `updated`, indistinguishable from an upload of
the same text — how a version was produced is a property of the call, not of the document.

**`documents_diff`** answers "did that write do what I meant" without either text. `documents_history` says
a version exists, who wrote it and when; it cannot say what is *in* it. It is also what tells you what
somebody else changed after an edit was refused on its version — which is why the refusal message names the
diff call to make. `to_version` defaults to the newest version taken from the *history* rather than from
the document row, so it answers for a trashed document exactly as it does for a live one.

The whole loop, then: search to find which document, outline to find the section, get with a line range to
read it, edit to change it, diff to check the change is the one that was meant. None of the five steps
carries a whole document.

**Nothing is lost.** Every version is kept whole in `documents_history` — path, text, who, when, and *what
happened* (`created`, `updated`, `moved`, `deleted`, `restored`). Whole texts rather than diffs: documents are
read one at a time, so nothing has to walk a chain, and a chain of diffs is a structure whose failure mode is
"the oldest version is unreadable" — the one thing this table exists to prevent. It answers for a deleted
document too, because the id outliving the deletion is the point.

**History is a second table, and the only place in this service where one change writes two rows.**
Everywhere else state rides on its own row precisely to avoid that. There is therefore no transaction to hide
behind, so the order is fixed: **history first, then the working table.** The failure that leaves behind is a
version nothing is serving — one row too many, which the next attempt overwrites, since history is *upserted*
rather than inserted. The other order loses a version outright, and a history with a hole in it is not a
history.

**Deleting is a move to a second table, not a flag.** `documents_delete` takes the row out of `documents` and
puts it in `documents_trash`; `documents_restore` brings it back, by default exactly where it was, and is
*refused* when something has taken that path since — where a restored document lands is a decision, and a
guessed path is how a document ends up somewhere nobody looks. A second table rather than a `deleted` column
because with a flag every read of the working set — the index, the path lookup, the uniqueness check — would
have to remember to exclude the deleted, and the one that forgot would be a bug nobody notices until a path
that looks free refuses a document.

The trash is **flat** (it keeps only the last path each document had) and **invisible in the browser**:
`documents_trash` is the only way to see it. It is not a place to browse — it is a list you ask for when
something needs restoring.

**Deleting a folder is `documents_delete_folder`, and it is the single deletion repeated.** There is no
folder to delete — folders are read off the paths of the documents in them, so a folder stops existing
exactly when the last document under it does, and emptying one by hand is one call per document and a
folder that survives because the caller got bored three files early. The call takes the **subtree**, which
is what deleting a folder means anywhere else, and it does *not* take a document that merely shares the
folder's name: the rule is the `docs/` prefix, slash included. Every document goes through the same path a
single deletion does — its own history entry, its own trash row, its own id — so each is restorable on its
own, and the answer lists what went, because the trash is flat and a folder of forty is forty rows in it.

There is deliberately **no way to say "all of them"**: an empty folder is refused, since that is not a
folder, it is the project's documents. And there is no transaction across the documents — each is two
tables of its own — so a failure part way through is *reported as itself*, with how many went and which one
stopped it, rather than rolled back. What went is in the trash and re-running finishes the job.

### A reference is a url, and it names either kind of document

**A reference is a list of urls in a `jsonb` column** on the task or goal row — `add_documents` /
`remove_documents`, the same shape as labels and for the same reason: a caller attaching one document must
not have to resend the four already there. Two forms, one per kind of document:

```
raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8   a document of the project's own, by id
raw/TM/github/specs/design/system.md       a file in a connected repository, by path
```

**A url rather than a bare id, because half of what a board points at has no id.** A file in a connected
repository is a working copy on disk, not a row: it has no `SortableId`, no version and no trash, and there
was no way to attach one — which meant a specification that already existed in a repository had to be copied
into the project before the work could point at it, and the copy started drifting the same afternoon. The
path is what those files *are* named by everywhere else in this product, so the reference names them by it.

Three more things fall out of the shape, and each of them was a reason on its own:

- **it says which board it is on**, so following one lands on that board. The Documents screen used to open
  on whichever project the browser remembered, which meant a reference followed from a task on another one
  drew a tree the document was not in and reported it missing — with nothing on screen to say why;
- **it survives being written down.** Paste one into a `CLAUDE.md`, an issue or a message and hand it back to
  `documents_get`, which takes a reference wherever it takes an id, as do `documents_edit`,
  `documents_update_path`, `documents_delete`, `documents_outline` and `documents_diff`. That is the point of
  a vocabulary an agent can be *told* rather than has to look up;
- **it is the address of the bytes**, give or take the leading slash. `/raw/TM/github/specs/design/system.md`
  is the route that has always served a mirrored file; `/raw/TM/document/<id>` is the same route reading by
  id instead of by path, which is what makes a reference survive the document being moved. The one ambiguity
  it introduces is narrow and settled in favour of what was there first: `document/` plus exactly one segment
  is an id, and `document/spec/a.md` is a document of the project's own in a folder called `document`.

**The four spellings of one document all resolve to the same reference**, because a caller holds whichever
one the call it last made handed back: the reference itself, the `id` a listing reports, the `path` a listing
reports, and — for a document of the project's own — the bare id, which is what every reference stored before
this vocabulary existed still is. What is *stored* is always the url, and `documents_list` reports it as
`reference` beside the id. Detaching compares canonical spellings, so a reference comes off with whichever
name you have for it.

Two departures from how labels behave:

- a reference naming **nothing** is refused. For a document of the project's own that means no such id, and a
  *trashed* one is refused with the path it had; for a file in a repository it means the mirror does not hold
  that path — and "the repository has not been read yet" is said as itself, because that is a different
  problem with a different fix. A label is a word; a reference is supposed to point at something a reader can
  open, and a dead link is worse than a refusal;
- a reference is **never cleaned up** when a document is deleted. Restoring is one call away, and a reference
  quietly dropped would not come back with the document. A reader that cannot resolve one is told it is in
  the trash.

Validating an added reference is the one piece of validation in `scripts/` that reads Postgres — documents
are not in memory. A file in a repository is cheaper: it is checked against the same listing `documents_list`
answered with, so what can be attached is exactly what the caller was offered, without a directory walk or a
request to GitHub. Either way it happens before a task number is reserved, like everything else, so a refused
reference does not burn an id.

In the browser: a **Documents** screen with the tree on the left and the selected document on the right,
modelled on the file browser in `remote-development-mcp` down to the class names — it is the same problem, and
a second design for it would be a second thing to maintain. The selection lives in the URL
(`/documents?selected=<reference>`), so a document is linkable, survives a reload and works with the back
button — and because a reference names its board, such a link opens the right one rather than whichever was
last browsed;
which folders are open is remembered per project in local storage, and every folder down to a linked document
is opened so a link lands ON it. How a document is drawn is decided by its **content type**, not by which column it came out of — html is text,
and a viewer that framed only binary payloads showed a web page as a wall of markup. Markdown is rendered with
a source toggle, an image is drawn in place, html and a PDF are framed with a way out to a full tab, any other
text is shown as it was written, and anything else is offered as a download.

**HTML is sandboxed and a PDF is not**, and the difference is deliberate. HTML served from our own origin
executes in it, and the session token lives in that origin's local storage — so a document an agent uploaded
could read every viewer's token. The frame carries `sandbox="allow-scripts"` and the raw response carries
`Content-Security-Policy: sandbox allow-scripts`: scripts and stylesheets still run, so the page renders as the
page it is, but it sits in an opaque origin. `allow-scripts` WITHOUT `allow-same-origin` is the combination
that matters — granting both lets the framed document drop the sandbox itself. The header as well as the
attribute, because "open full screen" loads that url directly in a tab, where the attribute does not exist. A
PDF gets none of it: its scripts run inside the browser's PDF viewer rather than in our page, and a sandbox
there breaks the viewer for no gain.

The file browser this viewer copies deliberately does NOT sandbox, and its comment names the condition for
changing that — a different exposure. This is one: a board on the public internet, behind a sign-in, showing
documents somebody else uploaded.

The raw bytes are served at **`/raw/{prefix}/{path}`** — the project in a path segment and the document's path
mirroring the tree, which is what makes a framed html page work: a page asks for its own `style.css` with a
relative url, and the browser resolves it against the address the page came from, so from
`/raw/TM/docs/page.html` it resolves to `/raw/TM/docs/style.css` and arrives. From a query url
(`?project=TM&id=…`) it would resolve back onto the api route and arrive as nothing. A path of arbitrary depth
is not something the routing macro can express, which is why this one route is a middleware rather than an
action — and it is the same reason the project is a segment here while every other endpoint takes it as a
parameter.

A reference on a task or a goal is a row labelled by what the reference itself carries, since the snapshot
carries references and not documents: a file in a connected repository is drawn by its **file name** with its
path as the tooltip, because the reference spells both out, and a document of the project's own is still
drawn by its id — its path is a row in a table this screen has not fetched. The two are marked differently on
purpose. A repository's file is somebody else's — no history here, and nothing on this board can change it —
and a reader who cannot tell them apart on a card learns the difference at the worst possible moment.

**Clicking one opens the document IN that dialog, at the same 95% of the window the task was read at, with
a back arrow in the header.** One size for both is the point rather than a saving: a document that opened
narrower than the task it came from would make the window jump on every click and jump back on every press of
the arrow. The reader is in the
middle of a task and the document is the specification it is done against — sending them to another screen to
read it means coming back to find the task, which is the trip nobody makes twice. The arrow restores the card
exactly as it was: the state is carried rather than re-fetched, because re-opening by id would land on
whatever that card looks like now rather than on what somebody was halfway through reading. The arrow is not a
second close — the cross still closes the lot, which is why the two sit at opposite ends of the header.

The document is drawn by the same `render_found` the Documents screen uses; a second rendering would be a
second set of decisions about framing, sandboxing and content types to keep in step. What a modal cannot give
is the full pane a PDF's viewer wants, so **Open in Documents** stays as a footnote under the document — that
one does leave, and because the reference names its board it lands on the right one.

Nothing on that screen is live, deliberately: the socket carries the board, and documents must not ride along,
so Refresh is the answer to "an agent just uploaded something".

**Uploading is the second exception in the product to "MCP writes, the UI reads"** — the first being a goal's
colour. It earns it: the alternative is not a person using MCP, it is a person unable to upload at all, because
a PDF on a laptop cannot reach an agent without being base64-ed by hand into a tool call. Everything else about
a document — moving, deleting, restoring — is still MCP only, and the browser's upload is a second DOOR into
the same write path rather than a second path: uploading onto a taken path writes a new version of what is
there, keeping its id and its history, exactly as the tool does. The author comes from the session rather than
from an argument, which is the one thing this side does better than MCP.

The form has no "create folder", because there is nothing to create: a folder exists for as long as a document
is in it. Choosing an existing folder and making a new one are one act — typing a path — and the dropdown
beside the box is a shortcut that fills it in.

**The `(project_id, doc_path)` index is unique**, as the backstop under the path-is-the-key rule: the
application checks before it writes, and the index is what stops two writes racing past that check.


### A connected repository is a clone you can read

Reference material usually already exists, and it usually already lives in a GitHub repository — a
specification, an API contract, somebody else's README that four tasks refer to. Copying it in by hand means
copying it in again every time it changes, which nobody does, which is how a project ends up with a
specification that is quietly a year old.

So a project can **connect** a repository. It takes a name, a url, optionally a branch and a folder inside the
repository, and from then on that repository appears in the project's documents as `github/<name>/…`,
fetched every ten minutes. It is set up by a person in the browser, under **Projects setup → GitHub**, and
only there: a connection is configuration, like a project's columns and its members, so nothing on the MCP
surface creates one. What the MCP surface then gets is everything *inside* it.

**The path has three parts and the documents tree draws two**, on purpose. `github/<name>/…` is what every
tool reads, writes and refuses by — it is the contract, and nothing in the browser may change it. But the
tree shows the repository as ONE top-level row, `analytics` with a `github@<owner>` tag and GitHub's mark
beside it, sitting among the project's own folders rather than inside a `github` one. That level held a
single word and cost a click to get past, and what it used to say — whose account this is — now sits on the
repository's own row, where it names one owner instead of hedging across all of them. The consequence worth
knowing: refreshing is per repository now, on each row, because there is no row above them to mean "all".

**What is behind those paths is a real `git clone` on a mounted disk.** Not a listing, not a cache of blobs —
a working copy, the same thing you would have in a terminal. That single fact is what the rest of this
section follows from: reading a file is reading a file, everything git can say about the repository is one
command away, and the whole folder can be replaced by a fresh one — which is what **Refresh** does.

**It is served by the tools that serve the project's own documents, and that is the whole point of the
design.** `documents_list` answers with the repository's files beside the real ones, sorted into the same path
order; `documents_get` reads one by reference, by id or by path; `documents_outline` works on one. An agent
needs to learn nothing new — it asks the same question and gets the reference material with the rest. Such a
file is referenced from a task or a goal exactly as one of the project's own is: `raw/{project}/github/…`,
which is the half of [the reference vocabulary](#a-reference-is-a-url-and-it-names-either-kind-of-document)
that exists because these files have no id to be named by.

**And nothing writes them.** `documents_upload`, `documents_edit`, `documents_delete`,
`documents_delete_folder` and `documents_update_path` all refuse a `github/` path, with one sentence that
says why and what to do instead. A connected repository is somebody else's, and this board reads it.

**The reason is what a refresh does, and the two decisions are one decision.** Pressing Refresh on a
connection clones the repository again and REPLACES the folder with the new copy. That is what makes a
connection honest — what is on the screen is what GitHub has, rather than whatever accumulated on a disk —
and it is exactly why nothing may be written there: a file written into that folder would disappear at the
next refresh, with nothing left to say it had ever been there. A product that offered a write it would
silently destroy would be worse than one that offers no write at all.

**So there are two ways to change one of these files, and both are honest.** Change it in the repository —
through GitHub, through a checkout of your own, through a commit and a push — and refresh the connection to
see it. Or take a copy this board owns: **sync** it into the project's own documents, where it gets an id, a
version, an author and a history, and where every writing tool works on it. The second is what most reference
material actually wants; the first is what a repository is for.

#### `github_git` is git, not a menu

One tool, three arguments — project, connection, and a command written the way you would type it. It runs
**the whole of git**: status, diff, log, add, commit, push, pull, fetch, branch, checkout, stash, merge,
rebase, reset, show, blame, `-c`, aliases. There is deliberately no allow-list of subcommands, because a git
with subcommands taken out of it is a git that will not do the one thing somebody needs at the moment they
need it — and because this runs inside the container the clones live in, so the blast radius of git is the
container.

**It is not how a file gets edited, because nothing here edits one.** What it is for is asking a repository
questions — `git log`, `git diff`, `git show`, `git blame`, `git status` — and, when somebody has been asked
to change the repository, running the change git itself can produce. Whatever it leaves in the working copy
lives only until the next refresh, which deletes the folder: push what is meant to last.

**The one rule is that it has to be git**, and it is checked against the first ARGUMENT rather than against
the text. A prefix check on the string would pass `gitleaks …` and `github-cli …`, which are different
programs.

**The command never reaches a shell**, and that is what makes the rule mean anything rather than being
decoration. Through a shell, `git status; rm -rf /` starts with `git` and passes any prefix check ever
written, because a shell reads it as two commands. The string is split into arguments here — quoting
honoured, so a commit message with spaces survives — and `git` is executed directly with them. There is then
no second command to read: `;`, `&&`, `|`, `$(…)` and backticks are ordinary characters inside an argument,
which git rejects for itself. That costs nothing in capability, because none of those were ever git.

The command runs **at the root of the clone**, which is not always where the documents paths start: a
connection rooted at `docs` shows `github/<name>/design/a.md` for what git calls `docs/design/a.md`, so
`git add design/a.md` fails with "did not match any files". `git status` settles it — it covers the whole
repository and prints paths in the spelling `git add` accepts.

Two things are supplied on every invocation, and **only one of them can be overridden the way you would
expect**. The committer identity can: `user.name` and `user.email` are single-valued, so a later
`-c user.name=…` wins, as does `--author "Name <email>"` on the commit — which is also why `git commit` never
fails with "please tell me who you are". The key cannot. It goes on as
`http.https://github.com/.extraHeader`, and git treats that key as a **list**: a second `-c` for it *adds* a
second `Authorization` header rather than replacing the first, and both go to github.com. Pushing under a
different token means resetting the list first and then supplying one —
`-c "http.https://github.com/.extraHeader="` followed by
`-c "http.https://github.com/.extraHeader=Authorization: Basic <base64 of x-access-token:TOKEN>"`. The empty
value is what git documents for exactly this; the order matters, because the reset only clears what came
before it.

**Conflicts are resolved with git** — `git pull` stopping on one leaves markers in the working tree, which
`documents_get` shows and nothing here can edit out: `git checkout --ours`/`--theirs` on the paths, or
`git merge --abort` to undo the attempt. Or the shortest answer of all, now that the folder is disposable:
press Refresh and take the branch as GitHub has it.

A non-zero exit is reported as a result rather than as a failure, because most of them are answers: a push
refused for non-fast-forward, a merge stopped on a conflict, a commit with nothing staged. Output is cut at a
cap with a line saying so — a `git log` over a long history is megabytes and the caller is a model with a
context window.

#### What the clone changes about everything else

**Reads cost nothing now.** The previous design held blob references and fetched a file from GitHub on every
read, which made a public repository browsable and a sync of two hundred files impossible: anonymous reads
share sixty an hour. A clone pays that budget once. `documents_get`, the viewer, a sync of the whole
repository — all file reads.

**A key that is gone is no longer a repository that is gone.** The token is still held in the process's
memory and written to no table, no settings file and no log, so it still has to be typed in again after a
restart — but the clone survives on the disk. A connection reading `needs-key` on Monday having worked all
Friday still lists every file it has and reads them; only reaching GitHub waits. That was the trade's one
real cost, and the disk paid it off. Pressing **Refresh** on such a connection is safe, too, and not by
luck — see the staging below: the clone that fails for want of a key happens BESIDE the folder rather than
in place of it, so what the failure produces is a sentence on the connection and not an empty tree.

**The timer fetches and the button re-clones, and those are different operations for different questions.**
A tick is a `git fetch` over eight repositories that have mostly not moved — it has to be cheap, and it
touches nothing in the working tree. The fast-forward after it happens only when `git status --porcelain` is
empty and it is `--ff-only`, so a working copy that a git command left something in stops rather than opening
a merge nobody asked for. A refresh that declines to merge is not a failure and is not reported as one.
Pressing **Refresh** asks the other question — *give me what GitHub has now* — and answers it the only way
that always works: the repository is cloned again, and the new copy replaces the old one.

**And it is staged rather than destructive, which is the whole of how it stays safe to press.** The clone
goes into `<name>~new` beside the connection's folder and takes as long as a repository takes, with nothing
locked and nothing deleted — the connection carries on being listed and read from the copy it already has.
Only when the new clone is complete does the swap happen, and it is two renames on one filesystem —
`<name>` to `<name>~old`, then `<name>~new` to `<name>` — under the write side of a `tokio::sync::RwLock`
held per connection. Everything that touches a working copy takes the read side for as long as it is
looking: a document read, the listing walk, a `github_git` command, a sync. So the exclusion lasts
microseconds and covers the one instant a reader could otherwise see a folder that is neither copy; a
five-minute `git rebase` delays a swap rather than having the ground taken out from under it; and the old
folder is deleted afterwards, outside the lock, because nothing is waiting on it.

**The timer's fast-forward takes that same write side**, and for a reason worth stating because it is the
one the lock was nearly written without. A swap is not the only thing that changes a working copy under a
reader: `git merge --ff-only` rewrites files too, and a read guard excludes nothing against a task holding
no guard at all. Before the fetch took the lock, a ten-minute tick could fast-forward the tree half way
through a sync of two hundred files, and what landed was a folder of documents assembled from two
different commits with nothing in the result saying so. The `git fetch` itself stays outside the lock —
it writes into `.git` and touches no file anybody is reading — so what is held is the cheap half: the
`git status` check, the branch comparison and the merge, as one step.

**And the two asks are told apart.** Pulling one connection is claimed through a per-connection lock with
two doors: the timer takes it with `try_` and skips a connection somebody is already refreshing, while a
person's Refresh WAITS for the fetch in flight and then re-clones. That asymmetry is not a nicety — when
the manual ask was simply dropped, the fetch it collided with finished, bumped the counter the dialog was
watching, and the screen reported a re-clone that never happened. The receipt is now read after the claim
is taken, so it can only be satisfied by the run it was issued for; a claim that does not come free within
twenty seconds answers with what is happening instead of holding the request open.

Three consequences worth stating. A refresh that FAILS — no key, no network, a repository that is not there
— changes nothing at all: the old folder is still the connection's, still listed, still readable, with the
reason beside it. `~` cannot appear in a connection's name, so neither staging folder can collide with a
working copy, and the one `remove_dir_all` in that module is guarded on the marker rather than on where the
path was built. And a crash between the two renames leaves the files in `<name>~old`: the next refresh
clears both staging folders before it starts, and the pull after that clones from nothing.

**And the fetch only fast-forwards the branch it is FOR**, which is a check git will not do for you: `git merge
--ff-only origin/dev` in a working copy sitting on `main` moves *main* onto dev's tip. It is a merge, and a
merge does not care that the two names differ. Two ordinary things reach that state — somebody edits a
connection's branch after it was cloned, or somebody takes the advice above and works on a branch of their
own — and neither would report anything, because the merge succeeds. So the checked-out branch is compared
with the connection's first, and a mismatch declines exactly as a dirty tree does. A detached HEAD declines
too.

**Editing a connection changes the row; Refresh is what applies it.** The url and the branch are read when
the clone is made and not by the timer: `origin` keeps the address it was cloned from and the working copy
stays on the branch it was checked out on, because a fetch does no `git remote set-url` and no checkout. So
re-pointing a connection at a different repository, or at a different branch, puts the new values on the
connections screen and moves nothing until somebody presses **Refresh** — which clones what the row now says,
`--branch` included, and swaps that in. That is the whole of re-pointing, and it is why the elaborate
`git remote set-url` / `git reset --hard` recipe this section used to carry is gone. The folder *inside* the
repository is still the one field that takes effect at once, because it is applied when the working copy is
listed rather than when it is cloned.

**Detaching does not reclaim the folder.** Removing a connection deletes the row, the listing and the key,
and leaves the clone exactly where it was: a detach is a change to the board, and deleting a folder on a disk
in the same breath is a second thing nobody asked for. Reclaiming the space is a deliberate act on the host,
under `<git_repos_path>/<project id>/<connection name>`. The consequence is that a connection re-created under
the same name adopts the folder that is already there, `origin` included — which used to be permanent and now
is not: one press of **Refresh** clones what the new row names and replaces that folder with it.

**A connected folder does not make a smaller clone.** The folder inside the repository roots what is *shown*;
what is cloned is the whole repository at its full history, because that is what `git clone` is. Two things
follow. The disk holds far more than the listing suggests — the five-thousand-file cap bounds
`documents_list`, not the checkout. And every git command reaches the whole repository: `git log` covers
files no connection shows, and a `git add -A` after editing one file stages anything else lying around in the
working tree.

**The listing is walked off the working copy**, with `git ls-files --cached --others --exclude-standard`, and
each of the three flags earns its place: `--cached` is what git tracks, `--others` adds what is in the tree
and not tracked — a file a `git` command left behind, which is visible on the screen rather than invisible
because nobody committed it — and `--exclude-standard` applies `.gitignore`, so a `target/` somebody built
inside the clone does not become forty thousand rows.

**That listing runs git UNCUT, and it is worth knowing why the distinction exists.** Every other git command
here answers a model, so its output is cut at 60 000 characters with a line saying so — which is right for a
`git log` and quietly wrong for a listing: `ls-files -z` passes 60 000 bytes at around 1 200 paths, and a cut
one does not fail. It loses every file after the cut, counts none of them as skipped, and glues the sentence
about being cut onto the last surviving path. Every number taken off that listing — how many files a
connection holds, how many of them nobody has briefed — would have been a number about a prefix of the
repository with nothing saying so. Internal plumbing therefore reads git's bytes whole and splits the NUL
list itself, which also means a path that is not UTF-8 is one file skipped rather than a listing half read.
Symlinks are skipped for a different reason: following one is how a repository would get this service to read
outside the clone.

**Each listed text file is also hashed there**, which is what lets a repository's files carry briefs like
any other document — see [the brief](#every-document-says-what-is-in-it--the-brief). The walk reuses a hash
whenever a file's size and mtime are the pair it hashed last time, so the ten-minute tick over a repository
nobody has pushed to opens nothing at all; a re-clone rewrites every file, so it re-reads the repository
once. It runs on a blocking thread: thousands of reads on a tokio worker, under the connection's read guard,
would have shown up as a refresh that appears to hang.

**The versions are git's, and this side stops pretending otherwise.** `documents_history`,
`documents_diff` and `documents_restore` refuse a file in a connected repository — and each refusal names the
git command that answers the same question: `git log --follow`, `git diff`, `git checkout --`. That is not a
gap being apologised for. The history is complete, it is just kept by the thing that keeps histories.
`expected_version` means nothing here for the same reason and a non-zero one is refused rather than ignored:
every read reports version 0, so a lock that can never fail to match is worse than no lock at all.

Deleting one is refused rather than performed, and it is worth seeing why that is not timidity: removing a
file from this disk would change nothing in the repository and would be undone by the next refresh, so what
looks like a deletion is a copy quietly diverging until a clone puts it back.

**Moving is refused in both directions, and for two different reasons.** Out of a connection into the
project's own documents is a *sync* — it produces a document with an id, a version and a history, and the file
stays in the repository. In is an upload into a folder nothing writes to. And a rename *inside* a connection
is a write like any other. None of the three is what "move" means, and a move that quietly changed what a
thing IS would be the kind of rename nobody could account for later.

**Syncing is still a copy and still exists**, for the same reason it did: what lands is a document with an
id, a version, an author and a full history that stops tracking the repository the moment it is written. The
`override` checkbox still separates "bring in what I have not got" from "make mine match theirs". What
changed is that it no longer runs out of rate limit halfway through two hundred files.

#### What it costs

**The disk is a mounted volume and has to be.** `git_repos_path` in settings says where, defaulting to the
path the compose file mounts, and one folder per connection sits under it keyed by project id then
connection name — two boards may each connect something called `specs`, and they are two working trees.
Leaving that on the container's writable layer would mean re-cloning every connected repository on every
deploy — minutes of downloading before a board that was already there could be read, repeated for each
service restart.

**The image carries git.** The runtime Dockerfile installs `git` and `ca-certificates` on top of
`ubuntu:22.04`; without them the service starts, serves the board, and fails on the first connected
repository with "could not run git".

The configuration is still durable and the key still is not: the repository, branch and folder are a `jsonb`
column on the project row, and the token is memory only, so a database dump carries no credential. The key
reaches GitHub as a scoped `http.https://github.com/.extraHeader` on one command line rather than in the
remote's url — a token written into the url would be a token in `.git/config` on the volume, outliving the
process it was typed into and landing in every backup of that disk.

What the listing deliberately leaves out is counted rather than listed: a file over the single-document size
limit, a path this product will not name, anything past five thousand files. A file too big to ever be read
is left out rather than shown, because a row every read refuses is worse than an absence. One number answers
the only question that matters — is the file I want missing because it was filtered, or because it is not
there?

Two things it is worth knowing it does **not** do. `documents_search` does not reach into connected
repositories — it searches the project's own texts out of Postgres, and `git grep` is the right tool for a
repository, which `github_git` will run. And a key supplied for a connection serves every member of the
project, so whatever that key is allowed to do is shared with them for as long as it is held.

**What it is allowed to do is decided on GitHub rather than here, and the two kinds of token differ.** A
classic token needs the `repo` scope to read a private repository at all, and `repo` is read *and* write —
so on that path a key that can fetch really can push. A fine-grained token is the one that can be either:
**Contents: Read** clones and fetches and has `git push` come back refused, **Contents: Read and write**
pushes, and pushes for everybody on the board. A public repository is cloned and fetched with no key at all,
so a key on one is only ever there in order to push.

## Moving a whole project — export and import

A board can be poured into another board. Two controls on the projects setup row: **Export** downloads the
project as a zip, **Import** takes one back.

**The archive is six YAML files and a folder — seven once a document has been briefed**, and the split is the point — one file per kind of thing, so
each is readable on its own and a diff between two exports says which of them changed:

```
project.yaml     the project's settings, plus what the archive holds
goals.yaml       every goal, each listing the releases it went out in
tasks.yaml       every task
comments.yaml    every comment — on tasks, goals and releases alike — oldest first
releases.yaml    every release, with the services in it
documents.yaml   what each of those files is: its ID, its path, its declared content type
documents/       the project's documents, as themselves, at their own paths
briefs.yaml      what each briefed document says, by content hash — only when there is one to carry
```

`releases.yaml` is a file of its own rather than a list nested in each goal, because that is the shape the
board has: a release is a project-level record and the goal lists it by handle. Nested, a release nobody
had attached would have nowhere to be written, and one listed by two goals would arrive as two. It is
optional in both directions — an archive made before releases existed has none and imports as a board with
none, which is why adding it did not move the format version.

`comments.yaml` being its own file rather than a list nested in each card is what makes an export worth
opening: it is the project's whole conversation in the order it happened.

**Prose travels base64, and nothing else does.** A task's text is Markdown written by a person or an agent —
newlines, colons, leading dashes, `#`, quotes, tabs, every character YAML gives a meaning to. Encoding it
means the file cannot be mis-parsed by a reader with a different idea of block scalars and cannot be silently
re-indented by a hand edit. Ids, statuses, priorities, labels, emails, urls and moments stay legible, because
they are vocabulary rather than prose and none of them can carry a newline. Every such field says so in its
name: `text_base64`, `name_base64`, `title_base64`.

The documents are **not** encoded and not nested — they are files in a `documents/` folder at their real
paths, so the archive is also just a folder of the project's documents, openable by anything.

**Every reference in the file is a handle, and every one is remapped on the way in.** A task is identified by
`(project, number)` out of the receiving project's own counter, so `TM-42` lands as whatever this board hands
out next. That is why `goal: TM-G7` and `depends_on: [TM-4]` are spelled the way a person writes them: a bare
number would be indistinguishable from one this board already uses, and a handle can be checked. A reference
to something the archive does not carry drops the edge and is reported — the work is real, and the grouping
is not worth losing it over.

A release is renumbered the same way and out of the same reservation, since the receiving board serves
tasks, goals and releases from one counter exactly as the source did — and a goal's `releases: [TM-R12]` is
remapped onto the new numbers. Its thread is in `comments.yaml` with every other, named by its handle, and
the environments it is out on cross as the labels they were, in the order it reached them and spelled as
the source board spelled them — an import carries a board across, it does not merge two vocabularies. An
archive exported by 0.2.0 has no labels and may say `released_on_prod` instead; that arrives as the label
`Prod`. A **deleted** release is carried too, still listed by its goals: a goal goes
on listing a release that has been deleted so that bringing it back puts it on the goal again, and that only
holds on the other board if the release is there to bring back.

**A document is the exception: its ID crosses with it, and that is what `documents.yaml` is for.** A card
names a document by a reference — `raw/TM/document/<id>` — so the only part that has to be remapped is the
project prefix, the one part of a reference that is about which board rather than which document. Carrying
the id is safe because a `SortableId` is `{unix_micros}-{uuid}`: unique across instances, not merely within
one, so the same document on two boards is deliberately the same id and two different documents cannot
collide. The `documents/` folder cannot carry it — a folder is keyed by path — which is exactly why there is
a fifth file. Without it the receiving board minted a fresh id for every document and every reference on
every card arrived pointing at nothing, while the documents themselves sat right there.

A reference into a **connected repository** — `raw/TM/github/<repository>/<path>` — is carried across
untouched but for the prefix, and is never dropped. Nothing here could have made it arrive: an export
deliberately does not carry a repository, because the repository is still there and the receiving board gets
those files by connecting it. Dropping the reference would throw away the only record of which file the work
was done against.

Everything that is not a reference arrives unchanged: text, status, priority, kind, assignee, labels,
checklists, build links, and the created / updated / closed / deleted moments. **A comment keeps its own
author and its own moment** — the thread is a record of who said what, and re-signing it with whoever pressed
Import would make it false. A document, by contrast, genuinely *is* being written now, so its version is
signed by the importer.

**Import is additive, except for the settings, which are replaced.** Nothing already on the board is touched
or removed. But the statuses in the file are the source board's column ids and mean nothing unless this
project follows the same template — so the name, description, archive window and the two template ids are
replaced with the file's. Two of those replacements are conditional, and both because the data model forbids
the result rather than the intent: a **template id** is taken only if a template with that id is on this
instance, since pointing a project at one that is not here would empty its board rather than configure it;
and the **prefix** is taken only if it is free, since two projects cannot share one and the common case for
this feature is copying a board on the instance the original still lives on. Either one skipped is reported.
Membership is not in the file at all, and neither is whether the source project was archived: both are facts
about a board on *this* instance, and a file must not be able to hide the project it was poured into.

A status that survives into a project whose template has no such column is **kept as it is** and reads as
Todo, exactly as it would have here — which is what lets pointing the project at the right template
afterwards bring every one of those tasks to where it belongs. Rewriting them on the way in would have made
that impossible, permanently. The count is reported.

**Deleted work is carried; a connected repository is not.** A deleted task is hidden from every screen and
kept so that searching for its id still finds it — leaving it out would make exporting a quiet way of losing
the record, and `tasks.yaml` is hand-editable for anybody who wants it gone. A document under the reserved
`github/` root is a file in a working copy of a repository that is still there; the receiving board gets it
by connecting the same repository, and an import that wrote there would be editing a git checkout.

**The export never holds the archive in memory.** A project's documents are its real payload, so holding a
zip of them to hand to a response would set this service's footprint by the largest board anybody exports.
The archive is built into the temp directory one document at a time as it comes out of Postgres, and the
response streams it back off disk — with `Content-Length` known before the first byte, because the file is
already there, which is what gives the reader a progress bar instead of a spinner. The temp file is removed
when the download finishes, when the reader closes the tab half way through it, and when a read fails.

That is also why Export is a **link** and not a button — the only one on that screen. A download is a
navigation: the browser asks for the url, sees `Content-Disposition: attachment` and saves the file without
leaving the page, and the session rides along because it is a cookie. Doing it through `fetch` would mean
pulling the whole archive into the wasm heap to hand it back to the browser, which is the one thing the
streaming endpoint exists to avoid. Import is the mirror image and goes up as a **raw body** rather than
base64 in JSON, for the same reason: it is the largest thing this API carries.

Neither direction goes near `create_task` or `create_goal`. Those enforce the rules of *doing the work* — a
task starts in Todo, moving one to Done owes a comment — and an import is not somebody doing work: it is the
same board arriving somewhere else, and every one of those rules was already satisfied where it happened. So
the models are built directly and written through the repos, exactly as the startup load does.

Export needs membership of the board; import is admin only, like the rest of the setup screen it sits on.

### The templates move separately

A project export carries the **id** of the templates it follows, not the templates themselves — so pouring
a board onto a fresh instance leaves it pointing at configuration that is not there, which is exactly what
the import reports. **Export templates** / **Import templates** on the Settings page are the other half of
that move, and they are deliberately a separate act: templates are shared by every project on the
instance, so installing them is not something that should happen as a side effect of importing one board.

**One plain YAML file, not an archive.** Templates are a handful of ids, names and colours — nothing to
stream, nothing to put in a folder — and a single document is the artefact somebody actually wants: it
reviews in a diff, edits by hand, and keeps in a repository next to whatever else describes how a team
works. Both kinds are in it, because a board needs both:

```yaml
format: task-manager-templates/1
exported: 2026-08-06T20:00:00.000000Z
column_templates:
- id: default
  name_base64: …
  columns:
  - id: review
    name_base64: …
    order: 2
kind_templates:
- id: work
  name_base64: …
  kinds:
  - id: bug
    name_base64: …
    color: red
    icon: bug
```

Prose is base64 and everything else stays legible, the same rule the project format follows — one decision
made once, in `scripts/transfer_encoding.rs`, rather than twice slightly differently. Both lists are
sorted by id, so two exports of an unchanged instance are byte-identical and a diff shows only what
actually moved.

**A template id is never re-minted**, and that is the difference from every other id in a transfer. A
task's number is re-issued because it belongs to a project's counter; a template id is typed in by a
person, never renamed, and is what a project's `column_template_id` points at — so an id in the file *is*
the identity, and a template arriving without one is refused rather than generated. Generating one would
make the import create a second copy of every template on every run.

**A template already here is REPLACED**, which is the point and also the sharp edge: a template is
followed by projects, and replacing one changes every board that follows it in the same write. Nothing is
deleted by it — a task parked in a column the new version does not have keeps its stored status and reads
as Todo, and putting the column back brings it home — but the change is instant and wide, so the count of
affected projects comes back per template and the dialog puts the button behind a checkbox. Created and
replaced are counted apart for the same reason: creating a template affects nothing.

Validation is the same code the Settings screen goes through — `validate_column_id`,
`validate_task_type_id`, `parse_kind_color`, `is_anchor_column` — so a file cannot install something the UI
would refuse to save. Partial by design, like every import here: a template that fails validation is
reported with the reason and the rest still arrives. Admin only, both directions.

## The session is a cookie

`HttpOnly`, `Secure`, `SameSite`, `Path=/`, and it expires with the token inside it. It replaced a token in
local storage carried in an `Authorization` header, and the move bought three things that header could not:

* **The browser attaches it to requests our code does not make.** An `<img>` or an `<iframe>` pointed at a
  document's bytes, and the WebSocket handshake. All three used to need the token spelled into their url —
  where it lands in browser history, in `Referer`, in proxy logs, and in any link somebody copied and sent on.
  A raw document url is now safe to hand to a colleague: it opens for them only if they are signed in and on
  that board.
* **Script cannot read it.** Which matters here specifically, because this product renders html somebody else
  uploaded. A token in local storage is a token an uploaded page can take.
* **`fetch` sends it with no help from us** — its default is `credentials: "same-origin"` and every api url is
  relative, so the client attaches nothing and therefore cannot leak anything.

The `Authorization` header is still accepted, and the WebSocket still reads a `token=` query. Both are there so
the deploy did not sign everybody out mid-session, and both can go once no live token predates the change.

**Which board is open is NOT a cookie**, and it was one for exactly one release. It is a preference of one
browser's, so it lives in that browser's local storage — **by prefix, not by id** — and the picker is
initialised from it on every load. Two things were wrong with the cookie. The server never read it, because it
had no reason to: every request that acts on a project names the project it acts on, in the body or in the
`/raw/{prefix}/{path}` url, and checks membership of that one. And it rode on every single request regardless,
including the ones fetching a document's bytes.

The bug it left behind is worth recording, because it is the shape of bug this whole boundary invites: what was
stored was the PREFIX, and the Home screen compared it against a project's internal ID. That comparison can only
fail, so every reload fell through to whichever board sorted first and the picker silently forgot what somebody
had chosen. The remembered prefix now goes through the same resolution a task handle does, which also means a
board renamed since it was remembered is still found, and one somebody lost access to falls through instead of
leaving the screen on nothing.

The other preference in local storage is which folders of a document tree are open — it can be dozens of paths,
and neither of the two is anything the server has a use for.

## A project is named by its prefix, and there is no id to get wrong

That bug had a root, and it was two vocabularies for one thing. The fix is that **the internal project id does
not cross the boundary at all** — not in a response, not in a request, not on the socket:

* `ProjectResponse` has **no `id` field**. `prefix` is the identity, and the picker, the api layer and the
  remembered board all hold that one string.
* Every input model names a project as `project` — `RMS` — and the action resolves it through
  `require_project_by_prefix`, which does the lookup and the membership check as one step so neither can be
  done without the other. Below that line everything still speaks in ids: the board, the scripts, Postgres.
* Every response names the board it came from the same way: `TaskResponse.project`, `GoalResponse.project`,
  `DocumentResponse.project`, `BoardSnapshot.project`. A task's own id already carried the prefix in front of
  its number, so the two now agree by construction.
* The socket's `{"watch":"RMS"}` is a prefix too. It is resolved once, at subscribe, and the subscription then
  holds the id — the one place the id is the better half of the trade, because the push comes from the write
  side, which knows the project by id, and a subscription pinned to it survives a prefix rename instead of
  going quiet until the tab is reloaded.
* `/raw/{prefix}/{path}` was already this shape, for a different reason — relative asset references inside a
  framed html document — and it is now the same shape as everything else rather than the exception.

**The cost, said out loud: a prefix is renameable, so this is identity that can change under a client.** A
rename invalidates whatever a browser is holding, and the screen recovers on its next read of the project list;
`prefix_history` is what keeps an old TASK id resolving regardless. That is the same deal every MCP tool has
always had, and it buys something worth more than immutability at this boundary: there is only one name for a
board, so there is nothing left to translate, and nothing left to translate backwards.

One consequence for deploys: the field names on the wire changed, so **the UI and the API go out together.** An
old UI against the new API sends `projectId` where `project` is expected and reads an `id` that is no longer
there.

## Deleting is a flag

A task and a goal each carry a `deleted_moment`: a moment rather than a bool, because "deleted" and "deleted
when" are one fact and two columns for it can disagree — the same reason `close_moment` is shaped that way.

**Nothing is removed, from Postgres or from memory, and that is the whole point.** What is deleted drops out of
every board, list, count and derived answer, and stays exactly where it was so that SEARCHING still finds it
and reports it as gone. A row deleted outright could only answer "no such task", which is indistinguishable
from a typo and from another board's id — and the moment anybody wants a deleted task is precisely the moment
they are searching for its id.

So the split is: `tasks_of_project` keeps deleted work, because it fills the snapshot the browser searches;
`get_task`, `get_goal`, `goals_of_project`, `goal_progress`, `tasks_of_goal`, `tasks_amount`,
`labels_of_project` and `blocks` all forget it. Search has its own doors — `get_task_including_deleted` and
`get_goal_including_deleted` — and the screens hide deleted work unless the search box has something in it, in
which case it is drawn faded and flagged.

Two consequences worth stating, because both fall out rather than being written:

* **A deleted blocker keeps its dependents blocked.** `is_blocked` treats an id naming no task as unsatisfied —
  deliberately, so a typo cannot silently free work — and a deletion lands in the same place. Deleting a
  blocker is not a statement that it was finished.
* **A deleted goal's tasks are not deleted with it.** They read as standalone, because `effective_goal` no
  longer resolves. Whether work outlives its container is a decision somebody makes explicitly, not a side
  effect of removing the container.

**Deleting a goal is not closing it.** Closing says how a goal WENT — which is why it demands a resolution and
refuses while any task is open. Deleting says it should never have existed, and asks nothing, because there is
nothing to record about work that was never real. A goal can be deleted whether it was open or closed.

Both undo cleanly: `deleted: false` on `tasks_update` or `goals_update`. Deleting twice does not move the
moment.

## Archiving a project is a flag too

A project carries an `archived_moment`, shaped for the same reason a task's `deleted_moment` is: "archived"
and "archived when" are one fact. Nothing reads the moment today, and it is still a moment — a bool can never
become one afterwards, whereas a moment is already both.

**Archiving takes a project out of the pickers and out of nothing else.** It is not a deletion with a nicer
name and it is not a freeze: an archived board still resolves by prefix, still serves its tasks, goals,
comments and documents, still follows its templates, still pushes over the WebSocket to anybody watching it,
and still accepts writes. Somebody with the link is not meant to notice.

So the split is: `get_project`, `get_project_by_prefix`, `projects_ever_holding_prefix`,
`resolve_project_by_prefix`, `require_project_by_prefix`, `projects_visible_to` and the template follower
counts all **keep** an archived project. Only the display forgets it — the three project dropdowns on Home,
Goals and Documents, and the unnamed branch of `projects_list`.

Two of those are worth reading closely.

* **The dropdowns hide archived projects except the one currently open.** Not politeness: HTML picks a
  `select`'s shown item from the option carrying `selected`, so an open board with no option at all would
  leave the control displaying the *first* project while the screen below showed a different one. A board
  reached by link stays visible in the control that names it.
* **`projects_list` filters only when nothing was named.** Listing is what an agent is told exists, and a
  board somebody put away is not work to pick up; but `projects_list(project: "RMS")` still returns it, so an
  id from an older conversation keeps resolving. Same rule as a direct link in the browser.

`projects_visible_to` deliberately does not filter, and that is load-bearing: `/api/projects/v1/list` feeds
both a dropdown that must hide archived projects and the setup table that must show them under a toggle. The
server sends everything with `archived` on it and each reader decides. Filtering server-side would need a
second endpoint, or a refetch on every click of that toggle.

**The cost, stated out loud: an archived project keeps holding its prefix.** `is_prefix_free` still refuses
`RMS` to a new project, so archiving does not recycle a prefix. It cannot: if it did, a new project could
take `RMS` over, and every link into the archived board would quietly start landing on somebody else's — the
one failure mode the whole prefix-history arrangement exists to prevent. Freeing a prefix by archiving is a
much larger feature and is not this one.

Archiving is admin-only, lives at `/api/projects/v1/archived/set`, and has no MCP tool — the MCP surface has
no authorization at all, and this is configuration. It undoes cleanly with `archived: false` from the same
button, and archiving twice does not move the moment. Import does not carry it: a project's archived state
belongs to this instance's board, like membership, so pouring a file into a project cannot put it away.

## Who is who

**Authentication is Google OAuth.** `client_id`, `client_secret` and `redirect_uri` come from the
service settings, not from the database — that is what keeps the service able to authenticate on
a cold start.

**A session is carried inside its token**, not stored anywhere: a protobuf model (`email`, `expires`)
encrypted with `session_encryption_key` from the settings, base64'd, and sent back as the bearer token.
The OAuth CSRF `state` is the same shape. So a restart signs nobody out, there is no session table and
no session map, and a second instance would accept tokens issued by the first.

The cost is that a live token cannot be revoked, which makes `logout` a client-side discard. Two things
make that acceptable: the TTL is 12 hours, and every request re-reads the user row — so disabling
somebody locks them out on their very next call, which is what revocation would actually be for. The
same check runs on the WebSocket handshake, so an open tab stops receiving updates too. Rotating the
key is the blunt instrument that invalidates everything at once.

**A user lives in Postgres:** email, name, `disabled`, and an `admin` flag. The board shows the
**name**; a task is assigned by **email**.

**Admin is either of two things:** the `admin` flag on the user row, **or** membership of the
admin list in the service settings. The settings list is additive, not an alternative — it is the
emergency door and the bootstrap: on an empty database nobody is an admin in Postgres, yet those
emails still get in and create everyone else. Same idea as `super_admins` in
`rms/dashboards-rest-api`.

**Access to a project is binary** — you are a member or you are not. Membership is edited from the
project's side, in Projects setup. An admin sees every project without being a member. The
**Users** screen is a roster: add, rename, disable. A disabled user cannot sign in; their
assignments and their comment authorship stay exactly where they are.

A user who is a member of nothing sees an empty Home and a note to ask an admin — not a 403 on
the whole UI.

**Only an admin edits a project** — prefix, columns, kinds, membership.

## MCP surface

Mounted on the same service-sdk HTTP server at `/mcp`, alongside the REST controllers.

**Everything is named by its human handle.** A project is `RMS`, a task is `RMS-42`, and `depends_on`
is `["RMS-7"]`. The internal `SortableId` never crosses this boundary — it is a database and UI
detail. Handles resolve against the state at the moment of the call, which is exactly right for a
tool call and exactly wrong for a handle quoted from last month's chat; that second case is what
`tasks_resolve_id` is for.

Tools:

- `projects_list` — the boards, their columns, kinds and labels. Called first: everything else
  names a project or a task by an id this returns.
- `users_list` — who exists, with email and name. This is how a spoken first name becomes the
  email that goes into `assignee`.
- `tasks_list` / `tasks_create` / `tasks_update` / `tasks_delete`
- `releases_list` / `releases_create` / `releases_update` / `releases_delete` — what went out, and in
  which version of which microservice. `releases_create` takes the `goal` the release ships and attaches
  it in the same call; a release recorded without one is attached later with `add_releases` on
  `goals_update`, which is also where one is detached — the goal lists its releases, so the link is the
  goal's to change. `releases_list` filters by `goal`, by `microservice_id`, and by `env` / `not_on_env`
  for what is and is not out on an environment, newest first; it is capped by `limit` because nothing
  ages off it, and reports the environment labels the project uses. `envs` on `releases_create` and
  `add_envs` / `remove_envs` on `releases_update` say where a release is out — a label on it, not a
  second release — and each service takes a `release_link` to the build that produced it. See
  [Releases](#releases--the-record-of-what-went-out).
- `releases_add_comment` / `releases_get_comments` — the release's thread: how the rollout went, as
  opposed to its notes, which say what changed.
- `tasks_search` — the board and its threads, by what was written on them. The counterpart to
  `tasks_list`: that one answers "what is in this column", this one answers "where did we discuss
  this". **It is the only way to see inside a comment thread without already knowing which card to
  open** — every listing reports `comments_amount` and not one comment, deliberately, so without this
  the reasoning behind every decision on the board is reachable only by opening threads one call at a
  time. Returns a headline and the matching lines rather than whole tasks, sorted most-matched first.
- `tasks_add_comment` / `tasks_get_comments`
- `labels_list`
- `documents_list` / `documents_get` / `documents_history` — the index without the texts, one document
  with its text (by reference — `raw/{project}/document/{id}` or `raw/{project}/github/{repository}/{path}`,
  which is also what a task or a goal stores and what survives being written down — or by project and path,
  and optionally at an old `version`), and every version a document has had. Split that way on purpose: a document can be a whole specification, and an agent that
  pulled all of them in to find one would have spent the context it needed for the work.
- `documents_search` / `documents_outline` / `documents_get`'s `from_line` / `to_line` / `max_bytes` —
  the three halves of reading a large document without reading it. See
  [Documents](#documents).
- `documents_edit` — change parts of a text without sending the rest of it, atomically.
- `documents_diff` — a unified diff between two versions, returning neither.
- `documents_upload` / `documents_update_path` — write a version at a path, and move a document without
  touching its text. Two calls rather than one, so the history can tell the two apart.
- `documents_delete` / `documents_trash` / `documents_restore` — the trash, which exists nowhere else: the
  browser does not show it.
- `documents_delete_folder` — a folder and everything under it, in one call. The same deletion as
  `documents_delete`, repeated over the subtree, because there is no folder to delete: emptying it IS
  deleting it, and doing that one document at a time is how a folder ends up half gone.
- `documents_next_without_brief` / `documents_set_brief` — the reading loop: hand me the next document
  nobody has summarised, and here is what it says. Filed by content hash, so it covers a project's own
  documents and its repositories' files alike, and an edit makes a document unbriefed by itself. See
  [the brief](#every-document-says-what-is-in-it--the-brief).
- `github_refresh` — clone a connected repository again and wait for it, answering with what came back
  and how many of its files nobody has read yet. The only tool here that takes minutes, and the only one
  that reaches the network.
- `tasks_resolve_id` — the counterpart to composing ids on read. Given a human-written `RMS-42` it
  answers in two parts: the **direct** hit (the project holding `RMS` right now, and the task's
  current id), and the **archived** ones — every project that used to hold `RMS`, whether task 42
  exists there, and what that task is called *today*. Without this the cost of a reusable prefix
  would be unrecoverable: someone quoting an id from an old chat would land on a different task and
  never know. With it, the tool says "`RMS-42` is now this, and it used to mean that". It answers for a
  goal (`RMS-G7`) and a release (`RMS-R7`) the same way, and for a bare number whichever of the three it
  turned out to be — which is also how a deleted release is read back, since it is in no listing.

Every tool's description is written as a **use case** — when to call it and why — not as a list of
its fields. That is what made the original board's prompt work, and it is the part worth copying.

**`/mcp` has no authorization in the first version.** It sees every project and writes to any of
them; it is closed by the perimeter alone, like `development-tasks-mcp` before it. The asymmetry
this creates — reads gated by Google login and project membership, writes gated by nothing — is
recorded in `TODO.md` as the first thing to fix.

The original `development-tasks-mcp` prompt is **not** portable: it is built on a single board,
four hard-coded statuses, four hard-coded kinds, emails resolved against the rms console's `users`
table, the `.s`-suffix rule, the `dev_mine` badge and a `/system/development-tasks?task=` link.
The ideas port; the text does not.

## UI

Dioxus CSR — a static bundle. One bar of product areas across the top, work area under it at the full
width of the window. Across the top rather than down the left side because of Home: the board is a row of
columns and the only thing it wants is horizontal room, and a left menu costs that room on every screen
to be visible on the one screen where the vertical space was free anyway.

Everything below the router goes through one `Shell` component, which owns the single question every
screen needs answered first (is anybody signed in) so no view has to handle "not asked yet" and none can
forget to.

The stylesheet is **generated**: `ui/build.rs` concatenates `ui/css/*.css` into `ui/public/assets/app.css` through
`ci_utils::css::CssCompiler`. Edit the numbered sources — an edit to `app.css` survives exactly until the
next build.

| Area | |
|---|---|
| **Home** (root URL) | The board. A project dropdown on top — only projects you may see, and not archived ones unless the archived board is the one currently open; an admin sees all — and the choice is remembered in `localStorage`. Filters by task type and by assignee, plus a search box: free text narrows the board in place, while a task id (`RMS-42`) is looked up on the server and opens as a card, because the answer may be on another board or closed longer than seven days ago and therefore not drawn at all. **Read-only:** nothing is edited with a mouse, anywhere. |
| **Releases** | What has gone out, newest first: one row per release, folded to a line and opened to read the services — version, commit, when — and the notes. A flag on the row says a service in it needs its settings changed. Filtered by microservice. **Read-only**, like the board: a release is recorded through `/mcp`. |
| **Projects setup** | Every project as one row — prefix, name, description, task count, which column template it follows, its task types, how many members. Editing is by dialog: **Edit** for what a project *is* (name, description, prefix, and which column template), then **Task types**, **Members**, and **GitHub** — which is where a repository is connected, and the only place it can be: a connection is configuration, so no MCP tool creates one. **Archive** puts a project away and is the row action that is not a dialog — nothing is destroyed and the same button brings it back, so a confirm step would only teach people to click through confirms. Archived projects are hidden here too until **Show archived** is pressed, and then carry an `archived` label beside their prefix. This screen is the only place one can be seen and brought back. Admin only. |
| **Users** | The roster. Admin only. |
| **Settings** | A menu of areas on the left, the chosen one on the right, with the area in the route (`/settings/column-templates`) so each is linkable and Back works between them. **Column templates** is where a board's columns are configured. **Diagnostics** is read-only: which `client_id` was picked up, which `redirect_uri` is expected, how many admins the settings list holds — the first thing worth reading when a sign-in fails. |

The admin areas are hidden from a non-admin. Cosmetic on its own, since the server refuses them anyway;
the point is not showing somebody three screens that all answer 403.

### The dialog pattern

One shape, everywhere: **a dialog is handed a model, editing builds a new model beside it, Save lights up
only when the two differ, and pressing Save hands the new model out through an `EventHandler`.** The
handler — owned by the page or by `RenderDialog`, never by the dialog — makes the request and refreshes.

Nothing is sent until Save, and what is sent is the whole thing. Adding a column or a task type is a local
edit, so a half-finished set never reaches the server and is never visible to anybody else; Cancel costs
nothing because nothing was sent; and one snapshot request replaced the three (add, update, delete) that
used to need an order to be applied in.

Save being disabled on an untouched form is the point rather than an oversight: comparing models — not
tracking "was touched" — means typing a value and typing it back leaves the button off, because there is
genuinely nothing to save.

The plumbing: one `DialogState` enum, a `RenderDialog` router mounted once in the shell, `dialog_template`
supplying the frame, Cancel and close, and a `DialogFeedback` signal carrying how the last submit went —
without which a failed save would leave a dialog looking busy for ever, since it has no other way to learn
the request came back. `DialogState` is a context signal of its own rather than a field of `AppState`, which
the guide would have it be: Dioxus subscribes per signal, not per field, so putting it in `AppState` would
make opening a dialog re-run Home's board read.

**Two dialogs re-open themselves instead of closing, and both do it through the router.** The connections
dialog performs several acts in a row — connect, detach, hand over a key — so the router re-opens it with a
bumped `revision` after each, and the list reloads on the change; closing after every act would make
configuring three repositories nine gestures. The refresh dialog does the same with what it got back: the
router asks for a listing per connection, and re-opens the dialog carrying the receipts, which is what turns
it from a confirmation into a watch. Neither breaks the rule that matters — the dialog reads, the router
writes.

### Columns and task types live in templates, not in a project

A **column template** is a named set of columns, and a **task-type template** a named set of task types.
Both are defined once under Settings and followed by any number of A project carries only the id of each template it follows. One with no column template has a board of just
Todo and Done; one with no task-type template has no types to choose from. Both are legitimate states, not
errors — otherwise creating a project would require creating two templates first.

The indirection earns itself twice. The projects on one board mostly share a workflow, so per-project
columns meant typing the same four columns into every project and watching them drift. And a template is a
thing you change once and have every project follow.

In memory, `ProjectModel.columns` and `.kinds` are a **cache**, not the source of truth:
`BoardInner::rebuild_indexes` recomputes both from the templates on every write, so they cannot drift, and
editing a template moves every project following it within the same swap. That is what keeps the
indirection to one function — `has_column`, `effective_status`, `has_kind`, the board read and the MCP tools
all still ask a project for its own columns and types and never learn templates exist.

Deleting a template with any followers is refused. For columns the reason is sharp: a project whose template
vanished loses the middle of its board and every task sitting there reads as Todo. Losing a task type is
milder — a task pointing at one that is gone reads as having no type — but it still changes every following
project at once, so both are made deliberately rather than discovered.

### Everything is a POST with a body — no path parameters, no query values

`#[http_path]` fields are **appended** to the url in declaration order — `append_path_segment`, no `{name}`
substitution. So `/api/projects/v1/{project}/columns` is unreachable from the generated client: it can
only build `/api/projects/v1/{project}`, and the server answers 404. Seven of the ten project endpoints
were written that way and every one of them was dead on arrival.

So there is no `#[http_path]` anywhere in this repo. And no `#[http_query]` either, because a query value
has its own way of being mangled: **`+` in a query means a space.** Both halves of the OAuth callback are
base64-ish and regularly contain one, so sending them as query parameters silently corrupted the CSRF state
on roughly six sign-ins in ten — the ones whose state happened to contain a `+` — and surfaced as a 401 that
looked random and cleared itself on a retry.

Every endpoint the client calls is therefore a `POST` to a static url with everything in a JSON body, which
carries bytes verbatim. Reads too: `/api/projects/v1/list`, `/api/tasks/v1/list`. The lists live at `/list`
rather than at the collection root because create already owns the root as a POST, and two POST handlers on
one route is a collision.

One exception: `GET /api/system/v1/ping`. It is an infrastructure liveness probe reached by is-alive and the
proxy, not by the client — making it a POST would silently break the health check.

Sign-in is one button: `/api/auth/v1/google-url` → Google → back to `/authorized`, which exchanges the
code, stores the token and replaces the URL so a reload cannot re-submit a spent code. Signing out
navigates rather than routes, because it has to throw away the WebSocket task and the cached `/me` that a
route push would leave running under the login screen.

Home holds a WebSocket and repaints on a push. What travels is an **invalidation signal** — "this
project changed" — not a delta: the client re-reads through the normal REST call. Nothing can
drift out of sync, because there is no second copy of the state on the client. This matters more
than it would in a normal admin panel: every change originates outside the UI, so a static screen
would simply be lying.

Sticker text renders as Markdown (the `markdown` crate, which escapes raw HTML rather than passing
it through). A sticker links to itself as `?search=RMS-42` — the one button on a card copies that link, and
the board follows a handle found in the URL at mount to the task's own project and opens it. The link is the
search box written down, which is why there is no second parameter to keep working; the prefix is globally
unique, so the project is not part of it. An assignee email with no matching user row is shown as the raw
email.

## Running it

Runs on **HETZNER** as one container, `task-manager-mcp` — named after the product, whose namespace
and release-mcp stack (`services/task-manager-mcp`) carry the same name. One host
port from the **31500+** range: `31500 → 8000`. The container port is not arbitrary — service-sdk's HTTP
server listens on 8000 (8888 is its second, technical port).

`release/settings-template.yaml` is the settings-service template
(`product_id = task-manager-mcp`, `template_id = task-manager-rest-api`) and
`release/secrets.md` is the inventory of every secret it references. Two entries will refuse to start the
service if they are wrong, on purpose:

- `SessionEncryptionKey` must be **exactly 48 bytes** — `AesKey` panics on any other length, and checking
  it at startup beats dying on the first sign-in hours later (`openssl rand -hex 24`);
- `Admins` must name at least one address, or nobody can get into an empty database.

`release/docker-compose.yaml` follows the standard single-VM unix-socket layout, with one deliberate
deviation from the template: the memory limit is 256Mb rather than the usual 64Mb, because the service
holds the whole product in memory and replaces a snapshot on every write.

The client calls the REST API on relative `/api/...` URLs and opens `/ws` on the same origin — the origin
it was served from, which is this container. It never knows a base URL and there is nothing in it to
configure, and the reverse proxy has one upstream: see `release/reverse-proxy.md`.

The settings template is still called `task-manager-rest-api`, the name the server had while the client
was a container of its own. It is a record in settings-service and was left alone on purpose — renaming
a crate is not a reason to make a deploy wait on a rename in another system. The same goes for nothing
else: the image, the unix socket and the name the service logs under are all `task-manager` now, after
the crate. The container is the one thing named after the product instead — `task-manager-mcp`, and
with it the compose service and the hostname on `docker_net`.

Once it is up, register the MCP surface with the client that will work the board:

```json
"task-manager": { "type": "http", "url": "https://<host>/mcp" }
```

Locally: `cargo run` at the root for the server, which serves whatever is in `wwwroot/`; `dx serve` in
`ui/` while working on the client. Locally the settings come from `~/.task-manager`; in the container,
from `SETTINGS_URL`.

### Releasing — a version number, and the client is built first

One repository is one service, so a release is a version number:

```
./build-ui.sh                                        # only if ui/ or shared/ changed
git add wwwroot && git commit                        # … and commit what it produced
gh release create 0.2.0 --title "0.2.0" --notes ""
```

`gh release create` makes the tag, the tag starts `.github/workflows/release.yaml`, and the image comes
out as `ghcr.io/my-ai-utils/task-manager:0.2.0`. A tag with no release behind it would build as well —
the workflow fires on any tag, as `my-no-sql-server`'s does — but the release is the record of what went
out, so it is created with `gh` and not with `git push --tags`.

**`wwwroot/` is committed, and that is the whole of how the client gets into the image.** The workflow
builds the server and nothing else; the Dockerfile copies `wwwroot/` as it stands at the tag. So what a
release serves is exactly what is in the repository — and so `./build-ui.sh` is a step of making a
release, not something CI does for you. It runs `dx build --release --web` in `ui/`, stamps a fresh id
onto every asset url in `index.html` (`ui/build.py`), and replaces `wwwroot/` with the result.

Two things follow from that, and both are the price of the arrangement rather than accidents:

- **a release cut without rebuilding ships the previous client with the new server.** Nothing checks it.
  The workflow refuses a tag with no `wwwroot/index.html` at all, which catches a repository that never
  had a bundle and not one that has a stale one. The wire models are shared, so a change under `shared/`
  is a change to the client too;
- **every rebuilt client is a few megabytes of history.** The wasm is the bulk of it and its name is
  hashed, so each build is a new file as far as git is concerned.

The build is a plain `cargo build --release` on the runner, about ten minutes, with the dependency graph
compiled from scratch each time. There was a pre-baked builder image here that cut it to two; it went
with the two-service layout, and the shape now is `my-no-sql-server`'s.

**`Cargo.lock` at the root is committed**, un-ignored explicitly in `.gitignore`. Most dependencies are
spelled `"*"`, so with the lock ignored the same tag built twice would be two different binaries and one
bad patch published anywhere in a ~400-crate graph would break a release that changed nothing. Verify
with `cargo check --locked` before committing a change to it.

`.github/workflows/test.yaml` runs the server's tests and the wire crate's on every push to `main`. The
client's are not there — they are a native build of a Dioxus app, a second dependency graph the size of
the first, for a crate CI never builds otherwise. Run them with `cargo test` in `ui/`.

## Open

- The **share link** is built against the page's own origin, so it assumes the board is at the root of the
  public host — which it is: `https://task-manager.jetdev.eu/?search=RMS-42`.
- **Copying it needs a secure context.** `navigator.clipboard` exists on https and on localhost, which is
  both of the ways the board is reached; served over plain http the button shows the link in a dialog to be
  copied by hand instead.
- **No WebSocket reconnect.** A dropped socket leaves Home static until the page is reloaded. The dot in
  the header goes grey so it is visible rather than silent, but a laptop waking from sleep currently needs
  a refresh.
