//! Moving a whole project between boards: out as a zip, and back in.
//!
//! **The file is the contract**, which is why the shape of it lives in [`models`] with the reasoning on
//! every field rather than being implied by the code that writes it. An export is six YAML files and a
//! folder — `project.yaml`, `goals.yaml`, `tasks.yaml`, `comments.yaml`, `releases.yaml`, `documents.yaml`,
//! and `documents/` holding the project's documents as themselves, at their own paths. `documents.yaml` is
//! what makes the references on the cards survive the crossing: it carries each document's ID, which the
//! folder cannot, since a folder is keyed by path. Prose travels base64-encoded so that a task's
//! Markdown cannot be re-interpreted by a YAML reader or silently re-indented by a hand edit; ids, statuses,
//! labels, emails and moments stay legible.
//!
//! The two directions are deliberately asymmetric in where they hold the archive:
//!
//! * **out** builds onto disk — the temp directory — and streams the file back, so a board full of PDFs does
//!   not decide this service's memory footprint. See [`export`];
//! * **in** takes the zip as the raw request body and reads it from there, which is where it already is.
//!
//! Neither direction goes near `create_task`, `create_goal` or `create_release`. Those enforce the rules of
//! *doing the work* — a task lands in Todo, moving it to Done owes a comment, a release names the commit it
//! was built from — and an import is not somebody doing work: it is the same board arriving somewhere else,
//! and every one of those rules was already satisfied where it happened. So the models are built directly
//! and written through the repos, exactly as the startup load does with what it reads out of Postgres.

mod export;
mod import;
mod models;

pub use export::*;
pub use import::*;
