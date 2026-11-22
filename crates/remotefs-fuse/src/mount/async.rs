//! Asynchronous mount lifecycle over an [`AsyncRemoteFs`] client.

use std::path::Path;
use std::sync::{Arc, Mutex};

use remotefs::AsyncRemoteFs;
use tokio::runtime::Handle;
use tokio::sync::RwLock;

use super::{MountOption, Unmount};
use crate::driver::AsyncDriver;

/// A mounted [`AsyncRemoteFs`] client.
///
/// Requests are served by native async drivers: on Unix every FUSE request
/// becomes a task on the runtime; on Windows every Dokany callback awaits the
/// remote inside a single `block_on`. No blocking adapter is involved.
/// [`AsyncMount::mount`] connects the client and [`AsyncMount::run`] disconnects
/// it once the filesystem is unmounted.
pub struct AsyncMount<T>
where
    T: AsyncRemoteFs + 'static,
{
    #[cfg(unix)]
    session: Option<fuser::Session<AsyncDriver<T>>>,
    #[cfg(windows)]
    mountpoint: widestring::U16CString,
    #[cfg(windows)]
    driver: AsyncDriver<T>,
    remote: Arc<RwLock<T>>,
}

impl<T> std::fmt::Debug for AsyncMount<T>
where
    T: AsyncRemoteFs + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncMount").finish_non_exhaustive()
    }
}

impl<T> AsyncMount<T>
where
    T: AsyncRemoteFs + 'static,
{
    /// Connect `remote` and mount it at `mountpoint`.
    ///
    /// This function must be called inside a Tokio runtime. The runtime must
    /// outlive the mount because filesystem requests are driven on it.
    pub async fn mount(
        mut remote: T,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        let handle = Handle::current();
        remote.connect().await.map_err(std::io::Error::from)?;
        let remote = Arc::new(RwLock::new(remote));
        let driver = AsyncDriver::new(Arc::clone(&remote), options.to_vec(), handle);

        #[cfg(unix)]
        {
            let config = super::option::into_fuser_config(options);
            let mountpoint = mountpoint.to_path_buf();
            let session_result = tokio::task::spawn_blocking(move || {
                fuser::Session::new(driver, &mountpoint, &config)
                    .map_err(|err| super::mount_error(&mountpoint, err))
            })
            .await
            .map_err(join_error)?;
            let session = match session_result {
                Ok(session) => session,
                Err(err) => {
                    let _ = remote.write().await.disconnect().await;
                    return Err(err);
                }
            };
            Ok(Self {
                session: Some(session),
                remote,
            })
        }

        #[cfg(windows)]
        {
            dokan::init();
            let mountpoint =
                match widestring::U16CString::from_os_str(std::ffi::OsStr::new(mountpoint)) {
                    Ok(mountpoint) => mountpoint,
                    Err(_) => {
                        let _ = remote.write().await.disconnect().await;
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "Invalid mountpoint",
                        ));
                    }
                };
            Ok(Self {
                mountpoint,
                driver,
                remote,
            })
        }
    }

    /// Return a handle to unmount from another task or signal handler.
    /// Call this before [`AsyncMount::run`].
    pub fn unmounter(&mut self) -> AsyncUnmount {
        AsyncUnmount {
            inner: Arc::new(Mutex::new(Unmount {
                #[cfg(unix)]
                umount: self
                    .session
                    .as_mut()
                    .expect("filesystem session has already been started")
                    .unmount_callable(),
                #[cfg(windows)]
                mountpoint: self.mountpoint.clone(),
            })),
        }
    }

    /// Run the filesystem event loop until unmounted, then disconnect the client.
    pub async fn run(self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        {
            let mut session = self.session;
            let session = session.take().ok_or_else(|| {
                std::io::Error::other("filesystem session has already been started")
            })?;
            let task = spawn_blocking_with_disconnect(
                Handle::current(),
                Arc::clone(&self.remote),
                move || session.run(),
            );
            task.await.map_err(join_error)?
        }

        #[cfg(windows)]
        {
            let Self {
                mountpoint,
                driver,
                remote,
            } = self;
            let task = spawn_blocking_with_disconnect(Handle::current(), remote, move || {
                let options = MountOption::into_dokan_options(driver.options());
                let mut mounter = dokan::FileSystemMounter::new(&driver, &mountpoint, &options);
                mounter.mount().map(|_| ()).map_err(std::io::Error::other)
            });
            task.await.map_err(join_error)?
        }
    }
}

