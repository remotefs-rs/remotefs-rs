//! # Windows client
//!
//! Windows implementation of Smb fs client

mod credentials;
mod file_stream;

use std::ffi::CString;
use std::path::{Path, PathBuf};

pub use credentials::SmbCredentials;
use file_stream::FileStream;
use filetime::{self, FileTime};
use remotefs::fs::stream::{ReadAndSeek, WriteAndSeek};
use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use windows_sys::Win32::Foundation::{NO_ERROR, TRUE};
use windows_sys::Win32::NetworkManagement::WNet;

/// SMB file system client
pub struct SmbFs {
    remote_path: PathBuf,
    remote_name: String,
    credentials: SmbCredentials,
    wrkdir: PathBuf,
    is_connected: bool,
}

impl SmbFs {
    /// Instantiates a new SmbFs
    pub fn new(credentials: SmbCredentials) -> Self {
        let remote_name = format!("\\\\{}\\{}", credentials.server, credentials.share);
        Self {
            remote_path: PathBuf::from(&remote_name),
            remote_name,
            credentials,
            wrkdir: PathBuf::from("\\"),
            is_connected: false,
        }
    }

    /// Get full path for entry
    fn full_path(&self, p: &Path) -> PathBuf {
        let mut full_path = self.remote_path.clone();

        full_path.push(&self.wrkdir);
        full_path.push(p);

        full_path
    }

    fn check_connection(&mut self) -> RemoteResult<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn to_cstr(s: &str) -> CString {
        CString::new(s).unwrap()
    }
}

