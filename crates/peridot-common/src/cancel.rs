//! Lightweight cooperative cancellation handle.
//!
//! The TUI's Esc interrupt path flips a shared atomic so the agent loop can
//! observe the request between turns (or, for cancellation-aware operations,
//! mid-call via `is_cancelled()`). Lives in `peridot-common` so both
//! `peridot-core` (agent loop) and `peridot-tools` (long-running tool
//! executions like `shell_exec`) can read the same flag without a circular
//! crate dependency.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cooperative cancellation flag clonable across tasks.
#[derive(Clone, Default, Debug)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    /// Creates a fresh, un-cancelled token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the token as cancelled. Safe to call from any task.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Returns true once `cancel()` has been called at least once.
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_token_is_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancel_propagates_across_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }
}
