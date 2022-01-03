//! ## SCP
//!
//! Scp remote fs implementation

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
use crate::utils::fmt as fmt_utils;
use crate::utils::parser as parser_utils;
use crate::utils::path as path_utils;
use remotefs::fs::{
    FileType, Metadata, ReadStream, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex,
    UnixPexClass, Welcome, WriteStream,
};
use remotefs::File;

use regex::Regex;
use std::io::{BufReader, BufWriter, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

// -- export
pub use ssh2::Session as SshSession;

/// SCP "filesystem" client
pub struct ScpFs {
    session: Option<SshSession>,
    wrkdir: PathBuf,
    opts: SshOpts,
}

impl ScpFs {
    /// Creates a new `SftpFs`
    pub fn new(opts: SshOpts) -> Self {
        Self {
            session: None,
            wrkdir: PathBuf::from("/"),
            opts,
        }
    }

    /// Get a reference to current `session` value.
    pub fn session(&mut self) -> Option<&mut SshSession> {
        self.session.as_mut()
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

    /// ### parse_ls_output
    ///
    /// Parse a line of `ls -l` output and tokenize the output into a `FsFile`
    fn parse_ls_output(&self, path: &Path, line: &str) -> Result<File, ()> {
        // Prepare list regex
        // NOTE: about this damn regex <https://stackoverflow.com/questions/32480890/is-there-a-regex-to-parse-the-values-from-an-ftp-directory-listing>
        lazy_static! {
            static ref LS_RE: Regex = Regex::new(r#"^([\-ld])([\-rwxs]{9})\s+(\d+)\s+(.+)\s+(.+)\s+(\d+)\s+(\w{3}\s+\d{1,2}\s+(?:\d{1,2}:\d{1,2}|\d{4}))\s+(.+)$"#).unwrap();
        }
        trace!("Parsing LS line: '{}'", line);
        // Apply regex to result
        match LS_RE.captures(line) {
            // String matches regex
            Some(metadata) => {
                // NOTE: metadata fmt: (regex, file_type, permissions, link_count, uid, gid, filesize, modified, filename)
                // Expected 7 + 1 (8) values: + 1 cause regex is repeated at 0
                if metadata.len() < 8 {
                    return Err(());
                }
                // Collect metadata
                // Get if is directory and if is symlink
                let (is_dir, is_symlink): (bool, bool) = match metadata.get(1).unwrap().as_str() {
                    "-" => (false, false),
                    "l" => (false, true),
                    "d" => (true, false),
                    _ => return Err(()), // Ignore special files
                };
                // Check string length (unix pex)
                if metadata.get(2).unwrap().as_str().len() < 9 {
                    return Err(());
                }

                let pex = |range: Range<usize>| {
                    let mut count: u8 = 0;
                    for (i, c) in metadata.get(2).unwrap().as_str()[range].chars().enumerate() {
                        match c {
                            '-' => {}
                            _ => {
                                count += match i {
                                    0 => 4,
                                    1 => 2,
                                    2 => 1,
                                    _ => 0,
                                }
                            }
                        }
                    }
                    count
                };

                // Get unix pex
                let mode = UnixPex::new(
                    UnixPexClass::from(pex(0..3)),
                    UnixPexClass::from(pex(3..6)),
                    UnixPexClass::from(pex(6..9)),
                );

                // Parse modified and convert to SystemTime
                let modified: SystemTime = match parser_utils::parse_lstime(
                    metadata.get(7).unwrap().as_str(),
                    "%b %d %Y",
                    "%b %d %H:%M",
                ) {
                    Ok(t) => t,
                    Err(_) => SystemTime::UNIX_EPOCH,
                };
                // Get uid
                let uid: Option<u32> = match metadata.get(4).unwrap().as_str().parse::<u32>() {
                    Ok(uid) => Some(uid),
                    Err(_) => None,
                };
                // Get gid
                let gid: Option<u32> = match metadata.get(5).unwrap().as_str().parse::<u32>() {
                    Ok(gid) => Some(gid),
                    Err(_) => None,
                };
                // Get filesize
                let size = metadata
                    .get(6)
                    .unwrap()
                    .as_str()
                    .parse::<u64>()
                    .unwrap_or(0);
                // Get link and name
                let (file_name, symlink): (String, Option<PathBuf>) = match is_symlink {
                    true => self.get_name_and_link(metadata.get(8).unwrap().as_str()),
                    false => (String::from(metadata.get(8).unwrap().as_str()), None),
                };
                // Sanitize file name
                let file_name = PathBuf::from(&file_name)
                    .file_name()
                    .map(|x| x.to_string_lossy().to_string())
                    .unwrap_or(file_name);
                // Check if file_name is '.' or '..'
                if file_name.as_str() == "." || file_name.as_str() == ".." {
                    debug!("File name is {}; ignoring entry", file_name);
                    return Err(());
                }
                // Re-check if is directory
                let mut path: PathBuf = path.to_path_buf();
                path.push(file_name.as_str());
                // get file type
                let file_type = if symlink.is_some() {
                    FileType::Symlink
                } else if is_dir {
                    FileType::Directory
                } else {
                    FileType::File
                };
                // make metadata
                let metadata = Metadata {
                    accessed: SystemTime::UNIX_EPOCH,
                    created: SystemTime::UNIX_EPOCH,
                    file_type,
                    gid,
                    mode: Some(mode),
                    modified,
                    size,
                    symlink,
                    uid,
                };
                trace!(
                    "Found entry at {} with metadata {:?}",
                    path.display(),
                    metadata
                );
                // Push to entries
                Ok(File { path, metadata })
            }
            None => Err(()),
        }
    }

    /// ### get_name_and_link
    ///
    /// Returns from a `ls -l` command output file name token, the name of the file and the symbolic link (if there is any)
    fn get_name_and_link(&self, token: &str) -> (String, Option<PathBuf>) {
        let tokens: Vec<&str> = token.split(" -> ").collect();
        let filename: String = String::from(*tokens.get(0).unwrap());
        let symlink: Option<PathBuf> = tokens.get(1).map(PathBuf::from);
        (filename, symlink)
    }

    /// Execute setstat command and assert result is 0
    fn assert_stat_command(&mut self, cmd: String) -> RemoteResult<()> {
        match commons::perform_shell_cmd_with_rc(self.session.as_mut().unwrap(), cmd) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::StatFailed)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    /// Returns whether file at `path` is a directory
    fn is_directory(&mut self, path: &Path) -> RemoteResult<bool> {
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("test -d \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
        }
    }
}

impl RemoteFs for ScpFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("Initializing SFTP connection...");
        let mut session = commons::connect(&self.opts)?;
        // Get banner
        let banner: Option<String> = session.banner().map(String::from);
        debug!(
            "Connection established: {}",
            banner.as_deref().unwrap_or("")
        );
        // Get working directory
        debug!("Getting working directory...");
        self.wrkdir = commons::perform_shell_cmd(&mut session, "pwd")
            .map(|x| PathBuf::from(x.as_str().trim()))?;
        // Set session
        self.session = Some(session);
        info!(
            "Connection established; working directory: {}",
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
        debug!("Changing working directory to {}", dir.display());
        match commons::perform_shell_cmd(
            self.session.as_mut().unwrap(),
            format!("cd \"{}\"; echo $?; pwd", dir.display()),
        ) {
            Ok(output) => {
                // Trim
                let output: String = String::from(output.as_str().trim());
                // Check if output starts with 0; should be 0{PWD}
                match output.as_str().starts_with('0') {
                    true => {
                        // Set working directory
                        self.wrkdir = PathBuf::from(&output.as_str()[1..].trim());
                        debug!("Changed working directory to {}", self.wrkdir.display());
                        Ok(self.wrkdir.clone())
                    }
                    false => Err(RemoteError::new_ex(
                        // No such file or directory
                        RemoteErrorType::NoSuchFileOrDirectory,
                        format!("\"{}\"", dir.display()),
                    )),
                }
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Getting file entries in {}", path.display());
        // check if exists
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        match commons::perform_shell_cmd(
            self.session.as_mut().unwrap(),
            format!("unset LANG; ls -la \"{}/\"", path.display()).as_str(),
        ) {
            Ok(output) => {
                // Split output by (\r)\n
                let lines: Vec<&str> = output.as_str().lines().collect();
                let mut entries: Vec<File> = Vec::with_capacity(lines.len());
                for line in lines.iter() {
                    // First line must always be ignored
                    // Parse row, if ok push to entries
                    if let Ok(entry) = self.parse_ls_output(path.as_path(), line) {
                        entries.push(entry);
                    }
                }
                debug!(
                    "Found {} out of {} valid file entries",
                    entries.len(),
                    lines.len()
                );
                Ok(entries)
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Stat {}", path.display());
        // make command; Directories require `-d` option
        let cmd = match self.is_directory(path.as_path())? {
            true => format!("ls -ld \"{}\"", path.display()),
            false => format!("ls -l \"{}\"", path.display()),
        };
        match commons::perform_shell_cmd(self.session.as_mut().unwrap(), cmd.as_str()) {
            Ok(line) => {
                // Parse ls line
                let parent: PathBuf = match path.as_path().parent() {
                    Some(p) => PathBuf::from(p),
                    None => {
                        return Err(RemoteError::new_ex(
                            RemoteErrorType::StatFailed,
                            "Path has no parent",
                        ))
                    }
                };
                match self.parse_ls_output(parent.as_path(), line.as_str().trim()) {
                    Ok(entry) => Ok(entry),
                    Err(_) => Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)),
                }
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("test -e \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
        }
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Setting attributes for {}", path.display());
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        // set mode with chmod
        if let Some(mode) = metadata.mode {
            self.assert_stat_command(format!(
                "chmod {:o} \"{}\"",
                u32::from(mode),
                path.display()
            ))?;
        }
        if let Some(user) = metadata.uid {
            self.assert_stat_command(format!(
                "chown {}{} \"{}\"",
                user,
                metadata.gid.map(|x| format!(":{}", x)).unwrap_or_default(),
                path.display()
            ))?;
        }
        // set times
        self.assert_stat_command(format!(
            "touch -a -t {} \"{}\"",
            fmt_utils::fmt_time_utc(metadata.accessed, "%Y%m%d%H%M.%S"),
            path.display()
        ))?;
        self.assert_stat_command(format!(
            "touch -m -t {} \"{}\"",
            fmt_utils::fmt_time_utc(metadata.modified, "%Y%m%d%H%M.%S"),
            path.display()
        ))
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing file {}", path.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("rm -f \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing directory {}", path.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("rmdir \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing directory {} recursively", path.display());
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("rm -rf \"{}\"", path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::DirectoryAlreadyExists));
        }
        let mode = format!("{:o}", u32::from(mode));
        debug!(
            "Creating directory at {} with mode {}",
            path.display(),
            mode
        );
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("mkdir -m {} \"{}\"", mode, path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::FileCreateDenied)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!(
            "Creating a symlink at {} pointing at {}",
            path.display(),
            target.display()
        );
        if !self.exists(target).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        if self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::FileCreateDenied));
        }
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("ln -s \"{}\" \"{}\"", target.display(), path.display()),
        ) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::FileCreateDenied)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
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
            Err(err) => Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                err.to_string(),
            )),
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
        match commons::perform_shell_cmd_with_rc(
            self.session.as_mut().unwrap(),
            format!("mv -f \"{}\" \"{}\"", src.display(), dest.display()).as_str(),
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

    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.check_connection()?;
        debug!(r#"Executing command "{}""#, cmd);
        commons::perform_shell_cmd_at_with_rc(
            self.session.as_mut().unwrap(),
            cmd,
            self.wrkdir.as_path(),
        )
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Creating file {}", path.display());
        // blocking channel
        self.session.as_mut().unwrap().set_blocking(true);
        trace!("blocked channel");
        let mode = metadata.mode.map(u32::from).unwrap_or(0o644) as i32;
        let accessed = metadata
            .accessed
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let modified = metadata
            .modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        trace!(
            "Creating file with mode {:o}, accessed: {}, modified: {}",
            mode,
            accessed,
            modified
        );
        match self.session.as_mut().unwrap().scp_send(
            path.as_path(),
            mode,
            metadata.size,
            Some((modified, accessed)),
        ) {
            Ok(channel) => Ok(WriteStream::from(
                Box::new(BufWriter::with_capacity(65536, channel)) as Box<dyn Write>,
            )),
            Err(err) => {
                error!("Failed to create file: {}", err);
                Err(RemoteError::new_ex(RemoteErrorType::FileCreateDenied, err))
            }
        }
    }

    fn open(&mut self, path: &Path) -> RemoteResult<ReadStream> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Opening file {} for read", path.display());
        // check if file exists
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        self.session.as_mut().unwrap().set_blocking(true);
        trace!("blocked channel");
        match self.session.as_mut().unwrap().scp_recv(path.as_path()) {
            Ok((channel, _)) => Ok(ReadStream::from(Box::new(BufReader::with_capacity(
                65536, channel,
            )) as Box<dyn Read>)),
            Err(err) => {
                error!("Failed to open file: {}", err);
                Err(RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, err))
            }
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    #[cfg(feature = "with-containers")]
    use crate::mock::ssh as ssh_mock;

    use pretty_assertions::assert_eq;
    #[cfg(feature = "with-containers")]
    use serial_test::serial;
    #[cfg(feature = "with-containers")]
    use std::io::Cursor;

    #[test]
    fn should_init_scp_fs() {
        let mut client = ScpFs::new(SshOpts::new("localhost"));
        assert!(client.session.is_none());
        assert_eq!(client.is_connected(), false);
    }

    #[test]
    fn should_fail_connection_to_bad_server() {
        let mut client = ScpFs::new(SshOpts::new("mybad.verybad.server"));
        assert!(client.connect().is_err());
    }

    #[test]
    #[cfg(feature = "with-containers")]
    #[serial]
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_err());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client.open_file(p, buffer).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());

        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: SystemTime::UNIX_EPOCH,
                    created: SystemTime::UNIX_EPOCH,
                    file_type: FileType::File,
                    gid: Some(1000),
                    mode: Some(UnixPex::from(0o755)),
                    modified: SystemTime::UNIX_EPOCH,
                    size: 7,
                    symlink: None,
                    uid: Some(1000),
                }
            )
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        let stat = entry.metadata();
        assert_eq!(stat.accessed, SystemTime::UNIX_EPOCH);
        assert_eq!(stat.created, SystemTime::UNIX_EPOCH);
        assert_eq!(stat.modified, SystemTime::UNIX_EPOCH);
        assert_eq!(stat.mode.unwrap(), UnixPex::from(0o755));
        assert_eq!(stat.size, 7);

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
                    accessed: SystemTime::UNIX_EPOCH,
                    created: SystemTime::UNIX_EPOCH,
                    file_type: FileType::File,
                    gid: Some(1),
                    mode: Some(UnixPex::from(0o755)),
                    modified: SystemTime::UNIX_EPOCH,
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
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
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        let symlink = Path::new("b.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(symlink, &metadata, Box::new(reader))
            .is_ok());
        assert!(client.symlink(symlink, p).is_err());
        assert!(client.remove_file(symlink).is_ok());
        assert!(client.symlink(symlink, Path::new("c.sh")).is_err());
        finalize_client(client);
    }

    #[test]
    fn should_get_name_and_link() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        assert_eq!(
            client.get_name_and_link("Cargo.toml"),
            (String::from("Cargo.toml"), None)
        );
        assert_eq!(
            client.get_name_and_link("Cargo -> Cargo.toml"),
            (String::from("Cargo"), Some(PathBuf::from("Cargo.toml")))
        );
    }

    #[test]
    fn should_parse_file_ls_output() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        // File
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "-rw-r--r-- 1 root root  2056 giu 13 21:11 /tmp/Cargo.toml",
            )
            .ok()
            .unwrap();
        assert_eq!(entry.name().as_str(), "Cargo.toml");
        assert!(entry.is_file());
        assert_eq!(entry.path, PathBuf::from("/tmp/Cargo.toml"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o644_u32);
        assert_eq!(entry.metadata.size, 2056);
        assert_eq!(entry.extension().unwrap().as_str(), "toml");
        assert!(entry.metadata.symlink.is_none());
        // File (year)
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "-rw-rw-rw- 1 root root  3368 nov  7  2020 CODE_OF_CONDUCT.md",
            )
            .ok()
            .unwrap();
        assert_eq!(entry.name().as_str(), "CODE_OF_CONDUCT.md");
        assert!(entry.is_file());
        assert_eq!(entry.path, PathBuf::from("/tmp/CODE_OF_CONDUCT.md"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o666_u32);
        assert_eq!(entry.metadata.size, 3368);
        assert_eq!(entry.extension().unwrap().as_str(), "md");
        assert!(entry.metadata.symlink.is_none());
    }

    #[test]
    fn should_parse_directory_from_ls_output() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        // Directory
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "drwxr-xr-x 1 root root   512 giu 13 21:11 docs",
            )
            .ok()
            .unwrap();
        assert_eq!(entry.name().as_str(), "docs");
        assert!(entry.is_dir());
        assert_eq!(entry.path, PathBuf::from("/tmp/docs"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o755_u32);
        assert!(entry.metadata.symlink.is_none());
        // Short metadata
        assert!(client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "drwxr-xr-x 1 root root   512 giu 13 21:11",
            )
            .is_err());
        // Special file
        assert!(client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "crwxr-xr-x 1 root root   512 giu 13 21:11 ttyS1",
            )
            .is_err());
        // Bad pex
        assert!(client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "-rwxr-xr 1 root root   512 giu 13 21:11 ttyS1",
            )
            .is_err());
    }

    #[test]
    fn should_parse_symlink_from_ls_output() {
        let client = ScpFs::new(SshOpts::new("localhost"));
        // File
        let entry = client
            .parse_ls_output(
                PathBuf::from("/tmp").as_path(),
                "lrw-r--r-- 1 root root  2056 giu 13 21:11 Cargo.toml -> Cargo.prod.toml",
            )
            .ok()
            .unwrap();
        assert_eq!(entry.name().as_str(), "Cargo.toml");
        assert_eq!(entry.path, PathBuf::from("/tmp/Cargo.toml"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o644_u32);
        assert_eq!(entry.metadata.size, 2056);
        assert_eq!(entry.extension().unwrap().as_str(), "toml");
        assert_eq!(
            entry.metadata.symlink.as_deref().unwrap(),
            Path::new("Cargo.prod.toml")
        );
    }

    #[test]
    fn should_return_errors_on_uninitialized_client() {
        let mut client = ScpFs::new(SshOpts::new("localhost"));
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
        assert!(client.symlink(Path::new("/a"), Path::new("/b")).is_err());
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
    fn setup_client() -> ScpFs {
        let config_file = ssh_mock::create_ssh_config();
        let mut client = ScpFs::new(
            SshOpts::new("scp")
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
    fn finalize_client(mut client: ScpFs) {
        // Get working directory
        let wrkdir = client.pwd().ok().unwrap();
        // Remove directory
        assert!(client.remove_dir_all(wrkdir.as_path()).is_ok());
        assert!(client.disconnect().is_ok());
    }

    #[cfg(feature = "with-containers")]
    fn generate_tempdir() -> String {
        use rand::{distributions::Alphanumeric, thread_rng, Rng};
        let mut rng = thread_rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("/tmp/temp_{}", name)
    }
}
