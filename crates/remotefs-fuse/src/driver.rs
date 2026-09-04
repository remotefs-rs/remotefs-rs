#[cfg(unix)]
#[cfg_attr(docsrs, doc(cfg(unix)))]
mod unix;
#[cfg(windows)]
#[cfg_attr(docsrs, doc(cfg(windows)))]
mod windows;

use remotefs::RemoteFs;

use crate::MountOption;

/// Remote Filesystem Driver
///
/// This driver takes a instance which implements the [`RemoteFs`] trait and mounts it to a local directory.
///
/// The driver will use the [`fuser`](https://crates.io/crates/fuser) crate to mount the filesystem, on Unix systems, while
/// it will use [dokan](https://crates.io/crates/dokan) on Windows.
#[derive(Debug)]
pub struct Driver<T: RemoteFs> {
    /// Unix filesystem state.
    #[cfg(unix)]
    inner: std::sync::Mutex<unix::DriverInner<T>>,
    /// Mount options.
    #[cfg(windows)]
    pub(crate) options: Vec<MountOption>,
    #[cfg(windows)]
    /// [`RemoteFs`] instance usable as `Sync` in immutable references.
    remote: std::sync::Arc<std::sync::Mutex<T>>,
    #[cfg(windows)]
    /// [`windows::DirEntry`] foor directory
    file_handlers:
        dashmap::DashMap<widestring::U16CString, std::sync::Arc<std::sync::RwLock<windows::Stat>>>,
}

impl<T> Driver<T>
where
    T: RemoteFs,
{
    /// Create a new instance of the [`Driver`] providing a instance which implements the [`RemoteFs`] trait.
    ///
    /// The [`RemoteFs`] instance must be boxed.
    ///
    /// # Arguments
    ///
    /// * `remote` - The instance which implements the [`RemoteFs`] trait.
    /// * `options` - The mount options.
    pub fn new(remote: T, options: Vec<MountOption>) -> Self {
        Self {
            #[cfg(unix)]
            inner: std::sync::Mutex::new(unix::DriverInner::new(remote, options)),
            #[cfg(windows)]
            options,
            #[cfg(windows)]
            remote: std::sync::Arc::new(std::sync::Mutex::new(remote)),
            #[cfg(windows)]
            file_handlers: dashmap::DashMap::new(),
        }
    }

    #[cfg(unix)]
    pub(crate) fn with_inner<R>(
        &self,
        operation: impl FnOnce(&mut unix::DriverInner<T>) -> R,
    ) -> R {
        let mut inner = self.inner.lock().expect("Unix driver state lock poisoned");
        operation(&mut inner)
    }

    #[cfg(unix)]
    pub(crate) fn options(&self) -> Vec<MountOption> {
        self.with_inner(|inner| inner.options.clone())
    }
}
