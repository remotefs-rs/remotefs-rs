use std::str::FromStr;

/// Mount options for mounting a FUSE filesystem
///
/// Some of them are *nix-specific, and may not be available on other platforms, while other
/// are for Windows only.
///
/// [`MountOption`] implements [`FromStr`] with the syntax `key[=value]` for all options.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub enum MountOption {
    /* nix driver */
    #[cfg(unix)]
    /// Treat all files as if they are owned by the given user.
    /// This flag can be useful when mounting for instance sftp volumes,
    /// where the uid/gid of the files may be different from the user mounting the filesystem.
    /// This doesn't change the ownership of the files, but allows the user to access them.
    /// Of course, if the signed in user doesn't have the right permissions, the files will still be inaccessible.
    Uid(u32),
    #[cfg(unix)]
    /// Treat all files as if they are owned by the given user.
    /// This flag can be useful when mounting for instance sftp volumes,
    /// where the uid/gid of the files may be different from the user mounting the filesystem.
    /// This doesn't change the ownership of the files, but allows the user to access them.
    /// Of course, if the signed in user doesn't have the right permissions, the files will still be inaccessible.
    Gid(u32),
    #[cfg(unix)]
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
            }
        }

        dokan_options
    }
}

impl FromStr for MountOption {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (option, value) = match s.find('=') {
            Some(index) => (
                (s[..index]).to_ascii_lowercase().to_string(),
                Some(&s[index + 1..]),
            ),
            None => (s.to_ascii_lowercase().to_string(), None),
        };

