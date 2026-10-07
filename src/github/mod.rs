//! Connected GitHub repositories, cloned onto a volume and shown in a project's documents under
//! `github/`.
//!
//! **A connection is configuration; a clone is a working copy of somebody else's repository, and this
//! product only ever READS it.** The four things a person set up — a name, a repository, a branch, a
//! folder — are a row on the project and survive everything. What was cloned lives on a mounted disk and
//! survives a restart with them, and is replaceable: pressing Refresh clones the repository into a
//! folder of its own and swaps the two when it is complete. The key that reaches GitHub lives in this
//! process's memory and does not survive a restart.
//!
//! The reason the key is kept apart is what it is. A token written to Postgres is a token in every backup
//! and every dump of that table, for as long as anybody keeps one; a token held in memory is gone when
//! the process is. The price used to be that a private repository stopped working entirely after a
//! deploy. It no longer is: the clone is still there, so the files still list and still read — only
//! reaching GitHub waits for somebody to type the key again.
//!
//! What the rest of the service sees of all this is a path: `github/<connection>/<file>`, served by the
//! same `documents_list` and `documents_get` that serve the project's own documents. **The writing tools
//! refuse it.** `documents_upload`, `documents_edit`, `documents_delete` and `documents_update_path` all
//! stop at the reserved root, because the folder they would write into is REPLACED on the next refresh —
//! a write there is work that disappears without anybody being told. The way to change one of these files
//! is to change it in the repository; the way to get a copy that this board owns and keeps versions of is
//! to sync it into the project's own documents.

mod connection;
mod mirror;
mod puller;

/// Running git, and the one rule about what may be run. See [`git::parse_git_command`].
pub mod git;
/// The working copy on disk: where it is, how it gets there, and reading and writing files in it.
pub mod workdir;

pub use connection::*;
pub use mirror::*;
pub use puller::*;
