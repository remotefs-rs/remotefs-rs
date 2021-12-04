//! ## SFTP
//!
//! Sftp remote fs implementation

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
use super::{commons, SshOpts};
use crate::fs::{Metadata, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex, Welcome};
use crate::utils::path as path_utils;
use crate::{Directory, Entry, File};

use ssh2::{FileStat, OpenFlags, OpenType, RenameFlags};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime};

// -- export
pub use ssh2::{Session as SshSession, Sftp as SshSftp};

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
    fn check_connection(&self) -> RemoteResult<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Make fsentry from SFTP stat
    fn make_fsentry(&self, path: &Path, metadata: &FileStat) -> Entry {
        let name = match path.file_name() {
            None => "/".to_string(),
            Some(name) => name.to_string_lossy().to_string(),
        };
        debug!("Found file {}", name);
        // parse metadata
        let extension = path
            .extension()
            .map(|ext| String::from(ext.to_str().unwrap_or("")));
        let uid = metadata.uid;
        let gid = metadata.gid;
        let mode = metadata.perm.map(UnixPex::from);
        let size = metadata.size.unwrap_or(0);
        let atime = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(metadata.atime.unwrap_or(0)))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mtime: SystemTime = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(metadata.mtime.unwrap_or(0)))
            .unwrap_or(SystemTime::UNIX_EPOCH);
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
        let entry_metadata = Metadata {
            atime,
            ctime: SystemTime::UNIX_EPOCH,
            gid,
            mode,
            mtime,
            size,
            symlink,
            uid,
        };
        trace!("Metadata for {}: {:?}", path.display(), entry_metadata);
        if metadata.is_dir() {
            Entry::Directory(Directory {
                name,
                abs_path: path.to_path_buf(),
                metadata: entry_metadata,
            })
        } else {
            Entry::File(File {
                name,
                abs_path: path.to_path_buf(),
                metadata: entry_metadata,
                extension,
            })
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

    fn is_connected(&self) -> bool {
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
            Ok(Entry::File(_)) => Err(RemoteError::new_ex(
                RemoteErrorType::BadFile,
                "expected directory, got file",
            )),
            Ok(Entry::Directory(_)) => {
                self.wrkdir = dir;
                debug!("Changed working directory to {}", self.wrkdir.display());
                Ok(self.wrkdir.clone())
            }
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<Entry>> {
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

    fn stat(&mut self, path: &Path) -> RemoteResult<Entry> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Collecting metadata for {}", path.display());
            sftp.stat(path.as_path())
                .map(|x| self.make_fsentry(path.as_path(), &x))
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Setting metadata for {}", path.display());
            sftp.setstat(path.as_path(), FileStat::from(metadata))
                .map(|_| ())
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::StatFailed, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        match self.stat(path) {
            Ok(_) => Ok(true),
            Err(RemoteError {
                code: RemoteErrorType::NoSuchFileOrDirectory,
                ..
            }) => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Remove file {}", path.display());
            sftp.unlink(path.as_path())
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Remove dir {}", path.display());
            sftp.rmdir(path.as_path())
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotRemoveFile, e))
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
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e))
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let src = path_utils::absolutize(self.wrkdir.as_path(), src);
        let dest = path_utils::absolutize(self.wrkdir.as_path(), dest);
        debug!("Copying {} to {}", src.display(), dest.display());
        // Run `cp -rf`
        match commons::perform_shell_cmd_at(
            self.session.as_mut().unwrap(),
            format!(
                "cp -rf \"{}\" \"{}\"; echo $?",
                src.display(),
                dest.display()
            )
            .as_str(),
            self.wrkdir.as_path(),
        ) {
            Ok(output) => {
                match output.as_str().trim() == "0" {
                    true => Ok(()), // File copied
                    false => Err(RemoteError::new_ex(
                        // Could not copy file
                        RemoteErrorType::FileCreateDenied,
                        format!("\"{}\"", dest.display()),
                    )),
                }
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        if let Some(sftp) = self.sftp.as_ref() {
            let src = path_utils::absolutize(self.wrkdir.as_path(), src);
            let dest = path_utils::absolutize(self.wrkdir.as_path(), dest);
            debug!("Copying {} to {}", src.display(), dest.display());
            sftp.rename(src.as_path(), dest.as_path(), Some(RenameFlags::OVERWRITE))
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.check_connection()?;
        debug!(r#"Executing command "{}""#, cmd);
        commons::perform_shell_cmd_at(
            self.session.as_mut().unwrap(),
            format!("{}; echo $?", cmd),
            self.wrkdir.as_path(),
        )
        .map(|output| {
            if let Some(index) = output.trim().rfind('\n') {
                trace!("Read from stdout: '{}'", output);
                let actual_output = (&output[0..index + 1]).to_string();
                let rc = u32::from_str(&output[index..]).ok().unwrap_or(0);
                debug!(r#"Command output: "{}"; exit code: {}"#, actual_output, rc);
                (rc, actual_output)
            } else {
                (u32::from_str(&output).ok().unwrap_or(0), String::new())
            }
        })
    }

    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<Box<dyn Write>> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Opening file at {} for appending", path.display());
            let mode = metadata.mode.map(|x| u32::from(x) as i32).unwrap_or(0o644);
            sftp.open_mode(
                path.as_path(),
                OpenFlags::APPEND | OpenFlags::WRITE,
                mode,
                OpenType::File,
            )
            .map(|f| Box::new(BufWriter::with_capacity(65536, f)) as Box<dyn Write>)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<Box<dyn Write>> {
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
            .map(|f| Box::new(BufWriter::with_capacity(65536, f)) as Box<dyn Write>)
            .map_err(|e| RemoteError::new_ex(RemoteErrorType::FileCreateDenied, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn open(&mut self, path: &Path) -> RemoteResult<Box<dyn Read>> {
        if let Some(sftp) = self.sftp.as_ref() {
            let path = path_utils::absolutize(self.wrkdir.as_path(), path);
            debug!("Opening file at {}", path.display());
            sftp.open(path.as_path())
                .map(|f| Box::new(BufReader::with_capacity(65536, f)) as Box<dyn Read>)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, e))
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }
}

// -- impl

impl From<Metadata> for FileStat {
    fn from(metadata: Metadata) -> Self {
        FileStat {
            size: Some(metadata.size),
            uid: metadata.uid,
            gid: metadata.gid,
            perm: metadata.mode.map(u32::from),
            atime: metadata
                .atime
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|x| x.as_secs()),
            mtime: metadata
                .mtime
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .map(|x| x.as_secs()),
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use crate::mock::fs as fs_mock;
    use crate::mock::ssh as ssh_mock;

    use pretty_assertions::assert_eq;
    use std::fs::File as StdFile;

    #[test]
    fn should_initialize_sftp_filesystem() {
        let client = SftpFs::new(SshOpts::new("127.0.0.1"));
        assert!(client.session.is_none());
        assert!(client.sftp.is_none());
        assert_eq!(client.wrkdir, PathBuf::from("/"));
        assert_eq!(client.is_connected(), false);
    }

    #[test]
    #[cfg(feature = "with-containers")]
    fn should_operate_on_sftp_file_system() {
        crate::mock::logger();
        let config_file = ssh_mock::create_ssh_config();
        let mut client = SftpFs::new(
            SshOpts::new("sftp")
                .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
                .config_file(config_file.path()),
        );
        // Sample file
        let (entry, file) = fs_mock::create_sample_file_entry();
        // Connect
        assert!(client.connect().is_ok());
        // Check session and sftp
        assert!(client.session.is_some());
        assert!(client.sftp.is_some());
        assert_eq!(client.wrkdir, PathBuf::from("/config"));
        assert_eq!(client.is_connected(), true);
        // Pwd
        assert_eq!(client.wrkdir.clone(), client.pwd().ok().unwrap());
        // Stat
        let stat = client
            .stat(Path::new("/config/sshd.pid"))
            .ok()
            .unwrap()
            .unwrap_file();
        assert_eq!(stat.name.as_str(), "sshd.pid");
        let stat = client.stat(Path::new("/config")).ok().unwrap().unwrap_dir();
        assert_eq!(stat.name.as_str(), "config");
        // Stat (err)
        assert!(client.stat(Path::new("/config/5t0ca220.log")).is_err());
        // List dir (dir has 4 (one is hidden :D) entries)
        assert!(client.list_dir(&Path::new("/config")).unwrap().len() >= 4);
        // Make directory
        assert!(client
            .create_dir(Path::new("/tmp/omar"), UnixPex::from(0o775))
            .is_ok());
        // Remake directory (should report already exists)
        assert_eq!(
            client
                .create_dir(Path::new("/tmp/omar"), UnixPex::from(0o775))
                .err()
                .unwrap()
                .code,
            RemoteErrorType::DirectoryAlreadyExists
        );
        // Make directory (err)
        assert!(client
            .create_dir(Path::new("/root/aaaaa/pommlar"), UnixPex::from(0o775))
            .is_err());
        // Change directory
        assert!(client.change_dir(Path::new("/tmp/omar")).is_ok());
        // Change directory (err)
        assert!(client.change_dir(Path::new("/tmp/oooo/aaaa/eee")).is_err());
        // Copy (not supported)
        assert!(client
            .copy(entry.abs_path.as_path(), Path::new("/"))
            .is_err());
        // Exec
        assert_eq!(client.exec("echo 5").ok().unwrap(), (0, "5\n".to_string()));
        // Upload 2 files
        let mut writable = client
            .create(Path::new("omar.txt"), &entry.metadata)
            .ok()
            .unwrap();
        fs_mock::write_file(&file, &mut writable);
        assert!(client.on_written(writable).is_ok());
        let mut writable = client
            .create(Path::new("README.md"), &entry.metadata)
            .ok()
            .unwrap();
        fs_mock::write_file(&file, &mut writable);
        assert!(client.on_written(writable).is_ok());
        // Set stat
        let metadata = client
            .stat(Path::new("README.md"))
            .ok()
            .unwrap()
            .metadata()
            .clone();
        assert!(client.setstat(Path::new("README.md"), metadata).is_ok());
        // Upload file without stream
        let reader = Box::new(StdFile::open(entry.abs_path.as_path()).ok().unwrap());
        assert!(client
            .create_file(Path::new("README2.md"), &entry.metadata, reader)
            .is_ok());
        // Upload file (err)
        assert!(client
            .create(Path::new("/ommlar/omarone"), &entry.metadata)
            .is_err());
        // List dir
        let list = client.list_dir(Path::new("/tmp/omar")).ok().unwrap();
        assert_eq!(list.len(), 3);
        // Find
        assert_eq!(client.find("*.txt").ok().unwrap().len(), 1);
        assert_eq!(client.find("*.md").ok().unwrap().len(), 2);
        assert_eq!(client.find("*.jpeg").ok().unwrap().len(), 0);
        // Rename
        assert!(client
            .create_dir(Path::new("/tmp/uploads"), UnixPex::from(0o775))
            .is_ok());
        assert!(client
            .mov(
                list.get(0).unwrap().path(),
                Path::new("/tmp/uploads/README.txt")
            )
            .is_ok());
        // Rename (err)
        assert!(client
            .mov(list.get(0).unwrap().path(), Path::new("OMARONE"))
            .is_err());
        let dummy = Entry::File(File {
            name: String::from("cucumber.txt"),
            abs_path: PathBuf::from("/cucumber.txt"),
            extension: None,
            metadata: Metadata::default(),
        });
        assert!(client.mov(&dummy.path(), Path::new("/a/b/c")).is_err());
        // Remove
        assert!(client.remove_dir_all(list.get(1).unwrap().path()).is_ok());
        assert!(client.remove_dir_all(list.get(1).unwrap().path()).is_err());
        // Receive file
        let mut writable = client
            .create(Path::new("/tmp/uploads/README.txt"), &entry.metadata)
            .ok()
            .unwrap();
        fs_mock::write_file(&file, &mut writable);
        assert!(client.on_written(writable).is_ok());
        let file = client
            .list_dir(Path::new("/tmp/uploads"))
            .ok()
            .unwrap()
            .get(0)
            .unwrap()
            .clone()
            .unwrap_file();
        let mut readable = client.open(file.abs_path.as_path()).ok().unwrap();
        let mut data: Vec<u8> = vec![0; 1024];
        assert!(readable.read(&mut data).is_ok());
        assert!(client.on_read(readable).is_ok());
        let mut dest_file = fs_mock::create_sample_file();
        // Receive file wno stream
        assert!(client
            .open_file(file.abs_path.as_path(), &mut dest_file)
            .is_ok());
        // Receive file (err)
        assert!(client.open(entry.abs_path.as_path()).is_err());
        // Cleanup
        assert!(client.change_dir(Path::new("/")).is_ok());
        assert!(client.remove_dir_all(Path::new("/tmp/omar")).is_ok());
        assert!(client.remove_dir_all(Path::new("/tmp/uploads")).is_ok());
        // Disconnect
        assert!(client.disconnect().is_ok());
        assert_eq!(client.is_connected(), false);
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
    }
}
