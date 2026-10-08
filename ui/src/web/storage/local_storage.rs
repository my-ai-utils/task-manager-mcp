//! `localStorage`, for everything this module keeps there.
//!
//! **One door, and it is `dioxus_utils::js::LOCAL_STORAGE`.** The storage is never taken from
//! `web_sys::window()` here. That object draws the one line worth drawing: a key that is not there is `None`,
//! which is the ordinary case and reads as "start from scratch"; storage that cannot be obtained at all, or
//! that refuses a write, is a panic with its reason in the console — never "nothing is stored". The accessor
//! this replaced folded the second into the first and dropped a refused write on the floor, so a browser
//! that would not keep anything looked exactly like a first visit, on every visit, with nothing to say why.
//!
//! The reason there is a module here at all, rather than each caller naming `LOCAL_STORAGE`, is the other
//! half of this file: this crate's tests run natively, where there is no browser to ask. Under `cfg(test)`
//! the same three calls are answered by a map — one per test, since each test runs on its own thread —
//! which is what lets a state that writes through to storage be tested for WHAT it wrote and for what it
//! did not.

#[cfg(not(test))]
mod imp {
    use dioxus_utils::js::LOCAL_STORAGE;

    pub fn get(key: &str) -> Option<String> {
        LOCAL_STORAGE.get(key)
    }

    pub fn set(key: &str, value: &str) {
        LOCAL_STORAGE.set(key, value);
    }

    pub fn delete(key: &str) {
        LOCAL_STORAGE.delete(key);
    }
}

#[cfg(test)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    thread_local! {
        static HELD: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
        static WRITES: Cell<usize> = const { Cell::new(0) };
    }

    pub fn get(key: &str) -> Option<String> {
        HELD.with(|held| held.borrow().get(key).cloned())
    }

    pub fn set(key: &str, value: &str) {
        WRITES.with(|writes| writes.set(writes.get() + 1));
        HELD.with(|held| {
            held.borrow_mut().insert(key.to_string(), value.to_string());
        });
    }

    pub fn delete(key: &str) {
        WRITES.with(|writes| writes.set(writes.get() + 1));
        HELD.with(|held| {
            held.borrow_mut().remove(key);
        });
    }

    /// How many times this test has written or deleted anything — for the tests that are about a state
    /// NOT writing.
    pub fn writes() -> usize {
        WRITES.with(|writes| writes.get())
    }
}

pub use imp::*;
