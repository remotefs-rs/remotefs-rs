//! ## SCP
//!
//! Scp remote fs implementation

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lazy_regex::{Lazy, Regex};
use remotefs::File;
use remotefs::fs::{
    FileType, Metadata, ReadStream, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex,
    UnixPexClass, Welcome, WriteStream,
};

use super::SshOpts;
use crate::SshSession;
use crate::utils::{fmt as fmt_utils, parser as parser_utils, path as path_utils};

/// NOTE: about this damn regex <https://stackoverflow.com/questions/32480890/is-there-a-regex-to-parse-the-values-from-an-ftp-directory-listing>
static LS_RE: Lazy<Regex> = lazy_regex!(
    r#"^(?<sym_dir>[\-ld])(?<pex>[\-rwxsStT]{9})(?<sec_ctx>\.|\+|\@)?\s+(?<n_links>\d+)\s+(?<uid>.+)\s+(?<gid>.+)\s+(?<size>\d+)\s+(?<date_time>\w{3}\s+\d{1,2}\s+(?:\d{1,2}:\d{1,2}|\d{4}))\s+(?<name>.+)$"#
);

/// Which `stat(1)` format flags the remote host accepts.
///
/// `ls -l` prints times in the server's local timezone with no offset, so we
/// use `stat` to get a timezone-free Unix epoch. The CLI flags differ between
/// GNU coreutils and BSD `stat`, so we probe once per session and cache the
/// result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatFlavor {
    /// GNU coreutils: `stat -c '<fmt>'`.
    Gnu,
    /// BSD / macOS: `stat -f '<fmt>'`.
    Bsd,
    /// Neither flavor is available; fall back to parsing `ls` output.
    Unsupported,
}

/// SCP "filesystem" client
pub struct ScpFs<S>
where
    S: SshSession,
{
    session: Option<S>,
    wrkdir: PathBuf,
    opts: SshOpts,
    /// Cached `stat(1)` flavor for the remote host; probed lazily on first use.
    stat_flavor: Option<StatFlavor>,
}

#[cfg(feature = "libssh2")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh2")))]
impl ScpFs<super::backend::LibSsh2Session> {
    /// Constructs a new [`ScpFs`] instance with the `libssh2` backend.
    pub fn libssh2(opts: SshOpts) -> Self {
        Self {
            session: None,
            wrkdir: PathBuf::from("/"),
            opts,
            stat_flavor: None,
        }
    }
}

#[cfg(feature = "libssh")]
#[cfg_attr(docsrs, doc(cfg(feature = "libssh")))]
impl ScpFs<super::backend::LibSshSession> {
    /// Constructs a new [`ScpFs`] instance with the `libssh` backend.
    pub fn libssh(opts: SshOpts) -> Self {
        Self {
            session: None,
            wrkdir: PathBuf::from("/"),
            opts,
            stat_flavor: None,
        }
    }
}

#[cfg(feature = "russh")]
#[cfg_attr(docsrs, doc(cfg(feature = "russh")))]
impl<T> ScpFs<super::backend::RusshSession<T>>
where
    T: russh::client::Handler + Default + Send + 'static,
{
    /// Constructs a new [`ScpFs`] instance with the `russh` backend.
    pub fn russh(opts: SshOpts, runtime: std::sync::Arc<tokio::runtime::Runtime>) -> Self {
        let opts = opts.runtime(runtime);
        Self {
            session: None,
            wrkdir: PathBuf::from("/"),
            opts,
            stat_flavor: None,
        }
    }
}

