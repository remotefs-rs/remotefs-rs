//! ## Kube Container FS
//!
//! The `KubeContainerFs` client is a client that allows you to interact with a container in a pod.

mod exec;
mod stream;

use std::future::poll_fn;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;

use futures_io::{AsyncRead, AsyncWrite};
use k8s_openapi::api::core::v1::Pod;
use kube::api::AttachParams;
use kube::{Api, Client, Config};
use lazy_regex::{Lazy, Regex};
use remotefs::File;
use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, FileType, Metadata,
    ReadOptions, RemoteError, RemoteErrorType, RemoteResult, SetMetadata, UnixPex, UnixPexClass,
    WriteOptions,
};

use self::exec::KubeExec;
use self::stream::{ExecHandle, Finalize, KubeReadStream, KubeWriteStream, copy_limited};
use crate::utils::{fmt as fmt_utils, parser as parser_utils, path as path_utils};

/// NOTE: about this damn regex <https://stackoverflow.com/questions/32480890/is-there-a-regex-to-parse-the-values-from-an-ftp-directory-listing>
static LS_RE: Lazy<Regex> = lazy_regex!(
    r#"^([\-ld])([\-rwxsStT]{9})\s+(\d+)\s+(.+)\s+(.+)\s+(\d+)\s+(\w{3}\s+\d{1,2}\s+(?:\d{1,2}:\d{1,2}|\d{4}))\s+(.+)$"#
);

const WRITER_STDIN_BUF_SIZE: usize = 64 * 1024;

fn writer_script(path: &Path, opts: &WriteOptions, redirect: &str) -> String {
    let quoted = path_utils::shell_quote(path);
    match opts.size_hint {
        Some(size) => format!("head -c {size} {redirect} {quoted}"),
        None => format!("cat {redirect} {quoted}"),
    }
}

/// Blocking adapter over [`KubeContainerFs`], implementing [`remotefs::RemoteFs`].
#[cfg(feature = "tokio")]
pub type BlockingKubeContainerFs = remotefs::adapters::blocking::BlockOn<KubeContainerFs>;

/// An [`AsyncRemoteFs`] client speaking to a single Kubernetes pod container.
///
/// The client shells out to POSIX utilities (`cat`, `ls`, `rm`, ...) inside
/// the container over the pod exec stream, since Kubernetes has no native
/// remote file system API. Every path must be an absolute POSIX path.
///
/// # Examples
///
/// ```rust,no_run
/// use std::path::Path;
///
/// use remotefs::AsyncRemoteFs;
/// use remotefs_kube::KubeContainerFs;
///
/// # async fn run() -> remotefs::RemoteResult<()> {
/// let mut client = KubeContainerFs::new("my-pod", "container-name");
/// client.connect().await?;
/// let entries = client.list_dir(Path::new("/tmp")).await?;
/// client.disconnect().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct KubeContainerFs {
    config: Option<Config>,
    container: String,
    pod_name: String,
    exec: Option<KubeExec>,
}

impl KubeContainerFs {
    /// Create a client for `container` on `pod_name`.
    ///
    /// If [`KubeContainerFs::config`] is not called before
    /// [`connect`](AsyncRemoteFs::connect), the client falls back to the
    /// default kubeconfig (or the in-cluster configuration, when running
    /// inside a pod).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use remotefs_kube::KubeContainerFs;
    ///
    /// let client = KubeContainerFs::new("my-pod", "container-name");
    /// assert_eq!(client.pod_name(), "my-pod");
    /// ```
    pub fn new(pod_name: impl Into<String>, container: impl Into<String>) -> Self {
        Self {
            config: None,
            container: container.into(),
            pod_name: pod_name.into(),
            exec: None,
        }
    }

    /// Set the Kubernetes client configuration to use on
    /// [`connect`](AsyncRemoteFs::connect), instead of the default kubeconfig.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs_kube::{Config, KubeContainerFs};
    ///
    /// let config = Config::new("https://127.0.0.1:8443".parse().unwrap());
    /// let client = KubeContainerFs::new("my-pod", "container-name").config(config);
    /// ```
    #[must_use]
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// The pod this client targets.
    pub fn pod_name(&self) -> &str {
        &self.pod_name
    }

    /// The container this client targets.
    pub fn container(&self) -> &str {
        &self.container
    }

    /// Wrap the client for blocking callers using the given runtime handle.
    ///
    /// The returned value implements [`remotefs::RemoteFs`] and can be stored
    /// as `Box<dyn RemoteFs>`. It must not be used from inside an async
    /// context, as documented by [`tokio::runtime::Handle::block_on`].
    ///
    /// # Panics
    ///
    /// Panics if `handle` belongs to a current-thread runtime, which cannot
    /// drive the blocked operation from a non-async caller.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_kube::KubeContainerFs;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = tokio::runtime::Runtime::new()?;
    /// let mut client: Box<dyn RemoteFs> = Box::new(
    ///     KubeContainerFs::new("my-pod", "container-name").into_blocking(runtime.handle().clone()),
    /// );
    /// client.connect()?;
    /// client.disconnect()?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tokio")]
    #[must_use]
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingKubeContainerFs {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }

    /// Build an already-connected client over an existing pod API handle.
    ///
    /// Used by `KubeMultiPodFs` to address a container without mutating
    /// shared state.
    pub(crate) fn attached(pods: Api<Pod>, pod_name: &str, container: &str) -> Self {
        Self {
            config: None,
            container: container.to_string(),
            pod_name: pod_name.to_string(),
            exec: Some(KubeExec::new(pods, pod_name, container)),
        }
    }

