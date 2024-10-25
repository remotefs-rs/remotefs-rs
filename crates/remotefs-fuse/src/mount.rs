mod option;

use std::path::Path;

#[cfg(unix)]
use fuser::{Session, SessionUnmounter};
use remotefs::RemoteFs;

pub use self::option::MountOption;
use crate::driver::Driver;

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
    pub fn mount(
        remote: Box<dyn RemoteFs>,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        let driver = Driver::new(remote, options.to_vec());

        let options = driver
            .options
            .iter()
            .flat_map(|opt| opt.try_into())
            .collect::<Vec<_>>();

        Ok(Self {
            session: Session::new(driver, mountpoint, &options)?,
        })
    }

    /// Mount the filesystem implemented by  [`Driver`] to the provided mountpoint.
    ///
    /// You can specify the mount options using the `options` parameter.
    #[cfg(windows)]
    #[allow(clippy::self_named_constructors)]
    pub fn mount(
        mut driver: Driver,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        driver.options = options.to_vec();

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
