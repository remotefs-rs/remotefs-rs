mod error;
#[cfg(target_family = "unix")]
#[cfg_attr(docsrs, doc(cfg(target_family = "unix")))]
mod unix;

use std::path::{Path, PathBuf};

use remotefs::RemoteFs;

pub use self::error::{DriverError, DriverResult};

/// Remote Filesystem Driver
///
/// This driver takes a instance which implements the [`RemoteFs`] trait and mounts it to a local directory.
///
/// The driver will use the [`fuser`](https://crates.io/crates/fuser) crate to mount the filesystem, on Unix systems, while
/// it will use [dokan](https://crates.io/crates/dokan) on Windows.
pub struct Driver {
    data_dir: PathBuf,
    #[cfg(target_family = "unix")]
    database: unix::InodeDb,
    remote: Box<dyn RemoteFs>,
}

impl Driver {
    /// Create a new instance of the [`Driver`] providing a instance which implements the [`RemoteFs`] trait.
    ///
    /// The [`RemoteFs`] instance must be boxed.
    ///
    /// # Arguments
    ///
    /// * `data_dir` - A directory where inodes will be mapped.
    /// * `remote` - The instance which implements the [`RemoteFs`] trait.
    pub fn new(data_dir: &Path, remote: Box<dyn RemoteFs>) -> DriverResult<Self> {
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            #[cfg(target_family = "unix")]
            database: unix::InodeDb::load(&data_dir.join("inodes.json"))?,
            remote,
        })
    }
}