impl RemoteFs for SmbFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        // add connection
        trace!("connecting to {}", self.remote_name);

        let remote_name = Self::to_cstr(&self.remote_name);

        let mut resource = WNet::NETRESOURCEA {
            dwDisplayType: WNet::RESOURCEDISPLAYTYPE_SHAREADMIN,
            dwScope: WNet::RESOURCE_GLOBALNET,
            dwType: WNet::RESOURCETYPE_DISK,
            dwUsage: WNet::RESOURCEUSAGE_ALL,
            lpComment: std::ptr::null_mut(),
            lpLocalName: std::ptr::null_mut(),
            lpProvider: std::ptr::null_mut(),
            lpRemoteName: remote_name.as_c_str().as_ptr() as *mut u8,
        };

        let username = self
            .credentials
            .username
            .as_mut()
            .map(|username| Self::to_cstr(username));

        let password = self
            .credentials
            .password
            .as_mut()
            .map(|password| Self::to_cstr(password));

        let result = unsafe {
            let username_ptr = username
                .as_ref()
                .map(|username| username.as_ptr())
                .unwrap_or(std::ptr::null());
            let password_ptr = password
                .as_ref()
                .map(|password| password.as_ptr())
                .unwrap_or(std::ptr::null());
            WNet::WNetAddConnection2A(
                &mut resource as *mut WNet::NETRESOURCEA,
                password_ptr as *const u8,
                username_ptr as *const u8,
                WNet::CONNECT_INTERACTIVE,
            )
        };

        if result == NO_ERROR {
            self.is_connected = true;
            debug!("connected to {}", self.remote_path.display());
            Ok(Welcome::default())
        } else {
            Err(RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                result,
            ))
        }
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.check_connection()?;

        let remote_name = Self::to_cstr(&self.remote_name);

        let result =
            unsafe { WNet::WNetCancelConnection2A(remote_name.as_ptr() as *mut u8, 0, TRUE) };

        if result == NO_ERROR {
            self.is_connected = false;
            debug!("disconnected from {}", self.remote_path.display());
            Ok(())
        } else {
            Err(RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                result,
            ))
        }
    }

    fn is_connected(&mut self) -> bool {
        self.is_connected
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.check_connection()?;

        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        let path = self.full_path(dir);
        debug!("changing directory to {}", path.display());
        let file = self.stat(&path)?;
        if file.is_dir() {
            self.wrkdir = dir.to_path_buf();
            Ok(self.wrkdir.clone())
        } else {
            Err(RemoteError::new_ex(
                RemoteErrorType::BadFile,
                "path is not a directory",
            ))
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        self.check_connection()?;
        let abs_path = self.full_path(path);
        debug!("listing dir {}", abs_path.display());
        match std::fs::read_dir(abs_path) {
            Ok(e) => {
                let mut fs_entries: Vec<File> = Vec::new();
                for entry in e.flatten() {
                    match self.stat(entry.path().as_path()) {
                        Ok(entry) => fs_entries.push(entry),
                        Err(e) => error!("Failed to stat {}: {}", entry.path().display(), e),
                    }
                }
                Ok(fs_entries)
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, err)),
        }
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("stat {}", path.display());

        let attr = match std::fs::metadata(path.as_path()) {
            Ok(metadata) => metadata,
            Err(err) => {
                error!("Could not read file metadata: {}", err);
                return Err(RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, err));
            }
        };
        let metadata = Metadata::from(attr);
        // Match dir / file
        Ok(File { path, metadata })
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("setstat for {}", path.display());

        if let Some(mtime) = metadata.modified {
            let mtime = FileTime::from_system_time(mtime);
            debug!("setting mtime {:?}", mtime);
            filetime::set_file_mtime(&path, mtime)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))?;
        }
        if let Some(atime) = metadata.accessed {
            let atime = FileTime::from_system_time(atime);
            filetime::set_file_atime(path, atime)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))?;
        }
        Ok(())
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("checking whether {} exists", path.display());
        Ok(path.exists())
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("removing file {}", path.display());
        std::fs::remove_file(path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("removing dir {}", path.display());
        std::fs::remove_dir(path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("removing all at {}", path.display());
        std::fs::remove_dir_all(path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("creating dir at {}", path.display());
        if path.exists() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        std::fs::create_dir(&path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = self.full_path(src);
        let dest = self.full_path(dest);
        debug!("copying {} to {}", src.display(), dest.display());

        if src.is_dir() {
            // If destination path doesn't exist, create destination
            if !dest.exists() {
                debug!("Directory {} doesn't exist; creating it", dest.display());
                self.create_dir(dest.as_path(), UnixPex::from(0o775))?;
            }
            // Scan dir
            let dir_files: Vec<File> = self.list_dir(src.as_path())?;
            // Iterate files
            for dir_entry in dir_files.iter() {
                // Calculate dst
                let mut sub_dst = dest.clone();
                sub_dst.push(dir_entry.name());
                // Call function recursively
                self.copy(dir_entry.path(), sub_dst.as_path())?;
            }
        } else {
            // Copy file
            // If destination path is a directory, push file name
            let dest = match dest.as_path().is_dir() {
                true => {
                    let mut p: PathBuf = dest.clone();
                    p.push(src.file_name().unwrap());
                    p
                }
                false => dest.clone(),
            };
            // Copy entry path to dest path
            if let Err(err) = std::fs::copy(src, dest.as_path()) {
                error!("Failed to copy file: {}", err);
                return Err(RemoteError::new_ex(RemoteErrorType::IoError, err));
            }
            debug!("file copied");
        }
        Ok(())
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = self.full_path(src);
        let dest = self.full_path(dest);
        debug!("moving {} to {}", src.display(), dest.display());

        std::fs::rename(src, dest).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        self.check_connection()?;
        let path_abs = self.full_path(path);
        debug!("creating {} for reading...", path_abs.display());

        let writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path_abs)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
            .map(|file| {
                WriteStream::from(Box::new(FileStream::from(file)) as Box<dyn WriteAndSeek>)
            })?;

        self.setstat(path, metadata.clone())?;

        Ok(writer)
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        self.check_connection()?;
        let path_abs = self.full_path(path);
        debug!("creating {} for reading...", path_abs.display());

        let writer = std::fs::File::create(path_abs)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
            .map(|file| {
                WriteStream::from(Box::new(FileStream::from(file)) as Box<dyn WriteAndSeek>)
            })?;

        self.setstat(path, metadata.clone())?;

        Ok(writer)
    }

    fn open(&mut self, path: &Path) -> RemoteResult<ReadStream> {
        self.check_connection()?;
        let path = self.full_path(path);
        debug!("opening file {} for reading...", path.display());

        std::fs::File::open(path)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
            .map(|file| ReadStream::from(Box::new(FileStream::from(file)) as Box<dyn ReadAndSeek>))
    }
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    #[cfg(feature = "with-containers")]
    fn should_print_working_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_sync() {
        let client = SmbFs::new(SmbCredentials::new("pippo", "pippo"));

        is_sync(client);
    }

    #[test]
    fn test_should_be_send() {
        let client = SmbFs::new(SmbCredentials::new("pippo", "pippo"));

        is_send(client);
    }

    #[cfg(feature = "with-containers")]
    fn init_client() -> SmbFs {
        let mut client = SmbFs::new(
            SmbCredentials::new(env!("SMB_SERVER"), env!("SMB_SHARE"))
                .username(env!("SMB_USERNAME"))
                .password(env!("SMB_PASSWORD")),
        );
        assert!(client.connect().is_ok());

        client
    }

    #[cfg(feature = "with-containers")]
    fn finalize_client(mut client: SmbFs) {
        assert!(client.disconnect().is_ok());
    }
}
