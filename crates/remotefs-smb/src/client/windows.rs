//! # Windows client
//!
//! Windows implementation of Smb fs client

mod credentials;
mod file_stream;

use std::path::{Path, PathBuf};

pub use credentials::SmbCredentials;
use file_stream::FileStream;
use filetime::{self, FileTime};
use remotefs::fs::stream::{ReadAndSeek, WriteAndSeek};
use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

/// SMB file system client
pub struct SmbFs {
    remote_path: PathBuf,
    wrkdir: PathBuf,
}

impl SmbFs {
    /// Instantiates a new SmbFs
    pub fn new(credentials: SmbCredentials) -> Self {
        Self {
            remote_path: PathBuf::from(format!(
                "\\\\{}\\{}",
                credentials.server, credentials.share
            )),
            wrkdir: PathBuf::from("\\"),
        }
    }

    /// Get full path for entry
    fn full_path(&self, p: &Path) -> PathBuf {
        let mut full_path = self.remote_path.clone();

        full_path.push(&self.wrkdir);
        full_path.push(p);

        full_path
    }
}

impl RemoteFs for SmbFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        self.list_dir(Path::new("\\"))?;
        debug!("connected to {}", self.remote_path.display());

        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        Ok(())
    }

    fn is_connected(&mut self) -> bool {
        self.list_dir(Path::new("\\")).is_ok()
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.connect()?;

        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
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
        let path = self.full_path(path);
        debug!("checking whether {} exists", path.display());
        Ok(path.exists())
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        let path = self.full_path(path);
        debug!("removing file {}", path.display());
        std::fs::remove_file(path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        let path = self.full_path(path);
        debug!("removing dir {}", path.display());
        std::fs::remove_dir(path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        let path = self.full_path(path);
        debug!("removing all at {}", path.display());
        std::fs::remove_dir_all(path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        let path = self.full_path(path);
        debug!("creating dir at {}", path.display());
        if path.exists() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        std::fs::create_dir(&path).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
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
                    p.push(src.file_name().unwrap().to_owned());
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
        let src = self.full_path(src);
        let dest = self.full_path(dest);
        debug!("moving {} to {}", src.display(), dest.display());

        std::fs::rename(src, dest).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        let path_abs = self.full_path(path);
        debug!("creating {} for reading...", path_abs.display());

        let writer = std::fs::OpenOptions::new()
            .write(true)
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
        let path = self.full_path(path);
        debug!("opening file {} for reading...", path.display());

        std::fs::File::open(path)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
            .map(|file| ReadStream::from(Box::new(FileStream::from(file)) as Box<dyn ReadAndSeek>))
    }
}

#[cfg(test)]
#[cfg(feature = "with-containers")]
mod test {

    use std::io::Cursor;
    use std::time::Duration;

    use serial_test::serial;

    use super::*;

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_append_to_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .create_file(p, &Metadata::default().size(10), Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .append_file(p, &Metadata::default().size(14), Box::new(reader))
                .ok()
                .unwrap(),
            14
        );
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 24);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_append_to_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/tmp/aaaaaaa/hbbbbb/a.txt");
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_change_directory() {
        crate::mock::logger();
        let mut client = init_client();
        let pwd = client.pwd().ok().unwrap();
        assert!(client.change_dir(Path::new("/cargo-test")).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_change_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_copy_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_directory() {
        crate::mock::logger();
        let mut client = init_client();
        // create directory
        assert!(client
            .create_dir(Path::new("/cargo-test/mydir"), UnixPex::from(0o755))
            .is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_directory_cause_already_exists() {
        crate::mock::logger();
        let mut client = init_client();
        // create directory
        assert!(client
            .create_dir(Path::new("/cargo-test/mydir"), UnixPex::from(0o755))
            .is_ok());
        assert_eq!(
            client
                .create_dir(Path::new("/cargo-test/mydir"), UnixPex::from(0o755))
                .err()
                .unwrap()
                .kind,
            RemoteErrorType::DirectoryAlreadyExists
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_directory() {
        crate::mock::logger();
        let mut client = init_client();
        // create directory
        assert!(client
            .create_dir(
                Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"),
                UnixPex::from(0o755)
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .create_file(p, &Metadata::default().size(10), Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_exec_command() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        assert!(client.exec("echo 5").is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_tell_whether_file_exists() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        assert_eq!(client.exists(p).ok().unwrap(), true);
        assert_eq!(
            client.exists(Path::new("/cargo-test/b.txt")).ok().unwrap(),
            false
        );
        assert_eq!(
            client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap(),
            false
        );
        assert_eq!(client.exists(Path::new("/cargo-test/")).ok().unwrap(), true);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_list_dir() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let wrkdir = client.pwd().ok().unwrap();
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert_eq!(
            client
                .append_file(p, &Metadata::default().size(10), Box::new(reader))
                .unwrap(),
            10
        );
        // Verify size
        let file = client
            .list_dir(Path::new("/cargo-test/"))
            .ok()
            .unwrap()
            .get(0)
            .unwrap()
            .clone();
        assert_eq!(file.name().as_str(), "a.txt");
        let mut expected_path = wrkdir;
        expected_path.push(p);
        assert_eq!(file.path.as_path(), expected_path.as_path());
        assert_eq!(file.extension().as_deref().unwrap(), "txt");
        assert_eq!(file.metadata.size, 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_list_dir() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
        finalize_client(client);
    }

    /*
    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_move_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let dest = Path::new("/cargo-test/b.txt");
        assert!(client.mov(p, dest).is_ok());
        assert_eq!(client.exists(p).ok().unwrap(), false);
        assert_eq!(client.exists(dest).ok().unwrap(), true);
        finalize_client(client);
    }
     */

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_move_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let dest = Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt");
        assert!(client.mov(p, dest).is_err());
        assert!(client
            .mov(Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"), p)
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_open_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default().size(10), Box::new(reader))
            .is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert_eq!(client.open_file(p, buffer).ok().unwrap(), 10);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_open_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client
            .open_file(Path::new("/tmp/aashafb/hhh"), buffer)
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_print_working_directory() {
        crate::mock::logger();
        let mut client = init_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    /*
    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_dir_all() {
        crate::mock::logger();
        let mut client = init_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Create file
        let mut file_path = dir_path.clone();
        file_path.push(Path::new("/cargo-test/a.txt"));
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(file_path.as_path(), &Metadata::default(), Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir_all(dir_path.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir_all() {
        crate::mock::logger();
        let mut client = init_client();
        // Remove dir
        assert!(client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_dir() {
        crate::mock::logger();
        let mut client = init_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        assert!(client.remove_dir(dir_path.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir() {
        crate::mock::logger();
        let mut client = init_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Create file
        let mut file_path = dir_path.clone();
        file_path.push(Path::new("a.txt"));
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(file_path.as_path(), &Metadata::default(), Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir(dir_path.as_path()).is_err());
        finalize_client(client);
    }
     */

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_setstat_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("bbbbb/cccc/a.sh");
        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: None,
                    created: None,
                    file_type: remotefs::fs::FileType::File,
                    gid: Some(1),
                    mode: Some(UnixPex::from(0o755)),
                    modified: None,
                    size: 7,
                    symlink: None,
                    uid: Some(1),
                }
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_stat_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default().size(7), Box::new(reader))
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("/cargo-test/a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.size, 7);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_stat_file() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_make_symlink() {
        crate::mock::logger();
        let mut client = init_client();
        // Create file
        let p = Path::new("/cargo-test/a.sh");
        let symlink = Path::new("/cargo-test/b.sh");
        assert!(client.symlink(symlink, p).is_err());
        finalize_client(client);
    }

    fn init_client() -> SmbFs {
        let _ = std::fs::remove_dir_all(Path::new("/tmp/cargo-test"));
        let client = SmbFs::new("localhost:3445", "temp");
        // make test dir
        let _ = std::fs::create_dir(Path::new("/tmp/cargo-test"));
        client
    }

    fn finalize_client(client: SmbFs) {
        remove_dir_all("/cargo-test");
        std::thread::sleep(Duration::from_secs(1));
        drop(client);
    }

    fn remove_dir_all<S: AsRef<str>>(dir: S) {
        let _ = std::fs::remove_dir_all(Path::new(dir.as_ref()));
    }
}
