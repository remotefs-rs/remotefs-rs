mod error;
#[cfg(unix)]
#[cfg_attr(docsrs, doc(cfg(unix)))]
mod unix;

use remotefs::RemoteFs;

pub use self::error::{DriverError, DriverResult};
use crate::MountOption;

/// Remote Filesystem Driver
///
/// This driver takes a instance which implements the [`RemoteFs`] trait and mounts it to a local directory.
///
/// The driver will use the [`fuser`](https://crates.io/crates/fuser) crate to mount the filesystem, on Unix systems, while
/// it will use [dokan](https://crates.io/crates/dokan) on Windows.
pub struct Driver {
    /// Inode database
    #[cfg(unix)]
    database: unix::InodeDb,
    /// File handle database
    #[cfg(unix)]
    file_handlers: unix::FileHandlersDb,
    /// Mount options
    pub(crate) options: Vec<MountOption>,
    /// [`RemoteFs`] instance
    remote: Box<dyn RemoteFs>,
}

impl Driver {
    /// Create a new instance of the [`Driver`] providing a instance which implements the [`RemoteFs`] trait.
    ///
    /// The [`RemoteFs`] instance must be boxed.
    ///
    /// # Arguments
    ///
    /// * `remote` - The instance which implements the [`RemoteFs`] trait.
    /// * `options` - The mount options.
    pub fn new(remote: Box<dyn RemoteFs>, options: Vec<MountOption>) -> Self {
        Self {
            #[cfg(unix)]
            database: unix::InodeDb::load(),
            #[cfg(unix)]
            file_handlers: unix::FileHandlersDb::default(),
            options,
            remote,
        }
    }

    /// Get the specified uid from the mount options.
    fn uid(&self) -> Option<u32> {
        self.options.iter().find_map(|opt| match opt {
            MountOption::Uid(uid) => Some(*uid),
            _ => None,
        })
    }

    /// Get the specified gid from the mount options.
    fn gid(&self) -> Option<u32> {
        self.options.iter().find_map(|opt| match opt {
            MountOption::Gid(gid) => Some(*gid),
            _ => None,
        })
    }
}