    // -- private

    /// The exec runner, or `NotConnected`.
    fn runner(&self) -> RemoteResult<&KubeExec> {
        self.exec
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    async fn exists_at(&self, exec: &KubeExec, path: &Path) -> RemoteResult<bool> {
        let quoted = path_utils::shell_quote(path);
        exec.shell_test(&format!("test -e {quoted} || test -L {quoted}"))
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::StatFailed, err))
    }

    async fn is_directory(&self, exec: &KubeExec, path: &Path) -> RemoteResult<bool> {
        exec.shell_test(&format!("test -d {}", path_utils::shell_quote(path)))
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::StatFailed, err))
    }

    /// Fail with `NoSuchFileOrDirectory` unless `path` exists.
    async fn require_exists(&self, exec: &KubeExec, path: &Path) -> RemoteResult<()> {
        if self.exists_at(exec, path).await? {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
        }
    }

    /// Fail unless the parent of `path` is an existing directory.
    async fn require_parent_dir(&self, exec: &KubeExec, path: &Path) -> RemoteResult<()> {
        let parent = path.parent().unwrap_or(Path::new("/"));
        if self.is_directory(exec, parent).await? {
            Ok(())
        } else {
            Err(RemoteError::with_message(
                RemoteErrorType::NoSuchFileOrDirectory,
                format!("parent directory {} does not exist", parent.display()),
            ))
        }
    }

    /// Spawn a writer command reading from stdin.
    async fn spawn_writer(
        &self,
        path: &Path,
        opts: &WriteOptions,
        redirect: &str,
    ) -> RemoteResult<AsyncWriteStream> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        self.require_parent_dir(exec, path).await?;
        let script = writer_script(path, opts, redirect);
        debug!("Opening write stream: {script}");
        let params = AttachParams::default()
            .stdin(true)
            // Kubernetes runtimes may require an output channel even when
            // the command redirects stdout to the target file.
            .stdout(true)
            .stderr(false)
            .max_stdin_buf_size(WRITER_STDIN_BUF_SIZE);
        let mut process = exec.spawn(&["/bin/sh", "-c", &script], params).await?;
        let stdin = process.stdin().ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::ProtocolError, "failed to attach stdin")
        })?;
        let handle = ExecHandle::new(process)?;
        let finalize = Finalize {
            exec: exec.clone(),
            path: path.to_path_buf(),
            mode: opts.mode,
            modified: opts.modified,
        };
        Ok(AsyncWriteStream::new(KubeWriteStream::new(
            Box::new(stdin),
            Some(handle),
            opts.size_hint,
            Some(finalize),
        )))
    }
}

