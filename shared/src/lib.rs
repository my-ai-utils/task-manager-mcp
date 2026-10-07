//! The wire contract between the server (`task-manager`, the crate at the repository root) and the
//! browser client (`task-manager-ui`, in `ui/`).
//!
//! Request models derive `MyHttpInput` — on the client that is the FlUrl request builder, on the
//! server (with the `server` feature) the same markup also parses the incoming request. Response
//! models derive `MyHttpObjectStructure`. Everything here is plain data: behaviour belongs to an
//! extension trait on whichever side needs it.
//!
//! Two conventions worth knowing before adding a model:
//!
//! * **Never `///` on a field** of a struct deriving any of these macros. The shared attribute
//!   parser has no branch for `#[doc = "…"]` and panics with `Somehow we got Punct here: =`,
//!   pointing at the `#[derive]` line rather than the comment. Use `//`.
//! * **Open vocabularies travel as `String`, not as an enum** — a column id, a kind id, a kind
//!   colour. They are configured per project at runtime, so an enum would turn "someone added a
//!   column" into a 500 on a read endpoint. The reader decides what to do with a value it does not
//!   know: an unknown status reads as `todo`, an unknown colour falls back to the default swatch.

pub mod auth;
pub mod column_templates;
pub mod documents;
pub mod github;
pub mod goals;
pub mod kind_color;
pub mod kind_templates;
pub mod priority;
pub mod project_transfer;
pub mod projects;
pub mod releases;
pub mod subtasks;
pub mod system;
pub mod task_title;
pub mod tasks;
pub mod templates_transfer;
pub mod users;
pub mod ws;
