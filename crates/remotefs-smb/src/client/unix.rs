//! # UNIX client
//!
//! UNIX implementation of Smb fs client

// -- exports
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use libc::mode_t;
pub use pavao::{SmbClient, SmbCredentials, SmbEncryptionLevel, SmbOptions, SmbShareMode};
use pavao::{SmbDirentType, SmbMode, SmbOpenOptions};
use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

use crate::utils::{path as path_utils, smb as smb_utils};

/// SMB file system client
pub struct SmbFs {
    client: SmbClient,
    wrkdir: PathBuf,
}

impl SmbFs {
    /// Try to create a new `SmbFs`.
    /// Fails if it is not possible to instantiate a smb context.
    pub fn try_new(credentials: SmbCredentials, options: SmbOptions) -> RemoteResult<Self> {
        Ok(Self {
            client: SmbClient::new(credentials, options)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::BadAddress, e))?,
            wrkdir: PathBuf::from("/"),
        })
    }

    /// Return a reference to the inner `pavao::SmbClient`
    pub fn client(&self) -> &SmbClient {
        &self.client
    }

    /// Return a mutable reference to the inner `pavao::SmbClient`
    pub fn client_mut(&mut self) -> &mut SmbClient {
        &mut self.client
    }

    // -- private

    fn check_connection(&self) -> RemoteResult<()> {
        trace!("checking connection...");
        match self.client.get_user() {
            Err(e) => {
                error!("connection ERROR: {}", e);
                Err(RemoteError::new_ex(RemoteErrorType::ConnectionError, e))
            }
            Ok(_) => {
                trace!("connection OK");
                Ok(())
            }
        }
    }

    fn get_uri<P: AsRef<Path>>(&self, p: P) -> String {
        let p = path_utils::absolutize(self.wrkdir.as_path(), p.as_ref());
        p.to_string_lossy().to_string()
    }
}

impl RemoteFs for SmbFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        // Get user to check whether connection works
        self.check_connection()?;
        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.check_connection()
    }

    fn is_connected(&mut self) -> bool {
        // test connection
        self.check_connection().is_ok()
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.check_connection().map(|_| self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        let dir = path_utils::absolutize(self.wrkdir.as_path(), dir);
        trace!("changing directory to {}", dir.display());
        // check if directory exists
        if self.stat(dir.as_path())?.is_dir() {
            self.wrkdir = dir;
            debug!("new working directory: {}", self.wrkdir.display());
            Ok(self.wrkdir.clone())
        } else {
            error!("cannot enter directory {}. Not a directory", dir.display());
            Err(RemoteError::new_ex(
                RemoteErrorType::BadFile,
                "not a directory",
            ))
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("listing files at {}", path);
        let dirents = self
            .client
            .list_dir(path.as_str())
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::StatFailed, e))?;
        // stat each dirent (NOTE: KEEP ONLY FILES AND DIRECTORIES)
        Ok(dirents
            .into_iter()
            .filter_map(|d| {
                if d.get_type() == SmbDirentType::File || d.get_type() == SmbDirentType::Dir {
                    let p = PathBuf::from(format!("{}/{}", path, d.name()));
                    Some(self.stat(&p))
                } else {
                    None
                }
            })
            .flatten()
            .collect())
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("get stat for {}", path);
        self.client
            .stat(path.as_str())
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::StatFailed, e))
            .map(|stat| smb_utils::smbstat_to_file(path, stat))
    }

    fn setstat(&mut self, _path: &Path, _metadata: Metadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        trace!("checking if {} exists...", path.display());
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(RemoteError {
                kind: RemoteErrorType::StatFailed,
                ..
            }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("removing file {}", path);
        self.client
            .unlink(path)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, e))
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("removing directory at {}", path);
        self.client
            .rmdir(path)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, e))
    }

    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()> {
        self.check_connection()?;
        if self.exists(path)? {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        let path = self.get_uri(path);
        trace!("making directory at {}", path);
        // check if directory exists
        self.client
            .mkdir(path, SmbMode::from(u32::from(mode) as mode_t))
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e))
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = self.get_uri(src);
        let dest = self.get_uri(dest);
        trace!("moving {} to {}", src, dest);
        // check if directory exists
        self.client
            .rename(src, dest)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn append_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("opening file at {} for append", path);
        let mut file = self
            .client
            .open_with(
                path,
                SmbOpenOptions::default()
                    .create(true)
                    .append(true)
                    .write(true)
                    .mode(
                        u32::from(metadata.mode.unwrap_or_else(|| UnixPex::from(0o644))) as mode_t,
                    ),
            )
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))?;
        io::copy(&mut reader, &mut file)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read + Send>,
    ) -> RemoteResult<u64> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("creating file at {}", path);
        let mut file = self
            .client
            .open_with(
                path,
                SmbOpenOptions::default()
                    .create(true)
                    .write(true)
                    .mode(
                        u32::from(metadata.mode.unwrap_or_else(|| UnixPex::from(0o644))) as mode_t,
                    ),
            )
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))?;
        io::copy(&mut reader, &mut file)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn open_file(&mut self, path: &Path, mut dest: Box<dyn Write + Send>) -> RemoteResult<u64> {
        self.check_connection()?;
        let path = self.get_uri(path);
        trace!("opening file at {} for read", path);
        let mut file = self
            .client
            .open_with(path, SmbOpenOptions::default().read(true))
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))?;
        io::copy(&mut file, &mut dest).map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn open(&mut self, _path: &Path) -> RemoteResult<ReadStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }
}

#[cfg(test)]
mod test {

    #[cfg(feature = "with-containers")]
    use std::io::Cursor;
    #[cfg(feature = "with-containers")]
    use std::time::Duration;

    #[cfg(feature = "with-containers")]
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

    fn is_send<T: Send>(_send: T) {}

    fn is_sync<T: Sync>(_sync: T) {}

    #[test]
    fn test_should_be_sync() {
        let client = SmbFs::try_new(
            SmbCredentials::default()
                .server("smb://localhost:3445")
                .share("/temp")
                .username("test")
                .password("test")
                .workgroup("pavao"),
            SmbOptions::default()
                .case_sensitive(true)
                .one_share_per_server(true),
        )
        .unwrap();

        is_sync(client);
    }

    #[test]
    fn test_should_be_send() {
        let client = SmbFs::try_new(
            SmbCredentials::default()
                .server("smb://localhost:3445")
                .share("/temp")
                .username("test")
                .password("test")
                .workgroup("pavao"),
            SmbOptions::default()
                .case_sensitive(true)
                .one_share_per_server(true),
        )
        .unwrap();

        is_send(client);
    }

    #[cfg(feature = "with-containers")]
    fn init_client() -> SmbFs {
        let _ = std::fs::remove_dir_all(Path::new("/tmp/cargo-test"));
        let client = SmbFs::try_new(
            SmbCredentials::default()
                .server("smb://localhost:3445")
                .share("/temp")
                .username("test")
                .password("test")
                .workgroup("pavao"),
            SmbOptions::default()
                .case_sensitive(true)
                .one_share_per_server(true),
        )
        .unwrap();
        // make test dir
        let _ = std::fs::create_dir(Path::new("/tmp/cargo-test"));
        client
    }

    #[cfg(feature = "with-containers")]
    fn finalize_client(client: SmbFs) {
        remove_dir_all("/cargo-test");
        std::thread::sleep(Duration::from_secs(1));
        drop(client);
    }

    #[cfg(feature = "with-containers")]
    fn remove_dir_all<S: AsRef<str>>(dir: S) {
        let _ = std::fs::remove_dir_all(Path::new(dir.as_ref()));
    }
}
