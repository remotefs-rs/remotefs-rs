mod option;

use std::path::Path;

use remotefs::RemoteFs;

pub use self::option::MountOption;
use crate::driver::Driver;

/// A struct to mount the filesystem.
pub struct Mount<'a, T>
where
    T: RemoteFs + Sync + 'a,
{
    #[cfg(unix)]
    session: fuser::Session<Driver<T>>,
    #[cfg(windows)]
    #[allow(dead_code)]
    file_system: dokan::FileSystem<'a, 'a, Driver<T>>,
    #[cfg(windows)]
    mountpoint: widestring::U16CString,
    #[cfg(unix)]
    marker: std::marker::PhantomData<&'a u8>,
}

impl<'a, T> Mount<'a, T>
where
    T: RemoteFs + Sync + 'a,
{
    /// Mount the filesystem implemented by  [`Driver`] to the provided mountpoint.
    ///
    /// You can specify the mount options using the `options` parameter.
    #[allow(clippy::self_named_constructors)]
    #[cfg(unix)]
    pub fn mount(
        remote: T,
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
            session: fuser::Session::new(driver, mountpoint, &options)?,
            marker: std::marker::PhantomData,
        })
    }

    /// Mount the filesystem implemented by  [`Driver`] to the provided mountpoint.
    ///
    /// You can specify the mount options using the `options` parameter.
    #[cfg(windows)]
    #[allow(clippy::self_named_constructors)]
    pub fn mount(
        remote: T,
        mountpoint: &Path,
        options: &[MountOption],
    ) -> Result<Self, std::io::Error> {
        use widestring::U16CString;

        let driver = Driver::new(remote, options.to_vec());
        dokan::init();

        //let options = driver
        //    .options
        //    .iter()
        //    .flat_map(|opt| opt.try_into())
        //    .collect::<Vec<_>>();

        let mountpoint =
            U16CString::from_os_str(std::ffi::OsStr::new(mountpoint)).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid mountpoint")
            })?;

        // For reference <https://github.com/dokan-dev/dokan-rust/blob/master/dokan/examples/memfs/main.rs>
        let mut mounter = dokan::FileSystemMounter::new(&driver, &mountpoint, todo!());
        let fs = mounter
            .mount()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(Self {
            file_system: fs,
            mountpoint,
        })
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
            #[cfg(windows)]
            mountpoint: self.mountpoint.clone(),
        }
    }
}

/// A thread-safe handle to unmount the filesystem.
pub struct Umount {
    #[cfg(unix)]
    umount: fuser::SessionUnmounter,
    #[cfg(windows)]
    mountpoint: widestring::U16CString,
}

impl Umount {
    /// Unmount the filesystem.
    pub fn umount(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        self.umount.unmount()?;

        #[cfg(windows)]
        if !dokan::unmount(&self.mountpoint) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to unmount",
            ));
        }

        Ok(())
    }
}
