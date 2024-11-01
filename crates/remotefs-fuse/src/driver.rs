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
pub struct Driver<T: RemoteFs> {
    /// Inode database
    #[cfg(unix)]
    database: unix::InodeDb,
    /// File handle database
    #[cfg(unix)]
    file_handlers: unix::FileHandlersDb,
    /// Mount options
    pub(crate) options: Vec<MountOption>,
    #[cfg(unix)]
    /// [`RemoteFs`] instance
    remote: T,
    #[cfg(windows)]
    /// [`RemoteFs`] instance usable as `Sync` in immutable references
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
            database: unix::InodeDb::load(),
            #[cfg(unix)]
            file_handlers: unix::FileHandlersDb::default(),
            options,
            #[cfg(unix)]
            remote,
            #[cfg(windows)]
            remote: std::sync::Arc::new(std::sync::Mutex::new(remote)),
            #[cfg(windows)]
            file_handlers: dashmap::DashMap::new(),
        }
    }
}
