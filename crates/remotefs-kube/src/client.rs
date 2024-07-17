//! ## SCP
//!
//! Scp remote fs implementation

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use futures_util::StreamExt as _;
use k8s_openapi::api::core::v1::Pod;
use kube::api::AttachParams;
use kube::{Api, Client, Config};
use lazy_regex::{Lazy, Regex};
use remotefs::fs::{
    FileType, Metadata, ReadStream, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex,
    UnixPexClass, Welcome, WriteStream,
};
use remotefs::File;
use tokio::io::AsyncWriteExt as _;
use tokio::runtime::Runtime;

use crate::utils::{fmt as fmt_utils, parser as parser_utils, path as path_utils};

/// NOTE: about this damn regex <https://stackoverflow.com/questions/32480890/is-there-a-regex-to-parse-the-values-from-an-ftp-directory-listing>
static LS_RE: Lazy<Regex> = lazy_regex!(
    r#"^([\-ld])([\-rwxsStT]{9})\s+(\d+)\s+(.+)\s+(.+)\s+(\d+)\s+(\w{3}\s+\d{1,2}\s+(?:\d{1,2}:\d{1,2}|\d{4}))\s+(.+)$"#
);

/// Kube "filesystem" client
pub struct KubeFs {
    config: Option<Config>,
    container: String,
    pod_name: String,
    pods: Option<Api<Pod>>,
    runtime: Arc<Runtime>,
    wrkdir: PathBuf,
}

impl KubeFs {
    /// Creates a new `KubeFs`
    ///
    /// If `config()` is not called then, it will try to use the configuration from the default kubeconfig file
    pub fn new(pod_name: impl ToString, container: impl ToString, runtime: &Arc<Runtime>) -> Self {
        Self {
            config: None,
            container: container.to_string(),
            pod_name: pod_name.to_string(),
            pods: None,
            runtime: runtime.clone(),
            wrkdir: PathBuf::from("/"),
        }
    }

    /// Set configuration
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
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

    /// Perform shell cmd at path and return output and return code
    fn shell_cmd_at_with_rc(
        &self,
        cmd: impl std::fmt::Display,
        path: &Path,
    ) -> RemoteResult<(u32, String)> {
        const STDOUT_SIZE: usize = 2048;

        let shell_cmd = format!(r#"cd {} && {}; echo -n ";$?""#, path.display(), cmd);
        debug!("Executing shell command: {}", shell_cmd);

        self.runtime.block_on(async {
            let attach_params = AttachParams::default()
                .stdout(true)
                .stdin(false)
                .stderr(true)
                .container(self.container.clone())
                .max_stdout_buf_size(STDOUT_SIZE);

            let mut process = self
                .pods
                .as_ref()
                .unwrap()
                .exec(
                    &self.pod_name,
                    vec!["/bin/sh", "-c", &shell_cmd],
                    &attach_params,
                )
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            let stdout_reader =
                tokio_util::io::ReaderStream::new(process.stdout().ok_or_else(|| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, "failed to read stdout")
                })?);

            let stdout = stdout_reader
                .filter_map(|r| async { r.ok().and_then(|v| String::from_utf8(v.to_vec()).ok()) })
                .collect::<Vec<_>>()
                .await
                .join("");

            // if level is debug print stderr
            if log::log_enabled!(log::Level::Debug) {
                let stderr_reader =
                    tokio_util::io::ReaderStream::new(process.stderr().ok_or_else(|| {
                        RemoteError::new_ex(RemoteErrorType::ProtocolError, "failed to read stderr")
                    })?);

                let stderr = stderr_reader
                    .filter_map(|r| async {
                        r.ok().and_then(|v| String::from_utf8(v.to_vec()).ok())
                    })
                    .collect::<Vec<_>>()
                    .await
                    .join("");
                debug!("Shell command stderr: {stderr}",);
            }

            process.join().await.map_err(|err| {
                RemoteError::new_ex(RemoteErrorType::ProtocolError, err.to_string())
            })?;

            // collect rc from stdout
            // count the number of tokens
            let token_count = stdout.chars().filter(|c| *c == ';').count();
            let mut tokens = stdout.split(';');
            // stdout is all tokens, except the last one
            let stdout = tokens
                .by_ref()
                .take(token_count)
                .collect::<Vec<&str>>()
                .join(";");
            // last token is the return code
            let rc = tokens
                .next()
                .ok_or_else(|| RemoteError::new(RemoteErrorType::ProtocolError))?
                .parse::<u32>()
                .map_err(|_| RemoteError::new(RemoteErrorType::ProtocolError))?;

            debug!("Shell command exit code: {rc}",);
            debug!("Shell command output: {stdout}");

            Ok((rc, stdout))
        })
    }

    /// Perform shell cmd and return output and return code
    fn shell_cmd_with_rc(&self, cmd: impl std::fmt::Display) -> RemoteResult<(u32, String)> {
        self.shell_cmd_at_with_rc(cmd, &self.wrkdir)
    }

    /// Perform shell cmd and return output
    fn shell_cmd(&self, cmd: impl std::fmt::Display) -> RemoteResult<String> {
        self.shell_cmd_with_rc(cmd).map(|(_, output)| output)
    }

    /// Returns from a `ls -l` command output file name token, the name of the file and the symbolic link (if there is any)
    fn get_name_and_link(&self, token: &str) -> (String, Option<PathBuf>) {
        let tokens: Vec<&str> = token.split(" -> ").collect();
        let filename: String = String::from(*tokens.first().unwrap());
        let symlink: Option<PathBuf> = tokens.get(1).map(PathBuf::from);
        (filename, symlink)
    }

    /// Execute setstat command and assert result is 0
    fn assert_stat_command(&mut self, cmd: String) -> RemoteResult<()> {
        match self.shell_cmd_with_rc(cmd) {
            Ok((0, _)) => Ok(()),
            Ok(_) => Err(RemoteError::new(RemoteErrorType::StatFailed)),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err)),
        }
    }

    /// Returns whether file at `path` is a directory
    fn is_directory(&mut self, path: &Path) -> RemoteResult<bool> {
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        match self.shell_cmd_with_rc(format!("test -d \"{}\"", path.display())) {
            Ok((0, _)) => Ok(true),
            Ok(_) => Ok(false),
            Err(err) => Err(RemoteError::new_ex(RemoteErrorType::StatFailed, err)),
        }
    }
}

