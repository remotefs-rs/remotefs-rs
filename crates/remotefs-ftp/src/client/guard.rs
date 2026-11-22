//! Transfer bookkeeping shared by the blocking and asynchronous clients.
//!
//! A [`TransferGuard`] marks a client as busy while an owned transfer stream
//! is alive and can mark the control connection unusable when cleanup fails.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Marks a client as having an in-flight transfer until dropped.
pub(crate) struct TransferGuard {
    active: Arc<AtomicBool>,
    connection_usable: Arc<AtomicBool>,
}

impl TransferGuard {
    /// Takes ownership of an already-raised `active` flag.
    pub(crate) fn new(active: Arc<AtomicBool>, connection_usable: Arc<AtomicBool>) -> Self {
        Self {
            active,
            connection_usable,
        }
    }

    /// Marks the control connection unusable until the client reconnects.
    pub(crate) fn invalidate(&self) {
        self.connection_usable.store(false, Ordering::SeqCst);
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[test]
    fn should_release_the_transfer_flag_on_drop() {
        let active = Arc::new(AtomicBool::new(true));
        let guard = TransferGuard::new(Arc::clone(&active), Arc::new(AtomicBool::new(true)));
        assert!(active.load(Ordering::SeqCst));
        drop(guard);
        assert!(!active.load(Ordering::SeqCst));
    }

    #[test]
    fn should_mark_the_connection_unusable_when_cleanup_fails() {
        let active = Arc::new(AtomicBool::new(true));
        let usable = Arc::new(AtomicBool::new(true));
        let guard = TransferGuard::new(Arc::clone(&active), Arc::clone(&usable));

        guard.invalidate();

        assert!(!usable.load(Ordering::SeqCst));
        drop(guard);
        assert!(!active.load(Ordering::SeqCst));
    }
}