        match (option.as_str(), value) {
            #[cfg(unix)]
            ("uid", Some(value)) => {
                let value = value
                    .parse()
                    .map_err(|e| format!("Invalid uid value: {}", e))?;
                Ok(MountOption::Uid(value))
            }
            #[cfg(unix)]
            ("uid", None) => Err("uid requires a value".to_string()),
            #[cfg(unix)]
            ("gid", Some(value)) => {
                let value = value
                    .parse()
                    .map_err(|e| format!("Invalid gid value: {}", e))?;
                Ok(MountOption::Gid(value))
            }
            #[cfg(unix)]
            ("gid", None) => Err("gid requires a value".to_string()),
            #[cfg(unix)]
            ("default_mode", Some(value)) => {
                let value = u32::from_str_radix(value, 8)
                    .map_err(|e| format!("Invalid default_mode value: {}", e))?;
                Ok(MountOption::DefaultMode(value))
            }
            #[cfg(unix)]
            ("default_mode", None) => Err("default_mode requires a value".to_string()),
            #[cfg(unix)]
            ("fsname", Some(value)) => Ok(MountOption::FSName(value.to_string())),
            #[cfg(unix)]
            ("fsname", None) => Err("fsname requires a value".to_string()),
            #[cfg(unix)]
            ("subtype", Some(value)) => Ok(MountOption::Subtype(value.to_string())),
            #[cfg(unix)]
            ("subtype", None) => Err("subtype requires a value".to_string()),
            #[cfg(unix)]
            ("custom", Some(value)) => Ok(MountOption::Custom(value.to_string())),
            #[cfg(unix)]
            ("custom", None) => Err("custom requires a value".to_string()),
            #[cfg(unix)]
            ("allow_other", None) => Ok(MountOption::AllowOther),
            #[cfg(unix)]
            ("allow_root", None) => Ok(MountOption::AllowRoot),
            #[cfg(unix)]
            ("auto_unmount", None) => Ok(MountOption::AutoUnmount),
            #[cfg(unix)]
            ("default_permissions", None) => Ok(MountOption::DefaultPermissions),
            #[cfg(unix)]
            ("dev", None) => Ok(MountOption::Dev),
            #[cfg(unix)]
            ("nodev", None) => Ok(MountOption::NoDev),
            #[cfg(unix)]
            ("suid", None) => Ok(MountOption::Suid),
            #[cfg(unix)]
            ("nosuid", None) => Ok(MountOption::NoSuid),
            #[cfg(unix)]
            ("ro", None) => Ok(MountOption::RO),
            #[cfg(unix)]
            ("rw", None) => Ok(MountOption::RW),
            #[cfg(unix)]
            ("exec", None) => Ok(MountOption::Exec),
            #[cfg(unix)]
            ("noexec", None) => Ok(MountOption::NoExec),
            #[cfg(unix)]
            ("atime", None) => Ok(MountOption::Atime),
            #[cfg(unix)]
            ("noatime", None) => Ok(MountOption::NoAtime),
            #[cfg(unix)]
            ("dirsync", None) => Ok(MountOption::DirSync),
            #[cfg(unix)]
            ("sync", None) => Ok(MountOption::Sync),
            #[cfg(unix)]
            ("async", None) => Ok(MountOption::Async),
            #[cfg(windows)]
            ("single_thread", None) => Ok(MountOption::SingleThread),
            #[cfg(windows)]
            ("flags", Some(value)) => {
                let value = value
                    .parse()
                    .map_err(|e| format!("Invalid flags value: {}", e))?;
                Ok(MountOption::Flags(value))
            }
            #[cfg(windows)]
            ("flags", None) => Err("flags requires a value".to_string()),
            #[cfg(windows)]
            ("timeout", Some(value)) => {
                let value = std::time::Duration::from_millis(
                    value
                        .parse()
                        .map_err(|e| format!("Invalid timeout value: {}", e))?,
                );
                Ok(MountOption::Timeout(value))
            }
            #[cfg(windows)]
            ("timeout", None) => Err("timeout requires a value".to_string()),
            #[cfg(windows)]
            ("allocation_unit_size", Some(value)) => {
                let value = value
                    .parse()
                    .map_err(|e| format!("Invalid allocation_unit_size value: {}", e))?;
                Ok(MountOption::AllocationUnitSize(value))
            }
            #[cfg(windows)]
            ("allocation_unit_size", None) => {
                Err("allocation_unit_size requires a value".to_string())
            }
            #[cfg(windows)]
            ("sector_size", Some(value)) => {
                let value = value
                    .parse()
                    .map_err(|e| format!("Invalid sector_size value: {}", e))?;
                Ok(MountOption::SectorSize(value))
            }
            #[cfg(windows)]
            ("sector_size", None) => Err("sector_size requires a value".to_string()),
            _ => Err(format!("Unknown mount option: {}", s)),
        }
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_should_convert_str_to_option() {
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("uid=1000").unwrap(),
            MountOption::Uid(1000)
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("gid=1000").unwrap(),
            MountOption::Gid(1000)
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("default_mode=0755").unwrap(),
            MountOption::DefaultMode(0o755)
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("fsname=foo").unwrap(),
            MountOption::FSName("foo".to_string())
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("subtype=foo").unwrap(),
            MountOption::Subtype("foo".to_string())
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("custom=foo").unwrap(),
            MountOption::Custom("foo".to_string())
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("allow_other").unwrap(),
            MountOption::AllowOther
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("allow_root").unwrap(),
            MountOption::AllowRoot
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("auto_unmount").unwrap(),
            MountOption::AutoUnmount
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("default_permissions").unwrap(),
            MountOption::DefaultPermissions
        );
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("dev").unwrap(), MountOption::Dev);
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("nodev").unwrap(), MountOption::NoDev);
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("suid").unwrap(), MountOption::Suid);
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("nosuid").unwrap(),
            MountOption::NoSuid
        );
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("ro").unwrap(), MountOption::RO);
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("rw").unwrap(), MountOption::RW);
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("exec").unwrap(), MountOption::Exec);
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("noexec").unwrap(),
            MountOption::NoExec
        );
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("atime").unwrap(), MountOption::Atime);
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("noatime").unwrap(),
            MountOption::NoAtime
        );
        #[cfg(unix)]
        assert_eq!(
            MountOption::from_str("dirsync").unwrap(),
            MountOption::DirSync
        );
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("sync").unwrap(), MountOption::Sync);
        #[cfg(unix)]
        assert_eq!(MountOption::from_str("async").unwrap(), MountOption::Async);
        #[cfg(windows)]
        assert_eq!(
            MountOption::from_str("single_thread").unwrap(),
            MountOption::SingleThread
        );
        #[cfg(windows)]
        assert_eq!(
            MountOption::from_str("flags=1").unwrap(),
            MountOption::Flags(1)
        );
        #[cfg(windows)]
        assert_eq!(
            MountOption::from_str("timeout=1000").unwrap(),
            MountOption::Timeout(std::time::Duration::from_millis(1000))
        );
        #[cfg(windows)]
        assert_eq!(
            MountOption::from_str("allocation_unit_size=4096").unwrap(),
            MountOption::AllocationUnitSize(4096)
        );
        #[cfg(windows)]
        assert_eq!(
            MountOption::from_str("sector_size=512").unwrap(),
            MountOption::SectorSize(512)
        );
    }
}