fn spawn_blocking_with_disconnect<T, F>(
    handle: Handle,
    remote: Arc<RwLock<T>>,
    operation: F,
) -> tokio::task::JoinHandle<Result<(), std::io::Error>>
where
    T: AsyncRemoteFs + 'static,
    F: FnOnce() -> Result<(), std::io::Error> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let loop_result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
            Ok(result) => result,
            Err(_) => Err(std::io::Error::other("filesystem operation panicked")),
        };
        let disconnect_result = handle.block_on(async {
            remote
                .write()
                .await
                .disconnect()
                .await
                .map_err(std::io::Error::from)
        });
        loop_result.and(disconnect_result)
    })
}

/// A cloneable, thread-safe handle to unmount an [`AsyncMount`].
#[derive(Clone, Debug)]
pub struct AsyncUnmount {
    inner: Arc<Mutex<Unmount>>,
}

impl AsyncUnmount {
    /// Unmount the filesystem on Tokio's blocking pool.
    pub async fn unmount(&self) -> Result<(), std::io::Error> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let mut unmount = inner
                .lock()
                .map_err(|_| std::io::Error::other("unmount handle lock poisoned"))?;
            unmount.unmount()
        })
        .await
        .map_err(join_error)?
    }
}

fn join_error(err: tokio::task::JoinError) -> std::io::Error {
    std::io::Error::other(format!("filesystem task failed: {err}"))
}

#[cfg(test)]
mod test {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use remotefs::AsyncRemoteFs;
    use remotefs::adapters::r#async::Unblock;
    use remotefs::fs::UnixPex;
    use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};

    use super::{AsyncMount, AsyncUnmount, spawn_blocking_with_disconnect};

    fn memory_fs() -> MemoryFs {
        let tree = Tree::new(node!(
            PathBuf::from("/"),
            Inode::dir(0, 0, UnixPex::from(0o755)),
        ));
        MemoryFs::new(tree)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_mount_should_connect_then_fail_on_missing_mountpoint() {
        let mountpoint = Path::new("/nonexistent/remotefs-fuse/mountpoint");
        let err = AsyncMount::mount(Unblock::new(memory_fs()), mountpoint, &[])
            .await
            .expect_err("mounting on a missing directory must fail");
        let message = err.to_string();
        assert!(message.contains("failed to mount filesystem"));
        assert!(message.contains(&mountpoint.display().to_string()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_dropped_run_handle_still_disconnects_remote() {
        let mut client = Unblock::new(memory_fs());
        client.connect().await.expect("connect");
        let remote = Arc::new(tokio::sync::RwLock::new(client));
        let task = spawn_blocking_with_disconnect(
            tokio::runtime::Handle::current(),
            Arc::clone(&remote),
            || {
                std::thread::sleep(Duration::from_millis(20));
                Ok(())
            },
        );
        drop(task);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !remote.read().await.is_connected() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropped run handle must not skip disconnect");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_panicking_run_operation_still_disconnects_remote() {
        let mut client = Unblock::new(memory_fs());
        client.connect().await.expect("connect");
        let remote = Arc::new(tokio::sync::RwLock::new(client));
        let task = spawn_blocking_with_disconnect(
            tokio::runtime::Handle::current(),
            Arc::clone(&remote),
            || panic!("run failed"),
        );
        let _ = task.await;

        assert!(!remote.read().await.is_connected());
    }

    #[test]
    fn test_async_types_are_send_sync_and_unmount_is_clone() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_clone<T: Clone>() {}
        assert_send_sync::<AsyncMount<Unblock<MemoryFs>>>();
        assert_send_sync::<AsyncUnmount>();
        assert_clone::<AsyncUnmount>();
    }
}