/// Parse a line of `ls -l` output into a [`File`] under `path`.
///
/// Returns `None` for lines that are not file entries (`total N`, `.`,
/// `..`, special files).
pub(crate) fn parse_ls_output(path: &Path, line: &str) -> Option<File> {
    trace!("Parsing LS line: '{line}'");
    let metadata = LS_RE.captures(line)?;
    // NOTE: metadata fmt: (regex, file_type, permissions, link_count, uid, gid, filesize, modified, filename)
    if metadata.len() < 8 {
        return None;
    }
    let (is_dir, is_symlink): (bool, bool) = match metadata.get(1)?.as_str() {
        "-" => (false, false),
        "l" => (false, true),
        "d" => (true, false),
        _ => return None,
    };
    let pex_str = metadata.get(2)?.as_str();
    if pex_str.len() < 9 {
        return None;
    }
    let pex = |range: Range<usize>| {
        let mut count: u8 = 0;
        for (i, c) in pex_str[range].chars().enumerate() {
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
    let mode = UnixPex::new(
        UnixPexClass::from(pex(0..3)),
        UnixPexClass::from(pex(3..6)),
        UnixPexClass::from(pex(6..9)),
    );
    let modified: SystemTime =
        parser_utils::parse_lstime(metadata.get(7)?.as_str(), "%b %d %Y", "%b %d %H:%M")
            .unwrap_or(SystemTime::UNIX_EPOCH);
    let uid: Option<u32> = metadata.get(4)?.as_str().parse::<u32>().ok();
    let gid: Option<u32> = metadata.get(5)?.as_str().parse::<u32>().ok();
    let size = metadata.get(6)?.as_str().parse::<u64>().unwrap_or(0);
    let (file_name, symlink): (String, Option<PathBuf>) = match is_symlink {
        true => get_name_and_link(metadata.get(8)?.as_str()),
        false => (String::from(metadata.get(8)?.as_str()), None),
    };
    let file_name = PathBuf::from(&file_name)
        .file_name()
        .map(|x| x.to_string_lossy().to_string())
        .unwrap_or(file_name);
    if file_name.as_str() == "." || file_name.as_str() == ".." {
        debug!("File name is {file_name}; ignoring entry");
        return None;
    }
    let path = path_utils::join(path, file_name.as_str());
    let file_type = if symlink.is_some() {
        FileType::Symlink
    } else if is_dir {
        FileType::Directory
    } else {
        FileType::File
    };
    let mut meta = Metadata::default()
        .file_type(file_type)
        .mode(mode)
        .modified(modified)
        .size(size);
    meta.gid = gid;
    meta.uid = uid;
    meta.symlink = symlink;
    trace!("Found entry at {} with metadata {meta:?}", path.display());
    Some(File::new(path, meta))
}

/// Split the `ls -l` name token into file name and symlink target.
fn get_name_and_link(token: &str) -> (String, Option<PathBuf>) {
    let tokens: Vec<&str> = token.split(" -> ").collect();
    let filename: String = String::from(*tokens.first().unwrap_or(&token));
    let symlink: Option<PathBuf> = tokens.get(1).map(PathBuf::from);
    (filename, symlink)
}

#[remotefs::async_trait]
impl AsyncRemoteFs for KubeContainerFs {
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.exec.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        debug!("Initializing Kube connection...");
        let client = match self.config.as_ref() {
            Some(config) => Client::try_from(config.clone()),
            None => Client::try_default().await,
        }
        .map_err(|err| RemoteError::with_source(RemoteErrorType::ConnectionError, err))?;
        let api: Api<Pod> = Api::default_namespaced(client);
        api.get(&self.pod_name)
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::ConnectionError, err))?;
        self.exec = Some(KubeExec::new(api, &self.pod_name, &self.container));
        info!("Connection established with pod {}", self.pod_name);
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        if self.exec.take().is_none() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        info!("Disconnected from remote");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.exec.is_some()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::APPEND
            | Capabilities::RANGE_READ
            | Capabilities::COPY
            | Capabilities::SYMLINK
            | Capabilities::SET_METADATA
            | Capabilities::POSIX_MODE
            | Capabilities::EXEC
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        debug!("Getting file entries in {}", path.display());
        if !self.is_directory(exec, path).await? {
            return Err(if self.exists_at(exec, path).await? {
                RemoteError::with_message(RemoteErrorType::BadFile, "not a directory")
            } else {
                RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)
            });
        }
        let output = exec
            .shell(&format!("ls -la {}/", path_utils::shell_quote(path)))
            .await?;
        if output.exit_code != 0 {
            return Err(RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("ls exited with status {}", output.exit_code),
            ));
        }
        let lines: Vec<&str> = output.stdout.lines().collect();
        let entries: Vec<File> = lines
            .iter()
            .filter_map(|line| parse_ls_output(path, line))
            .collect();
        debug!(
            "Found {} out of {} valid file entries",
            entries.len(),
            lines.len()
        );
        Ok(entries)
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        debug!("Stat {}", path.display());
        if path == Path::new("/") {
            return Ok(File::new(
                PathBuf::from("/"),
                Metadata::default().file_type(FileType::Directory),
            ));
        }
        let quoted = path_utils::shell_quote(path);
        let cmd = match self.is_directory(exec, path).await? {
            true => format!("ls -ld {quoted}"),
            false => format!("ls -l {quoted}"),
        };
        let output = exec.shell(&cmd).await?;
        if output.exit_code != 0 {
            return Err(RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory));
        }
        let parent = path.parent().unwrap_or(Path::new("/"));
        parse_ls_output(parent, output.stdout.trim())
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory))
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        self.exists_at(exec, path).await
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        debug!("Setting attributes for {}", path.display());
        self.require_exists(exec, path).await?;
        let quoted = path_utils::shell_quote(path);
        if let Some(mode) = metadata.mode {
            exec.shell_ok(
                &format!("chmod {:o} {quoted}", u32::from(mode)),
                RemoteErrorType::StatFailed,
            )
            .await?;
        }
        if let Some(uid) = metadata.uid {
            let gid = metadata
                .gid
                .map(|gid| format!(":{gid}"))
                .unwrap_or_default();
            exec.shell_ok(
                &format!("chown {uid}{gid} {quoted}"),
                RemoteErrorType::StatFailed,
            )
            .await?;
        } else if let Some(gid) = metadata.gid {
            exec.shell_ok(
                &format!("chgrp {gid} {quoted}"),
                RemoteErrorType::StatFailed,
            )
            .await?;
        }
        if let Some(accessed) = metadata.accessed {
            exec.shell_ok(
                &format!(
                    "touch -a -t {} {quoted}",
                    fmt_utils::fmt_time_utc(accessed, "%Y%m%d%H%M.%S")
                ),
                RemoteErrorType::StatFailed,
            )
            .await?;
        }
        if let Some(modified) = metadata.modified {
            exec.shell_ok(
                &format!(
                    "touch -m -t {} {quoted}",
                    fmt_utils::fmt_time_utc(modified, "%Y%m%d%H%M.%S")
                ),
                RemoteErrorType::StatFailed,
            )
            .await?;
        }
        Ok(())
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        if self.exists_at(exec, path).await? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        let quoted = path_utils::shell_quote(path);
        let cmd = match mode {
            Some(mode) => format!("mkdir -m {:o} {quoted}", u32::from(mode)),
            None => format!("mkdir {quoted}"),
        };
        debug!("Creating directory at {} ({cmd})", path.display());
        exec.shell_ok(&cmd, RemoteErrorType::FileCreateDenied).await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        self.require_exists(exec, path).await?;
        debug!("Removing file {}", path.display());
        exec.shell_ok(
            &format!("rm -f {}", path_utils::shell_quote(path)),
            RemoteErrorType::CouldNotRemoveFile,
        )
        .await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        self.require_exists(exec, path).await?;
        debug!("Removing directory {}", path.display());
        exec.shell_ok(
            &format!("rmdir {}", path_utils::shell_quote(path)),
            RemoteErrorType::DirectoryNotEmpty,
        )
        .await
    }

    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        self.require_exists(exec, path).await?;
        debug!("Removing directory {} recursively", path.display());
        exec.shell_ok(
            &format!("rm -rf {}", path_utils::shell_quote(path)),
            RemoteErrorType::CouldNotRemoveFile,
        )
        .await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src = path_utils::ensure_posix_absolute(src)?;
        let dest = path_utils::ensure_posix_absolute(dest)?;
        let exec = self.runner()?;
        self.require_exists(exec, src).await?;
        debug!("Moving {} to {}", src.display(), dest.display());
        exec.shell_ok(
            &format!(
                "mv -f {} {}",
                path_utils::shell_quote(src),
                path_utils::shell_quote(dest)
            ),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let src = path_utils::ensure_posix_absolute(src)?;
        let dest = path_utils::ensure_posix_absolute(dest)?;
        let exec = self.runner()?;
        self.require_exists(exec, src).await?;
        debug!("Copying {} to {}", src.display(), dest.display());
        exec.shell_ok(
            &format!(
                "cp -rf {} {}",
                path_utils::shell_quote(src),
                path_utils::shell_quote(dest)
            ),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let target = path_utils::ensure_posix_absolute(target)?;
        let exec = self.runner()?;
        debug!(
            "Creating a symlink at {} pointing at {}",
            path.display(),
            target.display()
        );
        self.require_exists(exec, target).await?;
        if self.exists_at(exec, path).await? {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        exec.shell_ok(
            &format!(
                "ln -s {} {}",
                path_utils::shell_quote(target),
                path_utils::shell_quote(path)
            ),
            RemoteErrorType::FileCreateDenied,
        )
        .await
    }

    /// Open `path` for reading.
    ///
    /// Offsets and lengths are applied remotely with `tail -c` and
    /// `head -c`. `length == Some(0)` returns an empty stream without
    /// spawning a process; an offset beyond the end of the file yields an
    /// empty stream as well.
    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let path = path_utils::ensure_posix_absolute(path)?;
        let exec = self.runner()?;
        let entry = self.stat(path).await?;
        if entry.is_dir() {
            return Err(RemoteError::with_message(
                RemoteErrorType::BadFile,
                "cannot read a directory",
            ));
        }
        if opts.length == Some(0) {
            return Ok(AsyncReadStream::new(KubeReadStream::empty()));
        }
        let quoted = path_utils::shell_quote(path);
        let offset = opts.offset.unwrap_or(0);
        let start = offset.saturating_add(1);
        let script = match (offset, opts.length) {
            (0, None) => format!("cat {quoted}"),
            (0, Some(length)) => format!("head -c {length} {quoted}"),
            (_, None) => format!("tail -c +{start} {quoted}"),
            (_, Some(length)) => {
                format!("tail -c +{start} {quoted} | head -c {length}")
            }
        };
        debug!("Opening read stream: {script}");
        let params = AttachParams::default()
            .stdin(false)
            .stdout(true)
            .stderr(false);
        let mut process = exec.spawn(&["/bin/sh", "-c", &script], params).await?;
        let stdout = process.stdout().ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::ProtocolError, "failed to attach stdout")
        })?;
        let handle = ExecHandle::new(process)?;
        Ok(AsyncReadStream::new(KubeReadStream::new(
            Box::new(stdout),
            Some(handle),
        )))
    }

    /// Create or truncate `path` and return a write stream feeding `cat`.
    ///
    /// The parent directory must exist. The file is only complete once the
    /// stream is finished; `WriteOptions::mode` and `modified` are applied
    /// on `finish`, and a `size_hint` is verified on `finish`.
    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        self.spawn_writer(path, opts, ">").await
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        self.spawn_writer(path, opts, ">>").await
    }

    /// Write `src` to `path`.
    ///
    /// When `opts.size_hint` is set, at most that many bytes are read from
    /// `src` (the hint is the transfer bound) and the transfer fails unless
    /// exactly that many bytes were written.
    async fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        let mut stream = self.create(path, opts).await?;
        let copied = copy_limited(src, &mut stream, opts.size_hint)
            .await
            .map_err(RemoteError::from);
        let copied = match copied {
            Ok(count) => poll_fn(|cx| Pin::new(&mut stream).poll_flush(cx))
                .await
                .map(|()| count)
                .map_err(RemoteError::from),
            Err(error) => Err(error),
        };
        let finished = stream.finish().await;
        match (copied, finished) {
            (Ok(count), Ok(())) => Ok(count),
            (Err(err), _) | (Ok(_), Err(err)) => Err(err),
        }
    }

    async fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        let exec = self.runner()?;
        debug!(r#"Executing command "{cmd}""#);
        exec.shell(cmd).await
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_init_kube_fs() {
        let client = KubeContainerFs::new("test", "test");
        assert!(client.config.is_none());
        assert_eq!(client.pod_name(), "test");
        assert_eq!(client.container(), "test");
        assert!(!client.is_connected());
    }

    #[test]
    fn writer_command_uses_size_hint_to_bound_remote_read() {
        let path = Path::new("/tmp/file");
        assert_eq!(
            super::writer_script(path, &WriteOptions::default(), ">"),
            "cat > '/tmp/file'"
        );
        let opts = WriteOptions::default().size_hint(5);
        assert_eq!(
            super::writer_script(path, &opts, ">"),
            "head -c 5 > '/tmp/file'"
        );
    }

    #[test]
    fn should_advertise_capabilities() {
        let caps = KubeContainerFs::new("test", "test").capabilities();
        assert!(caps.contains(Capabilities::STREAM_READ));
        assert!(caps.contains(Capabilities::STREAM_WRITE));
        assert!(caps.contains(Capabilities::APPEND));
        assert!(caps.contains(Capabilities::RANGE_READ));
        assert!(caps.contains(Capabilities::COPY));
        assert!(caps.contains(Capabilities::SYMLINK));
        assert!(caps.contains(Capabilities::SET_METADATA));
        assert!(caps.contains(Capabilities::POSIX_MODE));
        assert!(caps.contains(Capabilities::EXEC));
        assert!(!caps.contains(Capabilities::SEEK_READ));
        assert!(!caps.contains(Capabilities::SEEK_WRITE));
    }

    #[tokio::test]
    async fn should_reject_relative_paths_before_connection_check() {
        let client = KubeContainerFs::new("test", "test");
        let relative = Path::new("a.txt");
        assert_eq!(
            client.stat(relative).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.list_dir(relative).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client.exists(relative).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .open(relative, &ReadOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .create(relative, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .rename(Path::new("/a"), relative)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[tokio::test]
    async fn should_fail_as_not_connected() {
        let client = KubeContainerFs::new("test", "test");
        let path = Path::new("/tmp/a.txt");
        assert_eq!(
            client.stat(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exists(path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exec("echo 5").await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        let mut client = client;
        assert_eq!(
            client.disconnect().await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
    }

    #[tokio::test]
    async fn should_fail_connection_to_bad_server() {
        let config = Config::new("https://127.0.0.1:1".parse().unwrap());
        let mut client = KubeContainerFs::new("aaaaaa", "test").config(config);
        assert_eq!(
            client.connect().await.unwrap_err().kind(),
            RemoteErrorType::ConnectionError
        );
        assert!(!client.is_connected());
    }

    #[test]
    fn should_be_send_sync_and_object_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<KubeContainerFs>();
        let _: Box<dyn AsyncRemoteFs> = Box::new(KubeContainerFs::new("test", "test"));
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn blocking_wrapper_is_a_remote_fs_trait_object() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client: Box<dyn remotefs::RemoteFs> =
            Box::new(KubeContainerFs::new("test", "test").into_blocking(runtime.handle().clone()));
        assert!(!client.is_connected());
        assert!(client.capabilities().contains(Capabilities::EXEC));
    }

    #[cfg(feature = "tokio")]
    #[test]
    #[should_panic(expected = "into_blocking requires a multi-thread Tokio runtime")]
    fn blocking_wrapper_rejects_current_thread_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _ = KubeContainerFs::new("test", "test").into_blocking(runtime.handle().clone());
    }

    #[test]
    fn should_get_name_and_link() {
        assert_eq!(
            get_name_and_link("Cargo.toml"),
            (String::from("Cargo.toml"), None)
        );
        assert_eq!(
            get_name_and_link("Cargo -> Cargo.toml"),
            (String::from("Cargo"), Some(PathBuf::from("Cargo.toml")))
        );
    }

    #[test]
    fn should_parse_file_ls_output() {
        let entry = parse_ls_output(
            Path::new("/tmp"),
            "-rw-r--r-- 1 root root  2056 giu 13 21:11 /tmp/Cargo.toml",
        )
        .unwrap();
        assert_eq!(entry.name().as_str(), "Cargo.toml");
        assert!(entry.is_file());
        assert_eq!(entry.path, PathBuf::from("/tmp/Cargo.toml"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o644_u32);
        assert_eq!(entry.metadata.size, Some(2056));
        assert_eq!(entry.extension().unwrap().as_str(), "toml");
        assert!(entry.metadata.symlink.is_none());
        let entry = parse_ls_output(
            Path::new("/tmp"),
            "-rw-rw-rw- 1 root root  3368 nov  7  2020 CODE_OF_CONDUCT.md",
        )
        .unwrap();
        assert_eq!(entry.name().as_str(), "CODE_OF_CONDUCT.md");
        assert!(entry.is_file());
        assert_eq!(entry.path, PathBuf::from("/tmp/CODE_OF_CONDUCT.md"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o666_u32);
        assert_eq!(entry.metadata.size, Some(3368));
        assert_eq!(entry.extension().unwrap().as_str(), "md");
        assert!(entry.metadata.symlink.is_none());
    }

    #[test]
    fn should_parse_directory_from_ls_output() {
        let entry = parse_ls_output(
            Path::new("/tmp"),
            "drwxr-xr-x 1 root root   512 giu 13 21:11 docs",
        )
        .unwrap();
        assert_eq!(entry.name().as_str(), "docs");
        assert!(entry.is_dir());
        assert_eq!(entry.path, PathBuf::from("/tmp/docs"));
        assert_eq!(u32::from(entry.metadata.mode.unwrap()), 0o755_u32);
        assert!(entry.metadata.symlink.is_none());
        assert!(
            parse_ls_output(
                Path::new("/tmp"),
                "drwxr-xr-x 1 root root   512 giu 13 21:11",
            )
            .is_none()
        );
        assert!(
            parse_ls_output(
                Path::new("/tmp"),
                "crwxr-xr-x 1 root root   512 giu 13 21:11 ttyS1",
            )
            .is_none()
        );
        assert!(
            parse_ls_output(
                Path::new("/tmp"),
                "-rwxr-xr 1 root root   512 giu 13 21:11 ttyS1",
            )
            .is_none()
        );
        assert!(parse_ls_output(Path::new("/tmp"), "total 8").is_none());
        assert!(
            parse_ls_output(
                Path::new("/tmp"),
                "drwxr-xr-x 1 root root   512 giu 13 21:11 .",
            )
            .is_none()
        );
    }

    #[test]
    fn should_parse_symlink_from_ls_output() {
        let entry = parse_ls_output(
            Path::new("/tmp"),
            "lrwxrwxrwx 1 root root  2056 giu 13 21:11 Cargo -> Cargo.toml",
        )
        .unwrap();
        assert_eq!(entry.name().as_str(), "Cargo");
        assert!(entry.is_symlink());
        assert_eq!(entry.path, PathBuf::from("/tmp/Cargo"));
        assert_eq!(
            entry.metadata.symlink.as_deref().unwrap(),
            Path::new("Cargo.toml")
        );
    }

    #[cfg(feature = "integration-tests")]
    mod integration {

        use std::time::SystemTime;

        use futures::io::Cursor;
        use pretty_assertions::assert_eq;
        use serial_test::serial;

        use super::*;

        async fn write(client: &KubeContainerFs, path: &Path, data: &str) -> u64 {
            let mut reader = Cursor::new(data.as_bytes().to_vec());
            client
                .write_file(
                    path,
                    &WriteOptions::default().size_hint(data.len() as u64),
                    &mut reader,
                )
                .await
                .expect("write failed")
        }

        async fn read(client: &KubeContainerFs, path: &Path, opts: &ReadOptions) -> Vec<u8> {
            let mut dest = Cursor::new(Vec::new());
            client
                .read_file(path, opts, &mut dest)
                .await
                .expect("read failed");
            dest.into_inner()
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_append_to_file() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            assert_eq!(write(&client, &p, "hello ").await, 6);
            let mut reader = Cursor::new(b"world".to_vec());
            assert_eq!(
                client
                    .append_file(&p, &WriteOptions::default(), &mut reader)
                    .await
                    .unwrap(),
                5
            );
            assert_eq!(
                read(&client, &p, &ReadOptions::default()).await,
                b"hello world"
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_copy_file() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            write(&client, &p, "test data\n").await;
            let dest = tempdir.join("b.txt");
            assert!(client.copy(&p, &dest).await.is_ok());
            assert!(client.stat(&p).await.is_ok());
            assert!(client.stat(&dest).await.is_ok());
            assert!(
                client
                    .copy(&p, &tempdir.join("aaa/bbbb/ccc/b.txt"))
                    .await
                    .is_err()
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_create_directory() {
            let (pods, client, tempdir) = setup_client().await;
            let dir = tempdir.join("mydir");
            assert!(
                client
                    .create_dir(&dir, Some(UnixPex::from(0o755)))
                    .await
                    .is_ok()
            );
            assert!(client.exists(&dir).await.unwrap());
            assert_eq!(
                client
                    .create_dir(&dir, Some(UnixPex::from(0o755)))
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::AlreadyExists
            );
            assert!(
                client
                    .create_dir(&tempdir.join("nomode"), None)
                    .await
                    .is_ok()
            );
            assert!(
                client
                    .create_dir(Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"), None)
                    .await
                    .is_err()
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_create_file_and_verify_size() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            assert_eq!(write(&client, &p, "test data\n").await, 10);
            assert_eq!(client.stat(&p).await.unwrap().metadata().size, Some(10));
            let mut reader = Cursor::new(b"test data\n".to_vec());
            assert_eq!(
                client
                    .write_file(
                        Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf"),
                        &WriteOptions::default(),
                        &mut reader,
                    )
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_bound_write_to_size_hint_and_apply_mode() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.sh");
            let mut reader = Cursor::new(b"echo 5\nignored".to_vec());
            let opts = WriteOptions::default()
                .size_hint(7)
                .mode(UnixPex::from(0o755))
                .modified(SystemTime::UNIX_EPOCH);
            assert_eq!(client.write_file(&p, &opts, &mut reader).await.unwrap(), 7);
            let entry = client.stat(&p).await.unwrap();
            assert_eq!(entry.metadata().size, Some(7));
            assert_eq!(entry.metadata().mode.unwrap(), UnixPex::from(0o755));
            assert_eq!(entry.metadata().modified, Some(SystemTime::UNIX_EPOCH));
            let mut short = Cursor::new(b"abc".to_vec());
            assert_eq!(
                client
                    .write_file(
                        &tempdir.join("short"),
                        &WriteOptions::default().size_hint(10),
                        &mut short,
                    )
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::ProtocolError
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_stream_write_and_finish() {
            use futures::io::AsyncWriteExt as _;

            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("streamed.txt");
            let mut stream = client.create(&p, &WriteOptions::default()).await.unwrap();
            stream.write_all(b"hello").await.unwrap();
            stream.flush().await.unwrap();
            stream.finish().await.unwrap();
            assert_eq!(read(&client, &p, &ReadOptions::default()).await, b"hello");
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_exec_command() {
            let (pods, client, _tempdir) = setup_client().await;
            assert_eq!(
                client.exec("echo 5").await.unwrap(),
                ExecOutput::new(0, "5\n")
            );
            assert_eq!(client.exec("exit 3").await.unwrap().exit_code, 3);
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_tell_whether_file_exists() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            write(&client, &p, "test data\n").await;
            assert!(client.exists(&p).await.unwrap());
            assert!(!client.exists(&tempdir.join("b.txt")).await.unwrap());
            assert!(!client.exists(Path::new("/tmp/ppppp/bhhrhu")).await.unwrap());
            assert!(client.exists(Path::new("/tmp")).await.unwrap());
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_list_dir() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            write(&client, &p, "test data\n").await;
            let files = client.list_dir(&tempdir).await.unwrap();
            let file = files.first().unwrap();
            assert_eq!(file.name().as_str(), "a.txt");
            assert_eq!(file.path.as_path(), p.as_path());
            assert_eq!(file.extension().as_deref().unwrap(), "txt");
            assert_eq!(file.metadata.size, Some(10));
            assert_eq!(
                client
                    .list_dir(Path::new("/tmp/auhhfh/hfhjfhf/"))
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            assert_eq!(
                client.list_dir(&p).await.unwrap_err().kind(),
                RemoteErrorType::BadFile
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_rename_file() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            write(&client, &p, "test data\n").await;
            let dest = tempdir.join("b.txt");
            assert!(client.rename(&p, &dest).await.is_ok());
            assert!(!client.exists(&p).await.unwrap());
            assert!(client.exists(&dest).await.unwrap());
            assert!(
                client
                    .rename(&dest, Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"))
                    .await
                    .is_err()
            );
            assert!(
                client
                    .rename(Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"), &p)
                    .await
                    .is_err()
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_read_file_with_ranges() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            write(&client, &p, "abcdef").await;
            assert_eq!(read(&client, &p, &ReadOptions::default()).await, b"abcdef");
            assert_eq!(
                read(&client, &p, &ReadOptions::default().offset(2)).await,
                b"cdef"
            );
            assert_eq!(
                read(&client, &p, &ReadOptions::default().length(3)).await,
                b"abc"
            );
            assert_eq!(
                read(&client, &p, &ReadOptions::default().offset(2).length(2)).await,
                b"cd"
            );
            assert_eq!(
                read(&client, &p, &ReadOptions::default().offset(2).length(0)).await,
                b""
            );
            assert_eq!(
                read(&client, &p, &ReadOptions::default().offset(100)).await,
                b""
            );
            assert_eq!(
                client
                    .open(Path::new("/tmp/aashafb/hhh"), &ReadOptions::default())
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            assert_eq!(
                client
                    .open(&tempdir, &ReadOptions::default())
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::BadFile
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_finish_read_stream_before_eof() {
            use futures::io::AsyncReadExt as _;

            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("big.txt");
            write(&client, &p, &"x".repeat(64 * 1024)).await;
            let mut stream = client.open(&p, &ReadOptions::default()).await.unwrap();
            let mut buf = [0_u8; 16];
            assert_eq!(stream.read(&mut buf).await.unwrap(), 16);
            stream.finish().await.unwrap();
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_remove_dir_all() {
            let (pods, client, tempdir) = setup_client().await;
            let dir = tempdir.join("test");
            assert!(
                client
                    .create_dir(&dir, Some(UnixPex::from(0o775)))
                    .await
                    .is_ok()
            );
            write(&client, &dir.join("a.txt"), "test data\n").await;
            assert!(client.remove_dir_all(&dir).await.is_ok());
            assert!(
                client
                    .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
                    .await
                    .is_err()
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_remove_dir() {
            let (pods, client, tempdir) = setup_client().await;
            let dir = tempdir.join("test");
            assert!(
                client
                    .create_dir(&dir, Some(UnixPex::from(0o775)))
                    .await
                    .is_ok()
            );
            assert!(client.remove_dir(&dir).await.is_ok());
            assert!(
                client
                    .create_dir(&dir, Some(UnixPex::from(0o775)))
                    .await
                    .is_ok()
            );
            write(&client, &dir.join("a.txt"), "test data\n").await;
            assert_eq!(
                client.remove_dir(&dir).await.unwrap_err().kind(),
                RemoteErrorType::DirectoryNotEmpty
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_remove_file() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.txt");
            write(&client, &p, "test data\n").await;
            assert!(client.remove_file(&p).await.is_ok());
            assert_eq!(
                client.remove_file(&p).await.unwrap_err().kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_set_metadata() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.sh");
            write(&client, &p, "echo 5\n").await;
            let metadata = SetMetadata::default()
                .accessed(SystemTime::UNIX_EPOCH)
                .modified(SystemTime::UNIX_EPOCH)
                .mode(UnixPex::from(0o755))
                .uid(1000)
                .gid(1000);
            assert!(client.set_metadata(&p, &metadata).await.is_ok());
            let entry = client.stat(&p).await.unwrap();
            let stat = entry.metadata();
            assert_eq!(stat.accessed, None);
            assert_eq!(stat.created, None);
            assert_eq!(stat.modified, Some(SystemTime::UNIX_EPOCH));
            assert_eq!(stat.mode.unwrap(), UnixPex::from(0o755));
            assert_eq!(stat.size, Some(7));
            assert_eq!(
                client
                    .set_metadata(&tempdir.join("bbbbb/cccc/a.sh"), &metadata)
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_stat_file() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.sh");
            assert_eq!(write(&client, &p, "echo 5\n").await, 7);
            let entry = client.stat(&p).await.unwrap();
            assert_eq!(entry.name(), "a.sh");
            assert_eq!(entry.path(), p.as_path());
            assert_eq!(entry.metadata().size, Some(7));
            assert!(client.stat(Path::new("/")).await.unwrap().is_dir());
            assert!(client.stat(&tempdir).await.unwrap().is_dir());
            assert_eq!(
                client
                    .stat(&tempdir.join("missing"))
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_make_symlink() {
            let (pods, client, tempdir) = setup_client().await;
            let p = tempdir.join("a.sh");
            write(&client, &p, "echo 5\n").await;
            let symlink = tempdir.join("b.sh");
            assert!(client.symlink(&symlink, &p).await.is_ok());
            assert!(client.stat(&symlink).await.unwrap().is_symlink());
            assert_eq!(
                client.symlink(&symlink, &p).await.unwrap_err().kind(),
                RemoteErrorType::AlreadyExists
            );
            assert!(client.remove_file(&symlink).await.is_ok());
            assert_eq!(
                client
                    .symlink(&symlink, &tempdir.join("c.sh"))
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            finalize_client(pods, client).await;
        }

        #[cfg(feature = "tokio")]
        #[test]
        #[serial]
        fn should_round_trip_through_blocking_wrapper() {
            use remotefs::RemoteFs;

            let runtime = tokio::runtime::Runtime::new().unwrap();
            let (pods, client, tempdir) = runtime.block_on(setup_client());
            let mut client: Box<dyn RemoteFs> =
                Box::new(client.into_blocking(runtime.handle().clone()));
            let p = tempdir.join("blocking.txt");
            let mut src = std::io::Cursor::new(b"hello".to_vec());
            assert_eq!(
                client
                    .write_file(&p, &WriteOptions::default().size_hint(5), &mut src)
                    .unwrap(),
                5
            );
            let mut dest = std::io::Cursor::new(Vec::new());
            assert_eq!(
                client
                    .read_file(&p, &ReadOptions::default().offset(1), &mut dest)
                    .unwrap(),
                4
            );
            assert_eq!(dest.into_inner(), b"ello");
            let names: Vec<String> = client
                .list_dir(&tempdir)
                .unwrap()
                .into_iter()
                .map(|f| f.name())
                .collect();
            assert_eq!(names, vec!["blocking.txt".to_string()]);
            if let Err(err) = runtime.block_on(delete_test_pods(&pods)) {
                warn!("failed to clean up test pods: {err}");
            }
            assert!(client.disconnect().is_ok());
        }

        async fn setup_client() -> (Api<Pod>, KubeContainerFs, PathBuf) {
            crate::log_init();
            use kube::ResourceExt as _;
            use kube::api::PostParams;
            use kube::config::AuthInfo;

            let pod_name = generate_pod_name();
            let minikube_ip = std::env::var("MINIKUBE_IP").unwrap();

            debug!("setting up pod");
            let mut auth_info = AuthInfo {
                username: Some("minikube".to_string()),
                ..Default::default()
            };
            let home = std::env::var("HOME").unwrap();
            auth_info.client_certificate =
                Some(format!("{home}/.minikube/profiles/minikube/client.crt"));
            auth_info.client_key = Some(format!("{home}/.minikube/profiles/minikube/client.key"));
            debug!("Auth info: {auth_info:?}");

            let mut config = Config::new(format!("https://{minikube_ip}:8443").parse().unwrap());
            config.accept_invalid_certs = true;
            config.auth_info = auth_info;

            let client = Client::try_from(config.clone()).unwrap();
            let pods: Api<Pod> = Api::default_namespaced(client);

            let p: Pod = serde_json::from_value(serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": { "name": pod_name },
                "spec": {
                    "containers": [{
                      "name": "alpine",
                      "image": "alpine:3.20",
                      "imagePullPolicy": "IfNotPresent",
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
                    info!("Created {name}");
                }
                Err(kube::Error::Api(ae)) => assert_eq!(ae.code, 409),
                Err(e) => panic!("failed to create: {e}"),
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

            let mut client = KubeContainerFs::new(&pod_name, "alpine").config(config.clone());
            client.connect().await.expect("connection failed");
            let tempdir = PathBuf::from(generate_tempdir());
            client
                .create_dir(tempdir.as_path(), Some(UnixPex::from(0o775)))
                .await
                .expect("failed to create tempdir");
            (pods, client, tempdir)
        }

        async fn finalize_client(pods: Api<Pod>, mut client: KubeContainerFs) {
            if let Err(err) = delete_test_pods(&pods).await {
                warn!("failed to clean up test pods: {err}");
            }
            assert!(client.disconnect().await.is_ok());
        }

        /// Delete every pod named by [`generate_pod_name`], leaving pods
        /// created by other test runs alone. A single-node Minikube cluster
        /// runs out of room to schedule new ones after a couple dozen
        /// accumulate.
        async fn delete_test_pods(pods: &Api<Pod>) -> kube::Result<()> {
            use kube::ResourceExt as _;
            use kube::api::DeleteParams;

            for pod in pods.list(&Default::default()).await? {
                let name = pod.name_any();
                if name.starts_with("test-") {
                    pods.delete(&name, &DeleteParams::default()).await?;
                }
            }
            Ok(())
        }

        fn generate_pod_name() -> String {
            use rand::RngExt as _;
            use rand::distr::Alphanumeric;

            let mut rng = rand::rng();
            let random_string: String = std::iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .map(char::from)
                .filter(|c| c.is_alphabetic())
                .map(|c| c.to_ascii_lowercase())
                .take(12)
                .collect();
            format!("test-{random_string}")
        }

        fn generate_tempdir() -> String {
            use rand::RngExt;
            use rand::distr::Alphanumeric;

            let mut rng = rand::rng();
            let name: String = std::iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .map(char::from)
                .take(8)
                .collect();
            format!("/tmp/temp_{name}")
        }
    }
}
