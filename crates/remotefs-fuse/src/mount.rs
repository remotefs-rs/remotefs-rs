mod option;

use std::path::Path;

#[cfg(unix)]
use fuser::{Session, SessionUnmounter};

pub use self::option::MountOption;
use crate::Driver;

/// A struct to mount the filesystem.
pub struct Mount {
    #[cfg(unix)]
    session: Session<Driver>,
}

impl Mount {
    /// Mount the filesystem implemented by  [`Driver`] to the provided mountpoint.
    ///
    /// You can specify the mount options using the `options` parameter.
    #[allow(clippy::self_named_constructors)]
    #[cfg(unix)]
    pub fn mount(driver: Driver, mountpoint: &Path) -> Result<Self, std::io::Error> {
        let options = driver
            .options
            .iter()
            .flat_map(|opt| opt.try_into())
            .collect::<Vec<_>>();

        Ok(Self {
            session: Session::new(driver, mountpoint, &options)?,
        })
    }

    #[cfg(windows)]
    pub fn mount(driver: Driver, mountpoint: &Path) -> Result<Self, std::io::Error> {
        todo!()
    }

    /// Run the filesystem event loop.
    ///
    /// This function will block the current thread.
    pub fn run(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        self.session.run()?;

        Ok(())
    }

    /// Get a handle to unmount the filesystem.
    ///
    /// To umount see [`Umount::umount`].
    pub fn unmounter(&mut self) -> Umount {
        Umount {
            #[cfg(unix)]
            umount: self.session.unmount_callable(),
        }
    }
}

/// A thread-safe handle to unmount the filesystem.
pub struct Umount {
    #[cfg(unix)]
    umount: SessionUnmounter,
}

impl Umount {
    /// Unmount the filesystem.
    pub fn umount(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        self.umount.unmount()?;

        Ok(())
    }
}
