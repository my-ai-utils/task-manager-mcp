# task-manager-mcp

A task board that **agents work and people watch**.

The work itself — creating tasks, moving them, commenting, closing goals, recording releases — arrives
through an **MCP server**. The **browser** is where a person sets the boards up and reads them. Three
things are done there with a mouse and nothing else is: a card is dragged between columns, a goal's
colour is picked, and a document is uploaded.

It is one service in one container: it answers MCP and the REST API, and serves the browser client
itself.

- [What it does](#what-it-does)
- [The browser](#the-browser)
- [The MCP surface](#the-mcp-surface)
- [Running it](#running-it)

## What it does

### Projects

A project is a board. A task, a goal, a release and a document each belong to exactly one.

- **A prefix names it, and names everything on it.** A project called `RMS` has tasks `RMS-42`, goals
  `RMS-G7` and releases `RMS-R12`. The number comes from one counter per project, so it is never used
  twice — `RMS-12`, `RMS-G12` and `RMS-R12` are at most one real thing. An id is put together from the
  project's *current* prefix whenever it is shown: a prefix can be renamed, and an id quoted from before
  the rename still resolves. `tasks_resolve_id` says what an old id means now.
- **Columns.** `todo` and `done` are on every board. What stands between them comes from a **column
  template**: a named set of columns defined once under Settings and followed by any number of projects.
  A project with no template has a board of just Todo and Done.
- **Task types.** A type has a name, a colour and an icon, and comes from a **task-type template** the
  same way. A project with none simply has no types to choose from. (On the wire a type is called `kind`.)
- **Labels** are free tags. A label exists for as long as some task carries it.
- **Members.** A person sees a project if they are a member of it; an admin sees all of them.
- **Archive window.** Finished work leaves the board after a number of days — seven unless the project
  says otherwise. Nothing is deleted: it is still reachable by id and by asking for archived work.
- **Archiving a project** puts it away: it leaves every project picker and nothing else. Its tasks and
  documents still open by link, and it keeps its prefix. A project is never deleted.

### Tasks

| | |
|---|---|
| Text | Markdown. |
| Status | One of the project's columns. A task always starts in `todo`. |
| Priority | `super-high`, `high`, `normal`, `low` or `super-low`. Every list comes back most urgent first, oldest first within one priority. |
| Type | One of the project's task types. Optional. |
| Goal | The goal it is part of. Optional — a task may stand on its own. |
| Assignee | A person's email, or the reserved `AI` for work that is an agent's to do. |
| Labels | Free tags. |
| Depends on | Other tasks of the same project. A task is reported as `blocked` while any of them is not done, and each task lists the ones waiting on it. |
| Checklist | Small steps kept inside the task: a title, an optional longer text, and a tick. They block nothing — a step somebody else has to see is a task of its own. |
| Documents | References to the documents the work is done against. |
| Builds | Links to the CI runs that came out of the work, each with a title. |
| Thread | Comments: who said it, and what, in Markdown. |

- **Landing a task in `done` needs a comment** saying what was done. The move is refused without one,
  from MCP and from the browser alike.
- **Deleting is a flag.** A deleted task leaves every board and list, is still found by its id — which
  reports it as deleted — and comes back with `deleted: false`.

### Goals

A goal is an epic: the outcome a group of tasks is working towards, and the place the conversation about
it is kept.

- It has a name, a Markdown description, a colour, a priority of its own, a checklist, document
  references and a thread.
- **Tasks point at their goal**, and the goal reports its progress as done tasks out of all of them,
  counting work that has already left the board.
- **Closing a goal has two conditions**, and both are enforced: every task under it is done, and the
  call carries a comment — the resolution, which is what somebody reads months later to find out how it
  went.
- **Deleting is not closing.** Closing says how a goal went; deleting says it should never have existed.
  The tasks under a deleted goal are not deleted — they read as standing on their own.
- **A goal has a start and an end.** The start is when work on it began: set through MCP, or stamped
  by itself when the first task under it leaves Todo. The end is when it was closed — moved to done —
  and can be given a date of its own. A task's start is stamped when it leaves Todo.
- A closed goal leaves the screen after the project's archive window, like finished work.
- A goal lists the **releases** it went out in.

### Releases

A release is the record that a feature **went out**, and in what. One release is one feature, across
however many microservices it touched.

- **What changed:** a title, a description, release notes and the date of the release.
- **What was deployed:** one entry per microservice —

  | | |
  |---|---|
  | `microservice_id` | The name the service is deployed under. A release names a service once; reporting it again corrects that entry. |
  | `version` | Which version went out. |
  | `git_hash` | The commit it was built from. It has to be a hash — a branch or a tag is refused. |
  | `release_link` | Where the build can be looked at: the GitHub release, or the run that built the image. Optional. |
  | `datetime` | When this service went out. |
  | `settings_update_note` | What has to change in the service's settings for the release to work. Its own field, so it cannot be missed; the board flags a release that carries one. |
  | `description` | Anything else about this service's part of the release. |

- **Where it is out:** `envs`, a list of environment labels — `Dev`, `Prod` — in the order the release
  reached them. Reaching the next environment is a label added to the same release, not a second release.
  A label is one word, matched in any case, and is stored the way the project already spells it.
- **Whether its rollout is over:** `done`. A release is closed once it is out on every environment it is
  going to. That is somebody's statement — nothing knows which environments a project has — and it can be
  reopened.
- **The goal it shipped.** The goal lists the release, so the link is made with `goal` when the release
  is recorded, or later from the goal's side.
- **A thread**, for how the rollout went — as opposed to the notes, which say what changed.
- **An address of its own:** `/release/{project}/{release}` — `/release/RMS/RMS-R12`, or just the number —
  opens one release on a page that can be sent to somebody.
- The list is the whole history, newest first; nothing ages off it. Deleting a release is a flag, for one
  recorded by mistake.

### Documents

A project has documents: specifications, decisions, reference material. Tasks and goals **reference**
them instead of copying them into their own text.

- A document lives at a path — `docs/design/system.md` — and is text or a file: Markdown, HTML, a PDF, an
  image. Folders are just the paths.
- **Every version is kept.** A document has a history, any two versions can be compared, and an old one
  can be read back.
- **Deleting moves it to the trash**, from where it is restored with its id and history intact — so every
  task that referenced it works again.
- **A large document is worked on in pieces:** search across a project's documents, the outline of one,
  a range of its lines, and an edit that sends only what changes.
- **A brief** says what a document is about, so that an agent choosing what to read does not have to
  open each one.
- In the browser: a tree on the left, the document on the right — Markdown rendered, a PDF or an image
  shown, an HTML page shown in a sandbox. Documents and whole zip archives are uploaded from here.

### Connected GitHub repositories

Reference material usually lives in a repository already, so a project can **connect** one. It then
appears among the project's documents, under `github/<name>/…`, and is read with the same tools.

- Connected by an admin in the browser, under **Projects setup → GitHub**: a name, a url, optionally a
  branch and a folder inside the repository. Nothing on the MCP surface creates a connection.
- It is a real clone, fetched every ten minutes. **Refresh** — the button, or `github_refresh` — clones
  it again.
- Its files are read-only to the document tools. `github_git` runs git in the clone, which is how a
  repository is asked questions (`log`, `diff`, `blame`, `grep`) and how a change is pushed.
- A private repository needs a **key** — a GitHub token. What the key is allowed to do is decided on
  GitHub: a read-only token clones and fetches, a read-and-write one also pushes.
- **Sync from GitHub** copies files out of a repository into the project's own documents.

### Search and links

- `tasks_search` finds work by what was *written* about it — task texts, goal descriptions, checklists
  and every comment thread — and answers with the matching lines.
- `documents_search` does the same across a project's own documents. A connected repository is searched
  with git — `git grep`, through `github_git`.
- In the browser, the search box on Home narrows the board as you type; a task id typed there opens that
  task, even from another board or after it has left this one.
- **A task has a link:** `https://<host>/?search=RMS-42`. The button on a card copies it.
- **A release has a link:** `https://<host>/release/RMS/RMS-R12`. The `↗` on a release opens it in a new
  tab.

### Moving a project — export and import

**Export**, on a project's row in Projects setup, downloads the whole board as a zip: its tasks, goals,
releases and comments as YAML, and its documents as files. **Import** pours such an archive into a
project. Everything is renumbered out of the receiving project's counter and every reference is remapped;
what could not be carried over is reported. Column and task-type templates are exported and imported
separately, under Settings.

### People and access

- **Sign-in is Google.** A session lasts twelve hours.
- **Users** are a roster kept by admins: an email, a name, an admin flag, and `disabled`. A disabled user
  cannot sign in; their assignments and comments stay where they are.
- **An admin** is a user with the flag — or an address listed in the service settings, which is what
  lets the first person into an empty installation.
- Projects, templates and users are configured by admins only.

## The browser

| Screen | |
|---|---|
| **Home** | The board of one project: columns of cards, most urgent first. Filters by task type, assignee and goal, and a search box. A card is dragged to another column; a double-click opens it with its text, checklist, documents, builds and thread. |
| **Goals** | Every goal with its progress, priority and status — Todo, In Progress or Done — and its tasks folded underneath. The eye opens the goal: its text, and tabs for its comments and for the releases it went out in. This is also where a goal's colour is picked. **Timeline** shows the same goals as a Gantt chart over one month: a bar from the day a goal started to the day it was closed (or to today), a dashed line for the time it waited before it started, and marks for its releases and for the days its tasks were done. A click on a goal unfolds its tasks underneath, each with its own wait and bar; the eye opens the goal. ‹ and › move between months, closed goals of past months included; **Hide done** puts away the goals and tasks that are finished. |
| **Releases** | What has gone out, newest first. Each row shows the goal it shipped, the environments it is on, whether it is done, a flag when a service needs its settings changed, and the versions. Filters by microservice, by environment and by state: it opens on what is still in progress, and the browser remembers the filters it was left with. |
| **Documents** | The project's documents and its connected repositories as one tree, with a viewer beside it. |
| **Projects setup** | Admins only. Every project, with **Edit** (name, description, prefix, templates, archive window), **Members**, **GitHub**, **Export**, **Import** and **Archive**. |
| **Users** | Admins only. The roster. |
| **Settings** | Admins only. **Column templates**, **Task-type templates**, and **Diagnostics** — which Google client and redirect address the service picked up, the first thing to read when a sign-in fails. |

The board keeps itself current: a change made by an agent shows up without a reload.

## The MCP surface

The endpoint is `/mcp` on the same host. Everything is named the way a person names it — a project is
`RMS`, a task is `RMS-42` — and every tool describes itself as a use case: when to call it, and why.

| | |
|---|---|
| `projects_list` | The boards, their columns, task types and labels. Called first: everything else names a project by what this returns. |
| `users_list` | Who exists. How a spoken name becomes the email that goes into `assignee`. |
| `labels_list` | The tags a board already uses. |
| `tasks_list` · `tasks_create` · `tasks_update` · `tasks_delete` | Read a board; put a task on it; move it, rewrite it, reassign it, tick its checklist, attach documents and builds; delete it. |
| `tasks_add_comment` · `tasks_get_comments` | A task's thread. |
| `tasks_search` | Find work by what was written about it. |
| `tasks_resolve_id` | What an id somebody quoted refers to now — a task, a goal or a release. |
| `goals_list` · `goals_create` · `goals_update` · `goals_delete` | The goals of a project; open one; rename, rewrite, start, close or reopen it — with a start date and an end date of its own — and say which releases it went out in; delete it. |
| `goals_add_comment` · `goals_get_comments` | A goal's thread. |
| `releases_list` · `releases_create` · `releases_update` · `releases_delete` | The releases of a project, filtered by goal, microservice, environment or whether they are done; record one; add services and environments to it, or close it; delete it. |
| `releases_add_comment` · `releases_get_comments` | A release's thread. |
| `documents_list` · `documents_get` · `documents_upload` · `documents_update_path` | The index of a project's documents; one of them, whole or by line range; write a version; move or rename. |
| `documents_search` · `documents_outline` · `documents_edit` · `documents_diff` · `documents_history` | Work on a large document without sending all of it, and see what changed. |
| `documents_delete` · `documents_delete_folder` · `documents_trash` · `documents_restore` | The trash. |
| `documents_next_without_brief` · `documents_set_brief` | Read the next document nobody has summarised, and record what it says. |
| `github_refresh` · `github_git` | Clone a connected repository again; run git in it. |

**`/mcp` has no authorization.** It sees every project and writes to any of them, so it has to be closed
off by the network it is reachable from. See `TODO.md`.

## Running it

### What it needs

- **Postgres** — a database and a user of its own. The service creates its tables at start and adds
  whatever a new version needs.
- **A Google OAuth client** of type *Web application*, with `https://<host>/authorized` among its
  authorised redirect URIs.
- **Its settings.**

Run **one instance**. A second one would not see what the first one did.

### Settings

| Key | |
|---|---|
| `postgres_conn_string` | `host=… port=5432 dbname=… user=… password=…` |
| `google_client_id` | From the Google console. |
| `google_client_secret` | The secret half of the same client. |
| `google_redirect_uri` | `https://<host>/authorized`, byte for byte as registered in the console. |
| `session_encryption_key` | **Exactly 48 bytes** — `openssl rand -hex 24`. Anything else stops the service at start. |
| `admins` | A list of emails that are admins whatever the roster says. At least one, or nobody can get into an empty installation. |
| `seq_conn_string` | Where the logs go. |
| `git_repos_path` | Where connected repositories are cloned. Optional; `/root/git-repos` by default. |
| `my_telemetry` | Where telemetry goes. Optional, and left out of the deployed template. |

They are read from a YAML file at `~/.task-manager` when there is one, and otherwise from the address in
`SETTINGS_URL` — which is how the container gets them from settings-service.
`release/settings-template.yaml` is the template registered there, and `release/secrets.md` lists the
secrets it refers to.

### In a container

The image is `ghcr.io/my-ai-utils/task-manager:<version>`. `release/docker-compose.yaml` is the stack as
it is deployed:

- the service listens on **8000**, published on the host as `31500`;
- `SETTINGS_URL` points at the settings template;
- a **volume** is mounted on `/root/git-repos`, so connected repositories are not cloned again on every
  deploy;
- the memory limit is 256Mb: the whole board is held in memory.

A reverse proxy in front of it needs one upstream — the same port answers the pages, `/api`, `/mcp` and
the live updates. `release/reverse-proxy.md` has the configuration for `my-reverse-proxy`.

After a restart, a **private** connected repository needs its key given again in Projects setup → GitHub.
Keys are kept in memory only, so that no database dump carries one; until then the repository's files stay
readable and it is simply not fetched.

### Connecting an agent

```json
"task-manager": { "type": "http", "url": "https://<host>/mcp" }
```

### Locally

```
cargo run                # the server, at the root; settings from ~/.task-manager
cd ui && dx serve        # the client, while working on it
```

The server serves whatever is in `wwwroot/`. Tests: `cargo test` at the root, in `shared/` and in `ui/`.

### Building the client

```
./build-ui.sh
```

builds `ui/` and replaces `wwwroot/` with the result. **`wwwroot/` is committed**: the image is the server
binary plus that folder exactly as it stands in the repository. So after any change under `ui/` or
`shared/` it is rebuilt and committed — a release cut without it ships the previous client with the new
server.

### Releasing a version

```
./build-ui.sh                                        # only if ui/ or shared/ changed
git add wwwroot && git commit                        # … and commit what it produced
gh release create 0.2.0 --title "0.2.0" --notes ""
```

The release creates the tag, the tag starts `.github/workflows/release.yaml`, and a couple of minutes
later — longer when the dependencies have changed — the image is at
`ghcr.io/my-ai-utils/task-manager:0.2.0`. It is then rolled out by putting that tag into
`release/docker-compose.yaml` and applying the stack.

## What is where

```
src/               the server
shared/            the models both sides speak in
ui/                the browser client
wwwroot/           the client, built — committed, and what the image serves
release/           docker-compose, the settings template, the secrets list, the proxy configuration
Dockerfile         the binary plus wwwroot
build-ui.sh        ui/ -> wwwroot/
TODO.md            what was put off
```

## Known limits

- **`/mcp` has no authorization** — see above.
- **A link opened by somebody who is not signed in lands on Home** after they sign in, not on the task or
  the release it pointed at. They have to open it again.
- **If the board loses its connection to the server it stops updating** until the page is reloaded.
