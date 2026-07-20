//! Cooperative cancellation for in-flight session loads.
//!
//! Long parses (multi-MB Claude JSONLs) used to run to completion even
//! when the user already navigated away. The session_id-keyed flag
//! lives on `AppState`; the parsing thread polls a thread-local view
//! and bails out at the next checkpoint.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub type CancelFlag = Arc<AtomicUsize>;

thread_local! {
    static CURRENT: RefCell<Option<CancelFlag>> = const { RefCell::new(None) };
}

/// Make a fresh flag in the un-canceled state.
pub(crate) fn fresh() -> CancelFlag {
    Arc::new(AtomicUsize::new(0))
}

/// Trip the flag. Idempotent.
pub(crate) fn cancel(flag: &CancelFlag) {
    flag.store(1, Ordering::Relaxed);
}

/// Check whether the flag held by the current spawn_blocking thread (if
/// any) has been tripped. Returns false when no flag is installed.
pub(crate) fn is_canceled() -> bool {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed) != 0)
    })
}

/// Install `flag` as the cancellation flag for the current thread for the
/// duration of `work`. The previous flag (if any) is saved and restored
/// on return so nested `run_with` calls observe correct lexical scoping;
/// reused tokio worker threads don't leak state into the next task.
pub(crate) fn run_with<R>(flag: CancelFlag, work: impl FnOnce() -> R) -> R {
    struct Guard {
        previous: Option<CancelFlag>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.previous.take());
        }
    }

    let previous = CURRENT.with(|c| c.borrow_mut().replace(flag));
    let _guard = Guard { previous };
    work()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_reads_false_outside_run_with() {
        assert!(!is_canceled());
    }

    #[test]
    fn run_with_installs_and_clears_flag() {
        let flag = fresh();
        run_with(flag.clone(), || {
            assert!(!is_canceled());
            cancel(&flag);
            assert!(is_canceled());
        });
        // Cleared after closure.
        assert!(!is_canceled());
    }
}
