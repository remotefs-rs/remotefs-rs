/// Mount options for mounting a FUSE filesystem
///
/// Some of them are *nix-specific, and may not be available on other platforms, while other
/// are for Windows only.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub enum MountOption {
    /* nix driver */
    /// Treat all files as if they are owned by the given user.
    /// This flag can be useful when mounting for instance sftp volumes,
    /// where the uid/gid of the files may be different from the user mounting the filesystem.
    /// This doesn't change the ownership of the files, but allows the user to access them.
    /// Of course, if the signed in user doesn't have the right permissions, the files will still be inaccessible.
    Uid(u32),
    /// Treat all files as if they are owned by the given user.
    /// This flag can be useful when mounting for instance sftp volumes,
    /// where the uid/gid of the files may be different from the user mounting the filesystem.
    /// This doesn't change the ownership of the files, but allows the user to access them.
    /// Of course, if the signed in user doesn't have the right permissions, the files will still be inaccessible.
    Gid(u32),
    /// Set the default file mode in case the filesystem doesn't provide one
    /// If not set, the default is 0755
    DefaultMode(u32),
    /* fuser */
    /// Set the name of the source in mtab
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    FSName(String),
    /// Set the filesystem subtype in mtab
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Subtype(String),
    /// Allows passing an option which is not otherwise supported in these enums
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Custom(String),
    /// Allow all users to access files on this filesystem. By default access is restricted to the
    /// user who mounted it
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    AllowOther,
    /// Allow the root user to access this filesystem, in addition to the user who mounted it
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    AllowRoot,
    /// Automatically unmount when the mounting process exits
    ///
    /// `AutoUnmount` requires `AllowOther` or `AllowRoot`. If `AutoUnmount` is set and neither `Allow...` is set, the FUSE configuration must permit `allow_other`, otherwise mounting will fail.
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    AutoUnmount,
    /// Enable permission checking in the kernel
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    DefaultPermissions,

    /// Enable special character and block devices
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Dev,
    /// Disable special character and block devices
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    NoDev,
    /// Honor set-user-id and set-groupd-id bits on files
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Suid,
    /// Don't honor set-user-id and set-groupd-id bits on files
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    NoSuid,
    /// Read-only filesystem
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    RO,
    /// Read-write filesystem
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    RW,
    /// Allow execution of binaries
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Exec,
    /// Don't allow execution of binaries
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    NoExec,
    /// Support inode access time
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Atime,
    /// Don't update inode access time
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    NoAtime,
    /// All modifications to directories will be done synchronously
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    DirSync,
    /// All I/O will be done synchronously
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Sync,
    /// All I/O will be done asynchronously
    #[cfg(unix)]
    #[cfg_attr(docsrs, doc(cfg(unix)))]
    Async,

    // dokany
    /// Only use a single thread to process events. This is highly not recommended as can easily create a bottleneck.
    #[cfg(windows)]
    #[cfg_attr(docsrs, doc(cfg(windows)))]
    SingleThread,
    /// Controls behavior of the volume.
    #[cfg(windows)]
    #[cfg_attr(docsrs, doc(cfg(windows)))]
    Flags(u32),
    /// Max timeout of each request before Dokan gives up to wait events to complete.
    /// Timeout request is a sign that the userland implementation is no longer able to properly manage requests in time.
    /// The driver will therefore unmount the device when a timeout trigger in order to keep the system stable.
    ///
    /// If zero, defaults to 15 seconds.
    #[cfg(windows)]
    #[cfg_attr(docsrs, doc(cfg(windows)))]
    Timeout(std::time::Duration),
    /// Allocation Unit Size of the volume. This will affect the file size.
    #[cfg(windows)]
    #[cfg_attr(docsrs, doc(cfg(windows)))]
    AllocationUnitSize(u32),
    /// Sector Size of the volume. This will affect the file size.
    #[cfg(windows)]
    #[cfg_attr(docsrs, doc(cfg(windows)))]
    SectorSize(u32),
}

#[cfg(unix)]
#[cfg_attr(docsrs, doc(cfg(unix)))]
impl TryFrom<&MountOption> for fuser::MountOption {
    type Error = &'static str;

    fn try_from(value: &MountOption) -> Result<Self, Self::Error> {
        Ok(match value {
            MountOption::FSName(name) => fuser::MountOption::FSName(name.clone()),
            MountOption::Subtype(name) => fuser::MountOption::Subtype(name.clone()),
            MountOption::Custom(name) => fuser::MountOption::CUSTOM(name.clone()),
            MountOption::AllowOther => fuser::MountOption::AllowOther,
            MountOption::AllowRoot => fuser::MountOption::AllowRoot,
            MountOption::AutoUnmount => fuser::MountOption::AutoUnmount,
            MountOption::DefaultPermissions => fuser::MountOption::DefaultPermissions,
            MountOption::Dev => fuser::MountOption::Dev,
            MountOption::NoDev => fuser::MountOption::NoDev,
            MountOption::Suid => fuser::MountOption::Suid,
            MountOption::NoSuid => fuser::MountOption::NoSuid,
            MountOption::RO => fuser::MountOption::RO,
            MountOption::RW => fuser::MountOption::RW,
            MountOption::Exec => fuser::MountOption::Exec,
            MountOption::NoExec => fuser::MountOption::NoExec,
            MountOption::Atime => fuser::MountOption::Atime,
            MountOption::NoAtime => fuser::MountOption::NoAtime,
            MountOption::DirSync => fuser::MountOption::DirSync,
            MountOption::Sync => fuser::MountOption::Sync,
            MountOption::Async => fuser::MountOption::Async,
            _ => return Err("Unsupported mount option"),
        })
    }
}

#[cfg(windows)]
#[cfg_attr(docsrs, doc(cfg(windows)))]
impl MountOption {
    pub fn into_dokan_options(options: &[MountOption]) -> dokan::MountOptions<'_> {
        let mut dokan_options = dokan::MountOptions::default();

        for option in options {
            match option {
                MountOption::SingleThread => dokan_options.single_thread = true,
                MountOption::Flags(flags) => {
                    dokan_options.flags = dokan::MountFlags::from_bits_truncate(*flags)
                }
                MountOption::Timeout(timeout) => dokan_options.timeout = *timeout,
                MountOption::AllocationUnitSize(size) => dokan_options.allocation_unit_size = *size,
                MountOption::SectorSize(size) => dokan_options.sector_size = *size,
                _ => {}
            }
        }

        dokan_options
    }
}