impl<S> ScpFs<S>
where
    S: SshSession,
{
    /// Get a reference to current `session` value.
    pub fn session(&mut self) -> Option<&mut S> {
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

    /// Parse a line of `ls -l` output and tokenize the output into a `FsFile`
    fn parse_ls_output(&self, path: &Path, line: &str) -> Result<File, ()> {
        // Prepare list regex
        trace!("Parsing LS line: '{line}'");
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

                let (is_dir, is_symlink): (bool, bool) = match &metadata["sym_dir"] {
                    "-" => (false, false),
                    "l" => (false, true),
                    "d" => (true, false),
                    _ => return Err(()), // Ignore special files
                };
                // Check string length (unix pex)
                if metadata["pex"].len() < 9 {
                    return Err(());
                }

                let pex = |range: Range<usize>| {
                    let mut count: u8 = 0;
                    for (i, c) in metadata["pex"][range].chars().enumerate() {
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
                    &metadata["date_time"],
                    "%b %d %Y",
                    "%b %d %H:%M",
                ) {
                    Ok(t) => t,
                    Err(_) => SystemTime::UNIX_EPOCH,
                };
                // Get uid
                let uid: Option<u32> = metadata["uid"].parse::<u32>().ok();
                // Get gid
                let gid: Option<u32> = metadata["gid"].parse::<u32>().ok();
                // Get filesize
                let size = metadata["size"].parse::<u64>().unwrap_or(0);
                // Get link and name
                let (file_name, symlink): (String, Option<PathBuf>) = match is_symlink {
                    true => self.get_name_and_link(&metadata["name"]),
                    false => (String::from(&metadata["name"]), None),
                };
                // Sanitize file name
                let file_name = PathBuf::from(&file_name)
                    .file_name()
                    .map(|x| x.to_string_lossy().to_string())
                    .unwrap_or(file_name);
                // Check if file_name is '.' or '..'
                if file_name.as_str() == "." || file_name.as_str() == ".." {
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
                    accessed: None,
                    created: None,
                    file_type,
                    gid,
                    mode: Some(mode),
                    modified: Some(modified),
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
        let filename: String = String::from(*tokens.first().unwrap());
        let symlink: Option<PathBuf> = tokens.get(1).map(PathBuf::from);
        (filename, symlink)
    }

    /// Execute setstat command and assert result is 0
    fn assert_stat_command(&mut self, cmd: String) -> RemoteResult<()> {
        match self.session.as_mut().unwrap().cmd(cmd) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::StatFailed)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    /// Returns whether file at `path` is a directory
    fn is_directory(&mut self, path: &Path) -> RemoteResult<bool> {
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("test -d \"{}\"", path.display()))
        {
            Ok((0, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
        }
    }

    /// Detect which `stat(1)` flavor the remote host supports.
    ///
    /// Probes GNU first (`stat --version`, which BSD rejects) then BSD (`stat
    /// -f %m /`). The result is cached on the session; callers pay at most
    /// one extra roundtrip per connection. Returns
    /// [`StatFlavor::Unsupported`] when neither flavor works so the caller
    /// can fall back to the `ls`-based parser.
    fn stat_flavor(&mut self) -> StatFlavor {
        if let Some(flavor) = self.stat_flavor {
            return flavor;
        }
        let session = self.session.as_mut().unwrap();
        let flavor = match session.cmd("stat --version >/dev/null 2>&1") {
            Ok((0, _)) => StatFlavor::Gnu,
            _ => match session.cmd("stat -f %m / >/dev/null 2>&1") {
                Ok((0, _)) => StatFlavor::Bsd,
                _ => StatFlavor::Unsupported,
            },
        };
        trace!("Detected remote stat flavor: {flavor:?}");
        self.stat_flavor = Some(flavor);
        flavor
    }

    /// Fetch the Unix epoch mtime of a single `path` via `stat`.
    ///
    /// Returns [`None`] when the remote host exposes neither GNU nor BSD
    /// `stat`, or when the command fails.
    fn mtime_epoch(&mut self, path: &Path) -> Option<SystemTime> {
        let flag = match self.stat_flavor() {
            StatFlavor::Gnu => "-c %Y",
            StatFlavor::Bsd => "-f %m",
            StatFlavor::Unsupported => return None,
        };
        let cmd = format!("stat {} \"{}\"", flag, path.display());
        match self.session.as_mut().unwrap().cmd(cmd) {
            Ok((0, output)) => parser_utils::parse_stat_epoch(&output),
            _ => None,
        }
    }

    /// Fetch the Unix epoch mtime for every entry in `entries` under `dir`
    /// with a single batched `stat` invocation.
    ///
    /// Returns a map from basename to [`SystemTime`]. Entries missing from
    /// the map should fall back to the `ls`-parsed timestamp.
    fn mtimes_in_dir(&mut self, dir: &Path, entries: &[&str]) -> HashMap<String, SystemTime> {
        if entries.is_empty() {
            return HashMap::new();
        }
        let fmt = match self.stat_flavor() {
            StatFlavor::Gnu => "-c '%Y %n'",
            StatFlavor::Bsd => "-f '%m %N'",
            StatFlavor::Unsupported => return HashMap::new(),
        };
        let args: Vec<String> = entries
            .iter()
            .map(|name| format!("\"{}\"", dir.join(name).display()))
            .collect();
        let cmd = format!("stat {} {}", fmt, args.join(" "));
        match self.session.as_mut().unwrap().cmd(cmd) {
            Ok((_, output)) => parser_utils::parse_stat_listing(&output),
            Err(err) => {
                warn!("Batched stat failed, falling back to ls timestamps: {err}");
                HashMap::new()
            }
        }
    }
}

impl<S> RemoteFs for ScpFs<S>
where
    S: SshSession,
{
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("Initializing SFTP connection...");
        let mut session = S::connect(&self.opts)?;
        // Get banner
        let banner = session.banner()?;
        debug!(
            "Connection established: {}",
            banner.as_deref().unwrap_or("")
        );
        // Get working directory
        debug!("Getting working directory...");
        self.wrkdir = session
            .cmd("pwd")
            .map(|(_rc, output)| PathBuf::from(output.as_str().trim()))?;
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
            match session.disconnect() {
                Ok(_) => {
                    // Set session and sftp to none
                    self.session = None;
                    // Drop cached probe so a fresh connection re-detects.
                    self.stat_flavor = None;
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
            .map(|x| x.authenticated().unwrap_or_default())
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
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("cd \"{}\"; echo $?; pwd", dir.display()))
        {
            Ok((rc, output)) => {
                if rc != 0 {
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Failed to change directory: {}", output),
                    ));
                }
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
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("unset LANG; ls -la \"{}/\"", path.display()).as_str())
        {
            Ok((rc, output)) => {
                if rc != 0 {
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::ProtocolError,
                        format!("Failed to list directory: {}", output),
                    ));
                }
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
                // Override `ls`-parsed mtimes (which are TZ-ambiguous) with
                // the server's Unix epoch via `stat`. One batched roundtrip.
                let names: Vec<String> = entries.iter().map(|e| e.name()).collect();
                let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let mtimes = self.mtimes_in_dir(path.as_path(), &name_refs);
                for (entry, name) in entries.iter_mut().zip(names.iter()) {
                    if let Some(t) = mtimes.get(name) {
                        entry.metadata.modified = Some(*t);
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
        match self.session.as_mut().unwrap().cmd(cmd.as_str()) {
            Ok((rc, line)) => {
                if rc != 0 {
                    return Err(RemoteError::new_ex(
                        RemoteErrorType::NoSuchFileOrDirectory,
                        format!("Failed to stat file: {line}"),
                    ));
                }
                // Parse ls line
                let parent: PathBuf = match path.as_path().parent() {
                    Some(p) => PathBuf::from(p),
                    None => {
                        return Err(RemoteError::new_ex(
                            RemoteErrorType::StatFailed,
                            "Path has no parent",
                        ));
                    }
                };
                match self.parse_ls_output(parent.as_path(), line.as_str().trim()) {
                    Ok(mut entry) => {
                        // Override TZ-ambiguous `ls` mtime with Unix epoch.
                        if let Some(t) = self.mtime_epoch(path.as_path()) {
                            entry.metadata.modified = Some(t);
                        }
                        Ok(entry)
                    }
                    Err(_) => Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)),
                }
            }
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("test -e \"{}\"", path.display()))
        {
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
                metadata.gid.map(|x| format!(":{x}")).unwrap_or_default(),
                path.display()
            ))?;
        }
        // set times
        if let Some(accessed) = metadata.accessed {
            self.assert_stat_command(format!(
                "touch -a -t {} \"{}\"",
                fmt_utils::fmt_time_utc(accessed, "%Y%m%d%H%M.%S"),
                path.display()
            ))?;
        }
        if let Some(modified) = metadata.modified {
            self.assert_stat_command(format!(
                "touch -m -t {} \"{}\"",
                fmt_utils::fmt_time_utc(modified, "%Y%m%d%H%M.%S"),
                path.display()
            ))?;
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        if !self.exists(path.as_path()).ok().unwrap_or(false) {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        debug!("Removing file {}", path.display());
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("rm -f \"{}\"", path.display()))
        {
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
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("rmdir \"{}\"", path.display()))
        {
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
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("rm -rf \"{}\"", path.display()))
        {
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
        match self.session.as_mut().unwrap().cmd(format!(
            "mkdir -m {} \"{}\"",
            mode,
            path.display()
        )) {
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
        match self.session.as_mut().unwrap().cmd(format!(
            "ln -s \"{}\" \"{}\"",
            target.display(),
            path.display()
        )) {
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
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("cp -rf \"{}\" \"{}\"", src.display(), dest.display()).as_str())
        {
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
        match self
            .session
            .as_mut()
            .unwrap()
            .cmd(format!("mv -f \"{}\" \"{}\"", src.display(), dest.display()).as_str())
        {
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
        debug!(r#"Executing command "{cmd}""#);
        self.session
            .as_mut()
            .unwrap()
            .cmd_at(cmd, self.wrkdir.as_path())
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<WriteStream> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        debug!("Creating file {}", path.display());
        trace!("blocked channel");
        let mode = metadata.mode.map(u32::from).unwrap_or(0o644) as i32;
        let accessed = metadata
            .accessed
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let modified = metadata
            .modified
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .unwrap_or(Duration::ZERO)
            .as_secs();
        trace!("Creating file with mode {mode:o}, accessed: {accessed}, modified: {modified}");
        match self.session.as_mut().unwrap().scp_send(
            path.as_path(),
            mode,
            metadata.size,
            Some((modified, accessed)),
        ) {
            Ok(channel) => Ok(WriteStream::from(channel)),
            Err(err) => {
                error!("Failed to create file: {err}");
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
        trace!("blocked channel");
        match self.session.as_mut().unwrap().scp_recv(path.as_path()) {
            Ok(channel) => Ok(ReadStream::from(channel)),
            Err(err) => {
                error!("Failed to open file: {err}");
                Err(RemoteError::new_ex(RemoteErrorType::CouldNotOpenFile, err))
            }
        }
    }
}

#[cfg(test)]
mod tests;
