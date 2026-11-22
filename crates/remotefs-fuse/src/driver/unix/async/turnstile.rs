//! FIFO ordering for requests that must not be reordered on one file handle.

use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Notify;

/// Hands out tickets synchronously and lets tasks wait for their turn.
#[derive(Debug, Default)]
pub(crate) struct Turnstile {
    next: AtomicU64,
    serving: AtomicU64,
    notify: Notify,
}

/// A place in line that releases the next ticket when dropped.
#[derive(Debug)]
pub(crate) struct Turn<'a> {
    turnstile: &'a Turnstile,
}

impl Turnstile {
    /// Take the next ticket synchronously.
    pub(crate) fn ticket(&self) -> u64 {
        self.next.fetch_add(1, Ordering::AcqRel)
    }

    /// Wait until `ticket` is being served.
    pub(crate) async fn wait(&self, ticket: u64) -> Turn<'_> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.serving.load(Ordering::Acquire) == ticket {
                return Turn { turnstile: self };
            }
            notified.await;
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.turnstile.serving.fetch_add(1, Ordering::AcqRel);
        self.turnstile.notify.notify_waiters();
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Turnstile;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_should_serve_tickets_in_order() {
        let turnstile = Arc::new(Turnstile::default());
        let log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let started = Arc::new(AtomicUsize::new(0));

        let tickets: Vec<u64> = (0..16).map(|_| turnstile.ticket()).collect();
        let mut tasks = Vec::new();
        for ticket in tickets.into_iter().rev() {
            let turnstile = Arc::clone(&turnstile);
            let log = Arc::clone(&log);
            let started = Arc::clone(&started);
            tasks.push(tokio::spawn(async move {
                started.fetch_add(1, Ordering::SeqCst);
                let _turn = turnstile.wait(ticket).await;
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                log.lock().await.push(ticket);
            }));
        }
        for task in tasks {
            task.await.expect("task");
        }
        assert_eq!(*log.lock().await, (0..16).collect::<Vec<u64>>());
    }

    #[tokio::test]
    async fn test_dropped_turn_releases_next_ticket() {
        let turnstile = Turnstile::default();
        let first = turnstile.ticket();
        let second = turnstile.ticket();
        drop(turnstile.wait(first).await);
        let _turn = turnstile.wait(second).await;
    }
}
