#![crate_name = "remotefs_webdav"]
#![crate_type = "lib"]

//! # remotefs-webdav
//!
//! remotefs is a library that provides a client implementation of [Remotefs-rs](https://github.com/veeso/remotefs-rs)
//! for the WebDAV protocol as specified in [RFC4918](https://www.rfc-editor.org/rfc/rfc4918).
//!
//! ## Get started
//!
//! First of all you need to add **remotefs** and **remotefs-webdav** to your project dependencies:
//!
//! ```toml
//! [dependencies]
//! remotefs = "^0.3"
//! remotefs-webdav = "^0.2"
//! ```
//!
//! these features are supported:
//!
//! - `no-log`: disable logging. By default, this library will log via the `log` crate.

#![doc(html_playground_url = "https://play.rust-lang.org")]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo-128.png"
)]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/remotefs-rs/remotefs-rs/main/assets/logo.png"
)]

#[macro_use]
extern crate log;

#[cfg(test)]
mod mock;
mod parser;
mod webdav_xml;

use std::io::Read;
use std::path::{Path, PathBuf};

use remotefs::fs::{Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use rustydav::client::Client;

use self::parser::ResponseParser;

/// WebDAV remote fs client
pub struct WebDAVFs {
    client: Client,
    url: String,
    wrkdir: String,
    connected: bool,
}

impl WebDAVFs {
    /// Create a new WebDAVFs instance
    pub fn new(username: &str, password: &str, url: &str) -> WebDAVFs {
        WebDAVFs {
            client: Client::init(username, password),
            url: url.to_string(),
            wrkdir: String::from("/"),
            connected: false,
        }
    }

    /// Resolve query url
    fn url(&self, path: &Path, force_dir: bool) -> String {
        let mut p = self.url.clone();
        p.push_str(&self.path(path).to_string_lossy());
        if !p.ends_with('/') && (path.is_dir() || force_dir) {
            p.push('/');
        }
        p
    }

    /// Resolve path
    fn path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(&self.wrkdir).join(path)
        }
    }
}

impl RemoteFs for WebDAVFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        //self.list_dir(Path::new("/"))?;
        self.connected = true;

        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&mut self) -> bool {
        self.connected
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        Ok(PathBuf::from(&self.wrkdir))
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        let new_dir = self.path(dir);
        self.list_dir(&new_dir)?;

        self.wrkdir = new_dir.to_string_lossy().to_string();
        if !self.wrkdir.ends_with('/') {
            self.wrkdir.push('/');
        }
        debug!("Changed directory to: {}", self.wrkdir);
        Ok(new_dir)
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        let url = self.url(path, true);
        debug!("Listing directory: {}", url);
        let response = self
            .client
            .list(&url, "1")
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        debug!("Parsing response");
        match ResponseParser::from(response).files()? {
            files if !files.is_empty() => {
                // remove file at 0
                let mut children = Vec::with_capacity(files.len());
                for file in files.iter().skip(1) {
                    children.push(file.clone());
                }
                Ok(children)
            }
            _ => Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)),
        }
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        let url = self.url(path, false);
        debug!("Listing directory: {}", url);
        let response = self
            .client
            .list(&url, "1")
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        debug!("Parsing response");
        match ResponseParser::from(response).files()? {
            files if !files.is_empty() => Ok(files[0].clone()),
            _ => Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)),
        }
    }

    fn setstat(&mut self, _path: &Path, _metadata: Metadata) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        debug!("Checking if file exists: {}", path.display());
        Ok(self.stat(path).is_ok())
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        let url = self.url(path, false);
        debug!("Removing file: {}", url);
        let response = self
            .client
            .delete(&url)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        ResponseParser::from(response).status()
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        let url = self.url(path, true);
        debug!("Removing directory: {}", url);
        let response = self
            .client
            .delete(&url)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        ResponseParser::from(response).status()
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        self.remove_dir(path)
    }

    fn create_dir(&mut self, path: &Path, _mode: UnixPex) -> RemoteResult<()> {
        if self.stat(path).is_ok() {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        let url = self.url(path, true);
        // check if dir exists
        debug!("Creating directory: {}", url);
        let response = self
            .client
            .mkcol(&url)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        ResponseParser::from(response).status()
    }

    fn symlink(&mut self, _path: &Path, _target: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn copy(&mut self, _src: &Path, _dest: &Path) -> RemoteResult<()> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src_url = self.url(src, false);
        let dest_url = self.url(dest, false);
        debug!("Moving file: {} to {}", src_url, dest_url);

        let response = self
            .client
            .mv(&src_url, &dest_url)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        ResponseParser::from(response).status()
    }

    fn exec(&mut self, _cmd: &str) -> RemoteResult<(u32, String)> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
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

    fn create_file(
        &mut self,
        path: &Path,
        _metadata: &Metadata,
        mut reader: Box<dyn std::io::Read + Send>,
    ) -> RemoteResult<u64> {
        let url = self.url(path, false);
        debug!("Creating file: {}", url);
        let mut content = Vec::new();
        reader
            .read_to_end(&mut content)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
        let size = content.len() as u64;
        let response = self
            .client
            .put(content, &url)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        ResponseParser::from(response).status()?;

        Ok(size)
    }

    fn open_file(
        &mut self,
        src: &Path,
        mut dest: Box<dyn std::io::Write + Send>,
    ) -> RemoteResult<u64> {
        let url = self.url(src, false);
        debug!("Opening file: {}", url);
        let mut response = self
            .client
            .get(&url)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e))?;

        // write to dest
        let mut buf = vec![0; 1024];
        let mut total_size = 0;
        loop {
            let n = response
                .read(&mut buf)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
            total_size += n as u64;
            if n == 0 {
                return Ok(total_size);
            }
            dest.write_all(&buf[..n])
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::IoError, e))?;
        }
    }
}

