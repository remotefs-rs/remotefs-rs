use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Runs a backend keepalive operation at a fixed interval until stopped or
/// the operation reports that the connection is no longer usable.
pub(super) struct ServerKeepalive {
    state: Arc<(Mutex<WorkerState>, Condvar)>,
    shutdown: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct WorkerState {
    active: bool,
    cancelled: bool,
}

impl ServerKeepalive {
    pub(super) fn spawn(
        interval: Duration,
        shutdown: impl FnOnce() + Send + 'static,
        mut send: impl FnMut() -> bool + Send + 'static,
    ) -> Option<Self> {
        if interval.is_zero() {
            return None;
        }

        let state = Arc::new((
            Mutex::new(WorkerState {
                active: false,
                cancelled: false,
            }),
            Condvar::new(),
        ));
        let worker_state = Arc::clone(&state);
        let worker = std::thread::spawn(move || {
            let (state, wake) = &*worker_state;
            let mut state = state.lock().expect("server keepalive state was poisoned");
            loop {
                let result = wake
                    .wait_timeout_while(state, interval, |state| !state.cancelled)
                    .expect("server keepalive state was poisoned");
                state = result.0;
                if state.cancelled {
                    break;
                }
                state.active = true;
                drop(state);
                let keep_running = send();
                state = worker_state
                    .0
                    .lock()
                    .expect("server keepalive state was poisoned");
                state.active = false;
                wake.notify_all();
                if !keep_running {
                    break;
                }
            }
        });

        Some(Self {
            state,
            shutdown: Mutex::new(Some(Box::new(shutdown))),
            worker: Mutex::new(Some(worker)),
        })
    }

    /// Stops the worker, returning whether its transport had to be interrupted.
    pub(super) fn stop(&self) -> bool {
        let (state, wake) = &*self.state;
        let mut state = state.lock().expect("server keepalive state was poisoned");
        state.cancelled = true;
        wake.notify_all();
        if state.active {
            state = wake
                .wait_timeout_while(state, Duration::from_millis(100), |state| state.active)
                .expect("server keepalive state was poisoned")
                .0;
        }
        let interrupted = state.active;
        drop(state);
        let shutdown = self
            .shutdown
            .lock()
            .expect("server keepalive shutdown was poisoned")
            .take();
        if interrupted && let Some(shutdown) = shutdown {
            shutdown();
        }
        if let Some(worker) = self
            .worker
            .lock()
            .expect("server keepalive worker was poisoned")
            .take()
        {
            let _ = worker.join();
        }
        interrupted
    }
}

impl Drop for ServerKeepalive {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn should_run_and_stop_server_keepalive() {
        let (sender, receiver) = mpsc::channel();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let keepalive = ServerKeepalive::spawn(
            Duration::from_millis(1),
            move || {
                shutdown_sender
                    .send(())
                    .expect("failed to report forced shutdown");
            },
            move || sender.send(()).is_ok(),
        )
        .expect("keepalive should be enabled");

        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("keepalive did not run");
        assert!(!keepalive.stop());
        assert!(shutdown_receiver.try_recv().is_err());
        assert!(ServerKeepalive::spawn(Duration::ZERO, || {}, || true).is_none());
    }

    #[test]
    fn should_interrupt_active_keepalive_when_stopped() {
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel::<()>();
        let keepalive = ServerKeepalive::spawn(
            Duration::from_millis(1),
            move || {
                let _ = release_sender.send(());
            },
            move || {
                entered_sender
                    .send(())
                    .expect("failed to report active keepalive");
                let _ = release_receiver.recv();
                false
            },
        )
        .expect("keepalive should be enabled");
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("keepalive did not run");

        assert!(keepalive.stop());
    }
}