impl RemoteFs for KubeFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("Initializing Kube connection...");
        let api = self.runtime.block_on(async {
            let client = match self.config.as_ref() {
                Some(config) => Client::try_from(config.clone())
                    .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err)),
                None => Client::try_default()
                    .await
                    .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err)),
            }?;
            let api: Api<Pod> = Api::default_namespaced(client);

            if api.get(&self.pod_name).await.is_err() {
                Err(RemoteError::new(RemoteErrorType::ConnectionError))
            } else {
                Ok(api)
            }
        })?;

        debug!("Connection established with pod {}", self.pod_name);
        // Set pods
        self.pods = Some(api);
        debug!("Getting working directory...");
        // Get working directory
        let wrkdir = self.shell_cmd("pwd")?;
        if !wrkdir.starts_with('/') {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                format!("bad pwd response: {wrkdir}"),
            ));
        }
        self.wrkdir = PathBuf::from(wrkdir.trim());
        info!(
            "Connection established; working directory: {}",
            self.wrkdir.display()
        );
        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        if self.pods.is_none() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }

        debug!("Disconnecting from remote...");
        self.pods = None;

        info!("Disconnected from remote");
        Ok(())
    }

    fn is_connected(&mut self) -> bool {
        if let Some(pods) = self.pods.as_ref() {
            self.runtime
                .block_on(async { pods.get_status(&self.pod_name).await.is_ok() })
        } else {
            false
        }
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        Ok(self.wrkdir.clone())
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        self.check_connection()?;
        let dir = path_utils::absolutize(self.wrkdir.as_path(), dir);
        debug!("Changing working directory to {}", dir.display());
        match self.shell_cmd(format!("cd \"{}\"; echo $?; pwd", dir.display())) {
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
        match self.shell_cmd(format!("ls -la \"{}/\"", path.display()).as_str()) {
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
        match self.shell_cmd(cmd.as_str()) {
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
        match self.shell_cmd_with_rc(format!("test -e \"{}\"", path.display())) {
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
        match self.shell_cmd_with_rc(format!("rm -f \"{}\"", path.display())) {
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
        match self.shell_cmd_with_rc(format!("rmdir \"{}\"", path.display())) {
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
        match self.shell_cmd_with_rc(format!("rm -rf \"{}\"", path.display())) {
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
        match self.shell_cmd_with_rc(format!("mkdir -m {} \"{}\"", mode, path.display())) {
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
        match self.shell_cmd_with_rc(format!(
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
        match self.shell_cmd_with_rc(
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
        match self.shell_cmd_with_rc(
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
        self.shell_cmd_at_with_rc(cmd, self.wrkdir.as_path())
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
        metadata: &Metadata,
        reader: Box<dyn std::io::Read>,
    ) -> RemoteResult<u64> {
        self.check_connection()?;
        let path = path_utils::absolutize(self.wrkdir.as_path(), path);
        let file_name = path
            .file_name()
            .ok_or(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?;
        let tar_path = PathBuf::from(file_name);
        // prepare write
        let mut header = tar::Header::new_gnu();
        header
            .set_path(tar_path)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err))?;
        header.set_size(metadata.size);
        header.set_cksum();

        debug!("preparing archive to upload");
        let mut ar = tar::Builder::new(Vec::new());
        debug!("appending data to archive");
        ar.append(&header, reader)
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err))?;
        debug!("uploading archive to kube at: {}", path.display());

        let data = ar
            .into_inner()
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err))?;

        let dir_path = path.parent().unwrap_or(Path::new("/"));
        debug!("uploading archive to kube in dir: {}", dir_path.display());

        let size = self.runtime.block_on(async {
            let attach_params = AttachParams::default()
                .container(self.container.clone())
                .stdin(true)
                .stderr(false);
            let mut cmd = self
                .pods
                .as_ref()
                .unwrap()
                .exec(
                    &self.pod_name,
                    vec!["tar", "xf", "-", "-C", &dir_path.display().to_string()],
                    &attach_params,
                )
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            cmd.stdin()
                .ok_or_else(|| RemoteError::new(RemoteErrorType::ProtocolError))?
                .write_all(&data)
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            debug!("uploaded archive to kube at: {}", path.display());

            cmd.join()
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            Ok(metadata.size)
        })?;

        if !self.exists(path.as_path())? {
            return Err(RemoteError::new_ex(
                RemoteErrorType::NoSuchFileOrDirectory,
                "failed to create file",
            ));
        }

        Ok(size)
    }

    fn open_file(
        &mut self,
        src: &Path,
        mut dest: Box<dyn std::io::Write + Send>,
    ) -> RemoteResult<u64> {
        self.check_connection()?;

        let src = path_utils::absolutize(self.wrkdir.as_path(), src);
        debug!("opening file from kube at: {}", src.display());

        let tempfile = tempfile::NamedTempFile::new()
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

        let file_size = self.runtime.block_on(async {
            let mut tar_writer = tokio::fs::File::create(tempfile.path())
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

            let attach_params = AttachParams::default()
                .container(self.container.clone())
                .stdout(true)
                .stderr(true)
                .stdin(false);
            let mut cmd = self
                .pods
                .as_ref()
                .unwrap()
                .exec(
                    &self.pod_name,
                    vec![
                        "tar",
                        "cf",
                        "-",
                        "-C",
                        src.parent()
                            .unwrap_or(Path::new("/"))
                            .display()
                            .to_string()
                            .as_str(),
                        src.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .to_string()
                            .as_str(),
                    ],
                    &attach_params,
                )
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            let mut reader = cmd
                .stdout()
                .ok_or_else(|| RemoteError::new(RemoteErrorType::ProtocolError))?;

            let file_size: u64 = tokio::io::copy(&mut reader, &mut tar_writer)
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            cmd.join()
                .await
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

            debug!(
                "copied from kube to tar {}; {file_size} bytes",
                tempfile.path().display()
            );

            let tar_reader = std::fs::File::open(tempfile.path())
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

            let mut ar = tar::Archive::new(tar_reader);
            let mut file_to_extract = ar
                .entries()
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?
                .next()
                .ok_or(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))?
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

            let file_size = std::io::copy(&mut file_to_extract, &mut dest)
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::IoError, err.to_string()))?;

            debug!("extracted file to dest; {file_size} bytes");

            Ok(file_size)
        })?;

        Ok(file_size)
    }
}

#[cfg(test)]
mod test {

    #[cfg(feature = "integration-tests")]
    use std::io::Cursor;

    use pretty_assertions::assert_eq;
    #[cfg(feature = "integration-tests")]
    use serial_test::serial;

    use super::*;

    #[test]
    fn should_init_kube_fs() {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let mut client = KubeFs::new("test", "test", &rt);
        assert!(client.config.is_none());
        assert_eq!(client.is_connected(), false);
    }

    #[test]
    fn should_fail_connection_to_bad_server() {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let mut client = KubeFs::new("aaaaaa", "test", &rt);
        assert!(client.connect().is_err());
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_append_to_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_change_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        let pwd = client.pwd().ok().unwrap();
        assert!(client.change_dir(Path::new("/tmp")).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_change_directory_relative() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        assert!(client
            .create_dir(
                Path::new("should_change_directory_relative"),
                UnixPex::from(0o755)
            )
            .is_ok());
        assert!(client
            .change_dir(Path::new("should_change_directory_relative/"))
            .is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_change_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_copy_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_copy_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_create_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        let p = PathBuf::from(format!("{}/mydir", client.pwd().unwrap().display()));
        assert!(client.exists(&p).unwrap());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_create_directory_cause_already_exists() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_create_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // create directory
        assert!(client
            .create_dir(
                Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"),
                UnixPex::from(0o755)
            )
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_create_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert_eq!(
            client.create_file(p, &metadata, Box::new(reader)).unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_create_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_exec_command() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        assert_eq!(
            client.exec("echo 5").ok().unwrap(),
            (0, String::from("5\n"))
        );
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_tell_whether_file_exists() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_list_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_list_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_move_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_move_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_open_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let metadata = Metadata::default().size(file_data.len() as u64);
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert_eq!(client.open_file(p, buffer).ok().unwrap(), 10);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_open_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client
            .open_file(Path::new("/tmp/aashafb/hhh"), buffer)
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_print_working_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        assert!(client.pwd().is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_remove_dir_all() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_remove_dir_all() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Remove dir
        assert!(client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_remove_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        assert!(client.remove_dir(dir_path.as_path()).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_remove_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_remove_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.remove_file(p).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_setstat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        assert_eq!(stat.accessed, None);
        assert_eq!(stat.created, None);
        assert_eq!(stat.modified, Some(SystemTime::UNIX_EPOCH));
        assert_eq!(stat.mode.unwrap(), UnixPex::from(0o755));
        assert_eq!(stat.size, 7);

        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_setstat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_stat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert_eq!(
            client
                .create_file(p, &metadata, Box::new(reader))
                .ok()
                .unwrap(),
            7
        );
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.size, 7);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_stat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_make_symlink() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    #[serial]
    fn should_not_make_symlink() {
        crate::log_init();
        let (pods, mut client) = setup_client();
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
        finalize_client(pods, client);
    }

    #[test]
    fn should_get_name_and_link() {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let client = KubeFs::new("test", "test", &rt);
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
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let client = KubeFs::new("test", "test", &rt);
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
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let client = KubeFs::new("test", "test", &rt);
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
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let client = KubeFs::new("test", "test", &rt);
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
    fn test_should_parse_special_permissions_ls_output() {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let client = KubeFs::new("test", "test", &rt);
        assert!(client
            .parse_ls_output(
                Path::new("/tmp"),
                "-rw-rwSrw-    1 manufact  manufact    241813 Apr 22 09:31 L9800.SPF",
            )
            .is_ok());
        assert!(client
            .parse_ls_output(
                Path::new("/tmp"),
                "-rw-rwsrw-    1 manufact  manufact    241813 Apr 22 09:31 L9800.SPF",
            )
            .is_ok());

        assert!(client
            .parse_ls_output(
                Path::new("/tmp"),
                "-rw-rwtrw-    1 manufact  manufact    241813 Apr 22 09:31 L9800.SPF",
            )
            .is_ok());

        assert!(client
            .parse_ls_output(
                Path::new("/tmp"),
                "-rw-rwTrw-    1 manufact  manufact    241813 Apr 22 09:31 L9800.SPF",
            )
            .is_ok());
    }

    #[test]
    fn should_return_errors_on_uninitialized_client() {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let mut client = KubeFs::new("test", "test", &rt);
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

    #[cfg(feature = "integration-tests")]
    fn setup_client() -> (Api<Pod>, KubeFs) {
        // setup pod with random name

        use kube::api::PostParams;
        use kube::config::AuthInfo;
        use kube::ResourceExt as _;
        let pod_name = generate_pod_name();
        debug!("Pod name: {pod_name}");

        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );

        let minikube_ip = std::env::var("MINIKUBE_IP").unwrap();

        // setup pod
        debug!("setting up pod");
        // config for minikube
        let mut auth_info = AuthInfo::default();
        auth_info.username = Some("minikube".to_string());
        // get home
        let home = std::env::var("HOME").unwrap();
        auth_info.client_certificate =
            Some(format!("{home}/.minikube/profiles/minikube/client.crt"));
        auth_info.client_key = Some(format!("{home}/.minikube/profiles/minikube/client.key"));

        //let root_cert = format!("{home}/.minikube/ca.crt");

        debug!("Auth info: {auth_info:?}");

        let config = Config {
            cluster_url: format!("https://{minikube_ip}:8443").parse().unwrap(),
            default_namespace: "default".to_string(),
            read_timeout: None,
            root_cert: None,
            connect_timeout: None,
            write_timeout: None,
            accept_invalid_certs: true,
            auth_info,
            proxy_url: None,
            tls_server_name: None,
        };

        let pods = runtime.block_on(async {
            let client = Client::try_from(config.clone()).unwrap();
            let pods: Api<Pod> = Api::default_namespaced(client);

            let p: Pod = serde_json::from_value(serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": { "name": pod_name },
                "spec": {
                    "containers": [{
                      "name": "alpine",
                      "image": "alpine" ,
                      "command": ["tail", "-f", "/dev/null"],
                    }],
                }
            }))
            .unwrap();

            let pp = PostParams::default();
            match pods.create(&pp, &p).await {
                Ok(o) => {
                    let name = o.name_any();
                    assert_eq!(p.name_any(), name);
                    info!("Created {}", name);
                }
                Err(kube::Error::Api(ae)) => assert_eq!(ae.code, 409), // if you skipped delete, for instance
                Err(e) => panic!("failed to create: {e}"), // any other case is probably bad
            }

            debug!("Pod created");

            let establish = kube::runtime::wait::await_condition(
                pods.clone(),
                &pod_name,
                kube::runtime::conditions::is_pod_running(),
            );

            info!("Waiting for pod to be running...");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(30), establish)
                .await
                .expect("pod timeout");

            pods
        });

        let mut client = KubeFs::new(&pod_name, "alpine", &runtime).config(config.clone());
        client.connect().expect("connection failed");
        // Create wrkdir
        let tempdir = PathBuf::from(generate_tempdir());
        client
            .create_dir(tempdir.as_path(), UnixPex::from(0o775))
            .expect("failed to create tempdir");
        // Change directory
        client
            .change_dir(tempdir.as_path())
            .expect("failed to enter tempdir");
        (pods, client)
    }

    #[cfg(feature = "integration-tests")]
    fn finalize_client(pods: Api<Pod>, mut client: KubeFs) {
        // Get working directory

        use kube::api::DeleteParams;
        use kube::ResourceExt as _;
        let wrkdir = client.pwd().ok().unwrap();
        // Remove directory
        assert!(client.remove_dir_all(wrkdir.as_path()).is_ok());
        assert!(client.disconnect().is_ok());

        // cleanup pods
        let pod_name = client.pod_name;
        client.runtime.block_on(async {
            let dp = DeleteParams::default();
            pods.delete(&pod_name, &dp).await.unwrap().map_left(|pdel| {
                info!("Deleting {pod_name} pod started: {:?}", pdel);
                assert_eq!(pdel.name_any(), pod_name);
            });
        })
    }

    #[cfg(feature = "integration-tests")]
    fn generate_pod_name() -> String {
        use rand::distributions::{Alphanumeric, DistString};
        use rand::thread_rng;
        let random_string: String = Alphanumeric
            .sample_string(&mut thread_rng(), 8)
            .chars()
            .filter(|c| c.is_alphabetic())
            .map(|c| c.to_ascii_lowercase())
            .take(8)
            .collect();

        format!("test-{}", random_string)
    }

    #[cfg(feature = "integration-tests")]
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
