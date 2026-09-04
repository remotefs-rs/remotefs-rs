mod option;

use std::path::Path;

use remotefs::RemoteFs;

pub use self::option::MountOption;
use crate::driver::Driver;

/// A struct to mount the filesystem.
#[derive(Debug)]
pub struct Mount<T>
where
    T: RemoteFs + Sync + Send + 'static,
{
    #[cfg(unix)]
    session: Option<fuser::Session<Driver<T>>>,
    #[cfg(windows)]
    mountpoint: widestring::U16CString,
    #[cfg(windows)]
    driver: Driver<T>,
}

impl<T> Mount<T>
where
    T: RemoteFs + Sync + Send + 'static,
{
    /// Mount the filesystem implemented by `Driver` to the provided mountpoint.
    ///
    /// You can specify the mount options using the `options` parameter as an array of [`MountOption`].
    #[expect(
        clippy::self_named_constructors,
        reason = "`Mount::mount` reads more naturally than `Mount::new` for a mount API"
    )]
    #[cfg(unix)]
    pub fn mount(
        remote: T,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        let driver = Driver::new(remote, options.to_vec());

        let options = option::into_fuser_config(&driver.options());

        Ok(Self {
            session: Some(fuser::Session::new(driver, mountpoint, &options)?),
        })
    }

    /// Mount the filesystem implemented by `Driver` to the provided mountpoint.
    ///
    /// You can specify the mount options using the `options` parameter as an array of [`MountOption`].
    #[cfg(windows)]
    #[expect(
        clippy::self_named_constructors,
        reason = "`Mount::mount` reads more naturally than `Mount::new` for a mount API"
    )]
    pub fn mount(
        remote: T,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        use widestring::U16CString;

        let driver = Driver::new(remote, options.to_vec());
        dokan::init();

        let mountpoint =
            U16CString::from_os_str(std::ffi::OsStr::new(mountpoint)).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid mountpoint")
            })?;

        Ok(Self { mountpoint, driver })
    }

    /// Run the filesystem event loop.
    ///
    /// This function will block the current thread.
    pub fn run(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        self.session
            .take()
            .ok_or_else(|| std::io::Error::other("filesystem session has already been started"))?
            .run()?;

        #[cfg(windows)]
        {
            let options = MountOption::into_dokan_options(&self.driver.options);
            // For reference <https://github.com/dokan-dev/dokan-rust/blob/master/dokan/examples/memfs/main.rs>
            let mut mounter =
                dokan::FileSystemMounter::new(&self.driver, &self.mountpoint, &options);
            mounter.mount().map_err(std::io::Error::other)?;
        }

        Ok(())
    }

    /// Get a handle to unmount the filesystem.
    ///
    /// To umount see [`Unmount::unmount`].
    pub fn unmounter(&mut self) -> Unmount {
        Unmount {
            #[cfg(unix)]
            umount: self
                .session
                .as_mut()
                .expect("filesystem session has already been started")
                .unmount_callable(),
            #[cfg(windows)]
            mountpoint: self.mountpoint.clone(),
        }
    }
}

/// A thread-safe handle to unmount the filesystem.
#[derive(Debug)]
pub struct Unmount {
    #[cfg(unix)]
    umount: fuser::SessionUnmounter,
    #[cfg(windows)]
    mountpoint: widestring::U16CString,
}

impl Unmount {
    /// Unmount the filesystem.
    pub fn unmount(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        self.umount.unmount()?;

        #[cfg(windows)]
        if !dokan::unmount(&self.mountpoint) {
            return Err(std::io::Error::other("Failed to unmount"));
        }

        Ok(())
    }
}
