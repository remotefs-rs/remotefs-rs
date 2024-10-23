use std::path::Path;

use fuser::{MountOption, Session, SessionUnmounter};

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
    pub fn mount(
        driver: Driver,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            #[cfg(unix)]
            session: Session::new(driver, mountpoint, options)?,
        })
    }

    /// Run the filesystem event loop.
    ///
    /// This function will block the current thread.
    pub fn run(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        self.session.run()
    }

    /// Get a handle to unmount the filesystem.
    ///
    /// To umount see [`Umount::umount`].
    pub fn unmounter(&mut self) -> Umount {
        #[cfg(unix)]
        Umount {
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
        self.umount.unmount()
    }
}
