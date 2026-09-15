mod option;

#[cfg(feature = "tokio")]
#[cfg_attr(docsrs, doc(cfg(feature = "tokio")))]
mod r#async;

use std::path::Path;

use remotefs::RemoteFs;

#[cfg(feature = "tokio")]
pub use self::r#async::{AsyncMount, AsyncUnmount};
pub use self::option::MountOption;
use crate::driver::Driver;

#[cfg(unix)]
#[derive(Debug)]
struct MountFailure {
    mountpoint: std::path::PathBuf,
    source: std::io::Error,
}

#[cfg(unix)]
impl std::fmt::Display for MountFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(target_os = "macos")]
        if self.source.kind() == std::io::ErrorKind::Other
            && self.source.raw_os_error().is_none()
            && self.source.to_string() == "Unspecified Error"
        {
            return write!(
                f,
                "failed to mount filesystem at {mountpoint}: macFUSE returned an unspecified \
                 error; verify that the installed macFUSE version supports this macOS version \
                 and that its extension is enabled",
                mountpoint = self.mountpoint.display(),
            );
        }

        write!(
            f,
            "failed to mount filesystem at {mountpoint}: {source}",
            mountpoint = self.mountpoint.display(),
            source = self.source,
        )
    }
}

#[cfg(unix)]
impl std::error::Error for MountFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(unix)]
fn mount_error(mountpoint: &Path, source: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        source.kind(),
        MountFailure {
            mountpoint: mountpoint.to_path_buf(),
            source,
        },
    )
}

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
            session: Some(
                fuser::Session::new(driver, mountpoint, &options)
                    .map_err(|err| mount_error(mountpoint, err))?,
            ),
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
    pub(super) umount: fuser::SessionUnmounter,
    #[cfg(windows)]
    pub(super) mountpoint: widestring::U16CString,
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

#[cfg(all(test, unix))]
mod test {
    #[test]
    fn test_mount_error_should_preserve_kind_and_source() {
        let err = super::mount_error(
            std::path::Path::new("/tmp/example"),
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );

        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let context = err.get_ref().expect("mount error should contain context");
        assert_eq!(
            context
                .source()
                .expect("mount context should retain its source")
                .to_string(),
            "permission denied"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_mount_error_should_explain_unspecified_macfuse_failure() {
        let err = super::mount_error(
            std::path::Path::new("/tmp/example"),
            std::io::Error::other("Unspecified Error"),
        );

        let message = err.to_string();
        assert!(message.contains("macFUSE returned an unspecified error"));
        assert!(message.contains("supports this macOS version"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_mount_error_should_not_add_macfuse_hint_to_other_failures() {
        let err = super::mount_error(
            std::path::Path::new("/tmp/example"),
            std::io::Error::other("another mount failure"),
        );

        let message = err.to_string();
        assert!(message.contains("another mount failure"));
        assert!(!message.contains("macFUSE returned an unspecified error"));
    }
}
