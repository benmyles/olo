//! Olo's locks. In release builds they are `std::sync`'s; in debug builds
//! (tests, test sessions, `wm-fuzz` and `wm-sweep`) they are
//! [tracing-mutex](https://docs.rs/tracing-mutex)'s: every acquisition
//! adds an edge from the locks the thread holds to the one it takes, and
//! one that would close a cycle (taking a lock again on the thread that
//! holds it, or two locks taken in both orders) panics there and then,
//! with the stacks of both orders, before any thread hangs. That is how
//! Olo has hung before (Smithay's surface lock taken again inside
//! `with_states`; Deskbar's status lock taken again by what it called):
//! use these, not `std::sync`'s, for any lock that's held while calling
//! other code.
//!
//! Built with `--cfg olo_lockbud` (`tools/dev/lockcheck lockbud`) they
//! are `std::sync`'s in every build, for lockbud's static analysis, which
//! knows std's locks and not tracing-mutex's.

#[cfg(not(olo_lockbud))]
pub use tracing_mutex::stdsync::{Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(olo_lockbud)]
pub use std::sync::{Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(test)]
mod tests {
    use super::*;

    /// Two locks taken in both orders: the second order panics in a
    /// debug build, before anything waits. (Release builds don't check:
    /// `cargo test --release` would fail it.)
    #[test]
    #[cfg(all(not(olo_lockbud), debug_assertions))]
    #[should_panic(expected = "cycle")]
    fn an_order_and_its_reverse_panic() {
        let (a, b) = (Mutex::new(0), Mutex::new(0));
        {
            let _a = a.lock().unwrap();
            let _b = b.lock().unwrap();
        }
        let _b = b.lock().unwrap();
        let _a = a.lock().unwrap();
    }
}