#[cfg(test)]
mod test {

    #[cfg(feature = "with-containers")]
    use std::io::Cursor;

    use pretty_assertions::assert_eq;
    #[cfg(feature = "with-containers")]
    use serial_test::serial;

    use super::*;

    #[test]
    fn test_should_init_client() {
        crate::mock::logger();
        let client = WebDAVFs::new("user", "password", "http://localhost:3080");
        assert_eq!(client.url, "http://localhost:3080");
        assert_eq!(client.wrkdir, "/");
    }

    #[test]
    fn test_should_get_url() {
        let mut client = WebDAVFs::new("user", "password", "http://localhost:3080");
        let path = Path::new("a.txt");
        assert_eq!(client.url(path, false), "http://localhost:3080/a.txt");

        let path = Path::new("/a.txt");
        assert_eq!(client.url(path, false), "http://localhost:3080/a.txt");

        let path = Path::new("/");
        assert_eq!(client.url(path, false), "http://localhost:3080/");

        client.wrkdir = "/test/".to_string();
        let path = Path::new("a.txt");
        assert_eq!(client.url(path, false), "http://localhost:3080/test/a.txt");

        let path = Path::new("/a.txt");
        assert_eq!(client.url(path, false), "http://localhost:3080/a.txt");

        let path = Path::new("/gabibbo");
        assert_eq!(client.url(path, true), "http://localhost:3080/gabibbo/");
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_append_to_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_change_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_copy_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_create_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_create_directory_cause_already_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir/"), UnixPex::from(0o755))
            .is_ok());
        assert_eq!(
            client
                .create_dir(Path::new("mydir/"), UnixPex::from(0o755))
                .unwrap_err()
                .kind,
            RemoteErrorType::DirectoryAlreadyExists
        );
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_create_directory() {
        crate::mock::logger();
        let mut client = setup_client();
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
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_create_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert_eq!(
            client
                .create_file(p, &metadata, Box::new(reader))
                .ok()
                .unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_exec_command() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client.exec("echo 5").is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_tell_whether_file_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        assert_eq!(client.exists(p).ok().unwrap(), true);
        assert_eq!(client.exists(Path::new("b.txt")).ok().unwrap(), false);
        assert_eq!(
            client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap(),
            false
        );
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_list_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let wrkdir = client.pwd().ok().unwrap();
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let file = client
            .list_dir(wrkdir.as_path())
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
        assert_eq!(file.metadata.mode, None);
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_move_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let dest = Path::new("b.txt");
        assert!(client.mov(p, dest).is_ok());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_open_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert_eq!(client.open_file(p, buffer).ok().unwrap(), 10);
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_print_working_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_remove_dir_all() {
        crate::mock::logger();
        let mut client = setup_client();
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir_all(dir_path.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_remove_dir() {
        crate::mock::logger();
        let mut client = setup_client();
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
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_remove_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Remove dir
        assert!(client.remove_dir(Path::new("test/")).is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_remove_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.remove_file(p).is_ok());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_setstat_file() {
        use std::time::SystemTime;

        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: Some(SystemTime::UNIX_EPOCH),
                    created: Some(SystemTime::UNIX_EPOCH),
                    gid: Some(1000),
                    file_type: remotefs::fs::FileType::File,
                    mode: Some(UnixPex::from(0o755)),
                    modified: Some(SystemTime::UNIX_EPOCH),
                    size: 7,
                    symlink: None,
                    uid: Some(1000),
                }
            )
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_stat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.size, 7);
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_not_stat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(client);
    }

    #[test]
    #[serial]
    #[cfg(feature = "with-containers")]
    fn should_make_symlink() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let symlink = Path::new("b.sh");
        assert!(client.symlink(symlink, p).is_err());
        finalize_client(client);
    }

    #[cfg(feature = "with-containers")]
    fn setup_client() -> WebDAVFs {
        let mut client = WebDAVFs::new("alice", "secret1234", "http://localhost:3080");
        assert!(client.connect().is_ok(), "connect");
        // generate random string
        let wrkdir = PathBuf::from(format!("/test-{}/", uuid::Uuid::new_v4()));
        // mkdir
        assert!(
            client.create_dir(&wrkdir, UnixPex::from(0o755)).is_ok(),
            "create tempdir"
        );
        // change dir
        assert!(client.change_dir(&wrkdir).is_ok(), "change dir");
        assert!(client.is_connected(), "connected");

        client
    }

    #[cfg(feature = "with-containers")]
    fn finalize_client(mut client: WebDAVFs) {
        let wrkdir = client.pwd().unwrap();
        // remove tempdir
        assert!(client.remove_dir_all(&wrkdir).is_ok(), "remove tempdir");
        assert!(client.disconnect().is_ok(), "disconnect");
        assert!(!client.is_connected(), "disconnected");
    }
}
