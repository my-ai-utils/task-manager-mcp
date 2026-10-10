mod local_storage;

mod session;

pub use session::*;

mod project;
pub use project::*;

pub mod goals;
pub mod releases;

/// For the tests of a state that writes through to storage: how many writes this test has made so far.
#[cfg(test)]
pub use local_storage::writes as storage_writes;
