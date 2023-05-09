//! ## SFTP
//!
//! Sftp remote fs implementation

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use remotefs::fs::{
    FileType, Metadata, ReadStream, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex,
    Welcome, WriteStream,
};
use remotefs::File;
use ssh2::{FileStat, OpenFlags, OpenType, RenameFlags};
// -- export
pub use ssh2::{Session as SshSession, Sftp as SshSftp};

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
use super::{commons, SftpReadStream, SftpWriteStream, SshOpts};
use crate::utils::path as path_utils;

/// Sftp "filesystem" client
pub struct SftpFs {
    session: Option<SshSession>,
    sftp: Option<SshSftp>,
    wrkdir: PathBuf,
    opts: SshOpts,
}

impl SftpFs {
    /// Creates a new `SftpFs`
    pub fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            sftp: None,
            wrkdir: PathBuf::from("/"),
            opts,
        }
    }

    /// Get a reference to current `session` value.
    pub fn session(&mut self) -> Option<&mut SshSession> {
        self.session.as_mut()
    }

    /// Get a reference to current `sftp` value.
    pub fn sftp(&mut self) -> Option<&mut SshSftp> {
        self.sftp.as_mut()
    }

    // -- private

    /// Check connection status
    fn check_connection(&mut self) -> RemoteResult<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Make fsentry from SFTP stat
    fn make_fsentry(&self, path: &Path, metadata: &FileStat) -> File {
        let name = match path.file_name() {
            None => "/".to_string(),
            Some(name) => name.to_string_lossy().to_string(),
        };
        debug!("Found file {}", name);
        // parse metadata
        let uid = metadata.uid;
        let gid = metadata.gid;
        let mode = metadata.perm.map(UnixPex::from);
        let size = metadata.size.unwrap_or(0);
        let accessed = metadata.atime.map(|x| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(x))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        let modified = metadata.mtime.map(|x| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(x))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        let symlink = match metadata.file_type().is_symlink() {
            false => None,
            true => match self.sftp.as_ref().unwrap().readlink(path) {
                Ok(p) => Some(p),
                Err(err) => {
                    error!(
                        "Failed to read link of {} (even it's supposed to be a symlink): {}",
                        path.display(),
                        err
                    );
                    None
                }
            },
        };
        let file_type = if symlink.is_some() {
            FileType::Symlink
        } else if metadata.is_dir() {
            FileType::Directory
        } else {
            FileType::File
        };
        let entry_metadata = Metadata {
            accessed,
            created: None,
            file_type,
            gid,
            mode,
            modified,
            size,
            symlink,
            uid,
        };
        trace!("Metadata for {}: {:?}", path.display(), entry_metadata);
        File {
            path: path.to_path_buf(),
            metadata: entry_metadata,
        }
    }

    fn metadata_to_filestat(metadata: Metadata) -> FileStat {
        let atime = metadata
            .accessed
            .and_then(|x| x.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|x| x.as_secs());
        let mtime = metadata
            .modified
            .and_then(|x| x.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|x| x.as_secs());
        FileStat {
            size: Some(metadata.size),
            uid: metadata.uid,
            gid: metadata.gid,
            perm: metadata.mode.map(u32::from),
            atime,
            mtime,
        }
    }
}

impl RemoteFs for SftpFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("Initializing SFTP connection...");
        let session = commons::connect(&self.opts)?;
        // Set blocking to true
        session.set_blocking(true);
        // Get Sftp client
        debug!("Getting SFTP client...");
        let sftp = match session.sftp() {
            Ok(s) => s,
            Err(err) => {
                error!("Could not get sftp client: {}", err);
                return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
            }
        };
        // Get working directory
        debug!("Getting working directory...");
        self.wrkdir = match sftp.realpath(Path::new(".")) {
            Ok(p) => p,
            Err(err) => return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        };
        self.session = Some(session);
        self.sftp = Some(sftp);
        let banner: Option<String> = self.session.as_ref().unwrap().banner().map(String::from);
        debug!(
            "Connection established: '{}'; working directory {}",
            banner.as_deref().unwrap_or(""),
            self.wrkdir.display()
        );
        Ok(Welcome::default().banner(banner))
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        debug!("Disconnecting from remote...");
        if let Some(session) = self.session.as_ref() {
            // Disconnect (greet server with 'Mandi' as they do in Friuli)
            match session.disconnect(None, "Mandi!", None) {
                Ok(_) => {
                    // Set session and sftp to none
                    self.session = None;
                    self.sftp = None;
                    Ok(())
                }
                Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ConnectionError, err)),
            }
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn is_connected(&mut self) -> bool {
        self.session
            .as_ref()
            .map(|x| x.authenticated())
            .unwrap_or(false)
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        let dir = path_utils::absolutize(self.wrkdir.as_path(), dir);
        // Stat path to check if it exists. If it is a file, return error
        match self.stat(dir.as_path()) {
            Err(err) => Err(err),
            Ok(file) if file.is_dir() => {
                self.wrkdir = dir;
                debug!("Changed working directory to {}", self.wrkdir.display());
                Ok(self.wrkdir.clone())
            }
            Ok(_) => Err(RemoteError::new_ex(
                RemoteErrorType::BadFile,
                "expected directory, got file",
            )),
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Reading directory content of {}", path.display());
            match sftp.readdir(path.as_path()) {
                Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
                Ok(files) => Ok(files
                    .iter()
                    .map(|(path, metadata)| self.make_fsentry(path.as_path(), metadata))
                    .collect()),
            }
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Collecting metadata for {}", path.display());
            sftp.stat(path.as_path())
                .map(|x| self.make_fsentry(path.as_path(), &x))
                .map_err(|e| {
                    error!("Stat failed: {}", e);
                    RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, e)
                })
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Setting metadata for {}", path.display());
            sftp.setstat(path.as_path(), Self::metadata_to_filestat(metadata))
                .map(|_| ())
                .map_err(|e| {
                    error!("Setstat failed: {}", e);
                    RemoteError::new_ex(RemoteErrorType::StatFailed, e)
                })
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(RemoteError {
                kind: RemoteErrorType::NoSuchFileOrDirectory,
                ..
            }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Remove file {}", path.display());
            sftp.unlink(path.as_path()).map_err(|e| {
                error!("Remove failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, e)
            })
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Remove dir {}", path.display());
            sftp.rmdir(path.as_path()).map_err(|e| {
                error!("Remove failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, e)
            })
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        // Check if already exists
        debug!(
            "Creating directory {} (mode: {:o})",
            path.display(),
            u32::from(mode)
        );
        if self.exists(path.as_path())? {
            error!("directory {} already exists", path.display());
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        self.sftp
            .as_ref()
            .unwrap()
            .mkdir(path.as_path(), u32::from(mode) as i32)
            .map_err(|e| {
                error!("Create dir failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e)
            })
    }

    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        // Check if already exists
        debug!(
            "Creating symlink at {} pointing to {}",
            path.display(),
            target.display()
        );
        if !self.exists(target)? {
            error!("target {} doesn't exist", target.display());
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        self.sftp
            .as_ref()
            .unwrap()
            .symlink(target, path.as_path())
            .map_err(|e| {
                error!("Symlink failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e)
            })
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = path_utils::absolutize(self.wrkdir.as_path(), src);
        // check if file exists
        if !self.exists(src.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        let dest = path_utils::absolutize(self.wrkdir.as_path(), dest);
        debug!("Copying {} to {}", src.display(), dest.display());
        // Run `cp -rf`
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("cp -rf \"{}\" \"{}\"", src.display(), dest.display()).as_str(),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new_ex(
                // Could not copy file
                RemoteErrorType::FileCreateDenied,
                format!("\"{}\"", dest.display()),
            )),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = path_utils::absolutize(self.wrkdir.as_path(), src);
        // check if file exists
        if !self.exists(src.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        let dest = path_utils::absolutize(self.wrkdir.as_path(), dest);
        debug!("Moving {} to {}", src.display(), dest.display());
        self.sftp
            .as_ref()
            .unwrap()
            .rename(src.as_path(), dest.as_path(), Some(RenameFlags::OVERWRITE))
            .map_err(|e| {
                error!("Move failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e)
            })
    }

    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.check_connection()?;
        debug!(r#"Executing command "{}""#, cmd);
        commons::perform_shell_cmd_at_with_rc(
            self.session.as_mut().unwrap(),
            cmd,
            self.wrkdir.as_path(),
        )
    }

    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Opening file at {} for appending", path.display());
            let mode = metadata.mode.map(|x| u32::from(x) as i32).unwrap_or(0o644);
            sftp.open_mode(
                path.as_path(),
                OpenFlags::CREATE | OpenFlags::APPEND | OpenFlags::WRITE,
                mode,
                OpenType::File,
            )
            .map(SftpWriteStream::from)
            .map(WriteStream::from)
            .map_err(|e| {
                error!("Append failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e)
            })
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Creating file at {}", path.display());
            let mode = metadata.mode.map(|x| u32::from(x) as i32).unwrap_or(0o644);
            sftp.open_mode(
                path.as_path(),
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
                mode,
                OpenType::File,
            )
            .map(SftpWriteStream::from)
            .map(WriteStream::from)
            .map_err(|e| {
                error!("Create failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e)
            })
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn open(&mut self, path: &Path) -> RemoteResult<ReadStream> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        // check if file exists
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Opening file at {}", path.display());
        self.sftp
            .as_ref()
            .unwrap()
            .open(path.as_path())
            .map(SftpReadStream::from)
            .map(ReadStream::from)
            .map_err(|e| {
                error!("Open failed: {}", e);
                RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e)
            })
    }

    // -- override (std::io::copy is VERY slow on SFTP <https://github.com/veeso/remotefs-rs/issues/6>)

    fn append_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<u64> {
        if self.is_connected() {
            let mut stream = self.append(path, metadata)?;
            trace!("Opened remote file");
            let mut bytes: usize = 0;
            let transfer_size = metadata.size as usize;
            while bytes < transfer_size {
                let mut buffer: [u8; 65535] = [0; 65535];
                let bytes_read = reader.read(&mut buffer).map_err(|e| {
                    error!("Failed to read from file: {}", e);
                    RemoteError::new_ex(RemoteErrorType::IoError, e)
                })?;
                let mut delta = 0;
                while delta < bytes_read {
                    delta += stream.write(&buffer[delta..bytes_read]).map_err(|e| {
                        error!("Failed to write to stream: {}", e);
                        RemoteError::new_ex(RemoteErrorType::IoError, e)
                    })?;
                }
                bytes += bytes_read;
            }
            self.on_written(stream)?;
            trace!("Written {} bytes to destination", bytes);
            Ok(bytes as u64)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn std::io::Read>,
    ) -> RemoteResult<u64> {
        if self.is_connected() {
            let mut stream = self.create(path, metadata)?;
            trace!("Opened remote file");
            let mut bytes: usize = 0;
            let transfer_size = metadata.size as usize;
            while bytes < transfer_size {
                let mut buffer: [u8; 65535] = [0; 65535];
                let bytes_read = reader.read(&mut buffer).map_err(|e| {
                    error!("Failed to read from file: {}", e);
                    RemoteError::new_ex(RemoteErrorType::IoError, e)
                })?;
                let mut delta = 0;
                while delta < bytes_read {
                    delta += stream.write(&buffer[delta..bytes_read]).map_err(|e| {
                        error!("Failed to write to stream: {}", e);
                        RemoteError::new_ex(RemoteErrorType::IoError, e)
                    })?;
                }
                bytes += bytes_read;
            }
            self.on_written(stream)?;
            trace!("Written {} bytes to destination", bytes);
            Ok(bytes as u64)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn open_file(&mut self, src: &Path, mut dest: Box<dyn Write + Send>) -> RemoteResult<u64> {
        if self.is_connected() {
            let transfer_size = self.stat(src)?.metadata().size as usize;
            let mut stream = self.open(src)?;
            trace!("File opened");
            let mut bytes: usize = 0;
            while bytes < transfer_size {
                let mut buffer: [u8; 65535] = [0; 65535];
                let bytes_read = stream.read(&mut buffer).map_err(|e| {
                    error!("Failed to read from stream: {}", e);
                    RemoteError::new_ex(RemoteErrorType::IoError, e)
                })?;
                let mut delta = 0;
                while delta < bytes_read {
                    delta += dest.write(&buffer[delta..bytes_read]).map_err(|e| {
                        error!("Failed to write to file: {}", e);
                        RemoteError::new_ex(RemoteErrorType::IoError, e)
                    })?;
                }
                bytes += bytes_read;
            }
            self.on_read(stream)?;
            trace!("Copied {} bytes to destination", bytes);
            Ok(bytes as u64)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
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
    #[cfg(feature = "with-containers")]
    use crate::mock::ssh as ssh_mock;

    #[test]
    fn should_initialize_sftp_filesystem() {
        let mut client = SftpFs::new(SshOpts::new("127.0.0.1"));
        assert!(client.session.is_none());
        assert!(client.sftp.is_none());
        assert_eq!(client.wrkdir, PathBuf::from("/"));
        assert_eq!(client.is_connected(), false);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_append_to_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
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
        let mut client = setup_client();
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
        let mut client = setup_client();
        let pwd = client.pwd().ok().unwrap();
        assert!(client.change_dir(Path::new("/tmp")).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_change_directory() {
        crate::mock::logger();
        let mut client = setup_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_copy_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.copy(p, Path::new("b.txt")).is_ok());
        assert!(client.stat(p).is_ok());
        assert!(client.stat(Path::new("b.txt")).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_copy_file() {
        crate::mock::logger();
        let mut client = setup_client();
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
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_create_directory_cause_already_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        assert_eq!(
            client
                .create_dir(Path::new("mydir"), UnixPex::from(0o755))
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_create_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
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
        let mut client = setup_client();
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
    fn should_exec_command() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        assert_eq!(
            client.exec("echo 5").ok().unwrap(),
            (0, String::from("5\n"))
        );
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_tell_whether_file_exists() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        assert_eq!(client.exists(p).ok().unwrap(), true);
        assert_eq!(client.exists(Path::new("b.txt")).ok().unwrap(), false);
        assert_eq!(
            client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap(),
            false
        );
        assert_eq!(client.exists(Path::new("/tmp")).ok().unwrap(), true);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_list_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let wrkdir = client.pwd().ok().unwrap();
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default().size(10), Box::new(reader))
            .is_ok());
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
        assert_eq!(file.metadata.mode.unwrap(), UnixPex::from(0o644));
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_list_dir() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_move_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        // Verify size
        let dest = Path::new("b.txt");
        assert!(client.mov(p, dest).is_ok());
        assert_eq!(client.exists(p).ok().unwrap(), false);
        assert_eq!(client.exists(dest).ok().unwrap(), true);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_move_file() {
        crate::mock::logger();
        let mut client = setup_client();
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
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
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
        let mut client = setup_client();
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
        let mut client = setup_client();
        assert!(client.pwd().is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
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
        let mut client = setup_client();
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
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_remove_dir() {
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
        assert!(client
            .create_file(file_path.as_path(), &Metadata::default(), Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir(dir_path.as_path()).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_remove_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.remove_file(p).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_setstat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());

        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: Some(SystemTime::UNIX_EPOCH),
                    created: None,
                    file_type: FileType::File,
                    gid: Some(1000),
                    mode: Some(UnixPex::from(0o755)),
                    modified: Some(SystemTime::UNIX_EPOCH),
                    size: 7,
                    symlink: None,
                    uid: Some(1000),
                }
            )
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        let stat = entry.metadata();
        assert_eq!(stat.accessed, Some(SystemTime::UNIX_EPOCH));
        assert_eq!(stat.created, None);
        assert_eq!(stat.gid.unwrap(), 1000);
        assert_eq!(stat.modified, Some(SystemTime::UNIX_EPOCH));
        assert_eq!(stat.mode.unwrap(), UnixPex::from(0o755));
        assert_eq!(stat.size, 7);
        assert_eq!(stat.uid.unwrap(), 1000);

        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_setstat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("bbbbb/cccc/a.sh");
        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: None,
                    created: None,
                    file_type: FileType::File,
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
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default().size(7), Box::new(reader))
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.mode.unwrap(), UnixPex::from(0o644));
        assert_eq!(meta.size, 7);
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_stat_file() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_make_symlink() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        let symlink = Path::new("b.sh");
        assert!(client.symlink(symlink, p).is_ok());
        assert!(client.remove_file(symlink).is_ok());
        finalize_client(client);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
    fn should_not_make_symlink() {
        crate::mock::logger();
        let mut client = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(p, &Metadata::default(), Box::new(reader))
            .is_ok());
        let symlink = Path::new("b.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(symlink, &Metadata::default(), Box::new(reader))
            .is_ok());
        assert!(client.symlink(symlink, p).is_err());
        assert!(client.remove_file(symlink).is_ok());
        assert!(client.symlink(symlink, Path::new("c.sh")).is_err());
        finalize_client(client);
    }

    #[test]
    fn should_return_not_connected_error() {
        crate::mock::logger();
        let mut client = SftpFs::new(SshOpts::new("127.0.0.1"));
        assert!(client.change_dir(Path::new("/tmp")).is_err());
        assert!(client
            .copy(Path::new("/nowhere"), PathBuf::from("/culonia").as_path())
            .is_err());
        assert!(client.exec("echo 5").is_err());
        assert!(client.disconnect().is_err());
        assert!(client.symlink(Path::new("/a"), Path::new("/b")).is_err());
        assert!(client.list_dir(Path::new("/tmp")).is_err());
        assert!(client
            .create_dir(Path::new("/tmp"), UnixPex::from(0o755))
            .is_err());
        assert!(client.pwd().is_err());
        assert!(client.remove_dir_all(Path::new("/nowhere")).is_err());
        assert!(client
            .mov(Path::new("/nowhere"), Path::new("/culonia"))
            .is_err());
        assert!(client.stat(Path::new("/tmp")).is_err());
        assert!(client
            .setstat(Path::new("/tmp"), Metadata::default())
            .is_err());
        assert!(client.open(Path::new("/tmp/pippo.txt")).is_err());
        assert!(client
            .create(Path::new("/tmp/pippo.txt"), &Metadata::default())
            .is_err());
        assert!(client
            .append(Path::new("/tmp/pippo.txt"), &Metadata::default())
            .is_err());
    }

    // -- test utils

    #[cfg(feature = "with-containers")]
    fn setup_client() -> SftpFs {
        let config_file = ssh_mock::create_ssh_config();
        let mut client = SftpFs::new(
            SshOpts::new("sftp")
                .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
                .config_file(config_file.path()),
        );
        assert!(client.connect().is_ok());
        // Create wrkdir
        let tempdir = PathBuf::from(generate_tempdir());
        assert!(client
            .create_dir(tempdir.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Change directory
        assert!(client.change_dir(tempdir.as_path()).is_ok());
        client
    }

    #[cfg(feature = "with-containers")]
    fn finalize_client(mut client: SftpFs) {
        // Get working directory
        let wrkdir = client.pwd().ok().unwrap();
        // Remove directory
        assert!(client.remove_dir_all(wrkdir.as_path()).is_ok());
        assert!(client.disconnect().is_ok());
    }

    #[cfg(feature = "with-containers")]
    fn generate_tempdir() -> String {
        use rand::distributions::Alphanumeric;
        use rand::{thread_rng, Rng};
        let mut rng = thread_rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("/tmp/temp_{}", name)
    }
}
