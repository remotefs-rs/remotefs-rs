//! Observable in-memory fixtures for the filesystem contract tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(feature = "async")]
use crate::fs::{AsyncReadStream, AsyncRemoteFs, AsyncWriteStream};
use crate::fs::{
    Capabilities, ExecOutput, File, FileType, Metadata, ReadOptions, ReadStream, RemoteError,
    RemoteErrorType, RemoteFs, SetMetadata, UnixPex, Welcome, WriteOptions, WriteStream,
};

#[cfg(feature = "async")]
pub(crate) mod async_io;
mod stream;
mod tests;

pub(crate) use stream::{MockReader, MockWriter};

struct Entry {
    metadata: Metadata,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct State {
    entries: BTreeMap<PathBuf, Entry>,
    capabilities: Capabilities,
    fail_finish: Option<RemoteErrorType>,
    finish_count: usize,
    unfinished_count: usize,
    require_size_hint: bool,
}

/// An in-memory filesystem used to test default contract behavior.
pub(crate) struct MockRemoteFs {
    state: Arc<Mutex<State>>,
    connected: Arc<AtomicBool>,
}

impl MockRemoteFs {
    /// Creates a disconnected in-memory filesystem.
    pub(crate) fn new() -> Self {
        let state = Arc::new(Mutex::new(State::new()));
        Self {
            state,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Creates an already-connected in-memory filesystem.
    pub(crate) fn connected() -> Self {
        let filesystem = Self::new();
        filesystem.connected.store(true, Ordering::Release);
        filesystem
    }

    /// Creates a platform-rooted path for a fixture.
    pub(crate) fn path(path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            return path.to_path_buf();
        }
        #[cfg(unix)]
        let root = Path::new("/");
        #[cfg(windows)]
        let root = Path::new(r"C:\");
        root.join(path)
    }

    /// Adds a regular file to the fixture.
    pub(crate) fn seed_file(&self, path: &Path, bytes: &[u8]) {
        self.assert_absolute(path);
        let mut state = self.lock_state();
        Self::assert_parent_directory(&state, path);
        state.entries.insert(
            path.to_path_buf(),
            Entry {
                metadata: Metadata::default()
                    .file_type(FileType::File)
                    .size(bytes.len() as u64),
                bytes: bytes.to_vec(),
            },
        );
    }

    /// Adds an empty directory to the fixture.
    pub(crate) fn seed_dir(&self, path: &Path) {
        self.assert_absolute(path);
        let mut state = self.lock_state();
        Self::assert_parent_directory(&state, path);
        state.entries.insert(
            path.to_path_buf(),
            Entry {
                metadata: Metadata::default().file_type(FileType::Directory).size(0),
                bytes: Vec::new(),
            },
        );
    }

    /// Adds a symbolic link without resolving its target.
    pub(crate) fn seed_symlink(&self, path: &Path, target: &Path) {
        self.assert_absolute(path);
        self.assert_absolute(target);
        let mut state = self.lock_state();
        Self::assert_parent_directory(&state, path);
        state.entries.insert(
            path.to_path_buf(),
            Entry {
                metadata: Metadata::default()
                    .file_type(FileType::Symlink)
                    .symlink(target),
                bytes: Vec::new(),
            },
        );
    }

    /// Returns the bytes stored at a regular fixture file.
    pub(crate) fn contents(&self, path: &Path) -> Vec<u8> {
        self.lock_state()
            .entries
            .get(path)
            .map(|entry| entry.bytes.clone())
            .expect("fixture file must exist")
    }

    /// Returns the number of stream finalizers that ran.
    pub(crate) fn finish_count(&self) -> usize {
        self.lock_state().finish_count
    }

    /// Returns the number of streams dropped without finalization.
    pub(crate) fn unfinished_count(&self) -> usize {
        self.lock_state().unfinished_count
    }

    /// Injects a finalization result for subsequently completed streams.
    pub(crate) fn fail_finish(&self, error: Option<RemoteErrorType>) {
        self.lock_state().fail_finish = error;
    }

    /// Sets the capabilities advertised by this fixture.
    pub(crate) fn set_capabilities(&self, capabilities: Capabilities) {
        self.lock_state().capabilities = capabilities;
    }

    /// Requires a size hint before opening write streams when enabled.
    pub(crate) fn require_size_hint(&self, required: bool) {
        self.lock_state().require_size_hint = required;
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("mock state lock poisoned")
    }

    fn assert_absolute(&self, path: &Path) {
        assert!(
            crate::path::ensure_absolute(path).is_ok(),
            "fixture paths must be absolute"
        );
    }

    fn assert_parent_directory(state: &State, path: &Path) {
        let parent = path.parent().expect("fixture path must have a parent");
        assert!(
            state
                .entries
                .get(parent)
                .is_some_and(|entry| entry.metadata.is_dir()),
            "parent directory must exist"
        );
    }

    fn ensure_connected(&self) -> crate::RemoteResult<()> {
        if <Self as RemoteFs>::is_connected(self) {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn has_capability(&self, capability: Capabilities) -> bool {
        self.lock_state().capabilities.contains(capability)
    }

    fn unsupported<T>() -> crate::RemoteResult<T> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn missing() -> RemoteError {
        RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)
    }

    fn path_has_direct_child(path: &Path, parent: &Path) -> bool {
        path.parent() == Some(parent)
    }

    fn ranged_bytes(bytes: &[u8], opts: &ReadOptions) -> Vec<u8> {
        let length = bytes.len() as u64;
        let offset = opts.offset.unwrap_or(0).min(length);
        let end = opts
            .length
            .map(|requested| offset.saturating_add(requested).min(length))
            .unwrap_or(length);
        bytes[offset as usize..end as usize].to_vec()
    }
}

impl State {
    fn new() -> Self {
        let mut entries = BTreeMap::new();
        #[cfg(unix)]
        let root = PathBuf::from("/");
        #[cfg(windows)]
        let root = PathBuf::from(r"C:\");
        entries.insert(
            root,
            Entry {
                metadata: Metadata::default().file_type(FileType::Directory).size(0),
                bytes: Vec::new(),
            },
        );
        Self {
            entries,
            capabilities: Capabilities::all(),
            ..Self::default()
        }
    }
}

impl Capabilities {
    fn all() -> Self {
        Self::empty()
            | Self::STREAM_READ
            | Self::STREAM_WRITE
            | Self::APPEND
            | Self::RANGE_READ
            | Self::SEEK_READ
            | Self::SEEK_WRITE
            | Self::COPY
            | Self::SYMLINK
            | Self::SET_METADATA
            | Self::POSIX_MODE
            | Self::EXEC
    }
}

impl RemoteFs for MockRemoteFs {
    fn connect(&mut self) -> crate::RemoteResult<Welcome> {
        if self.connected.swap(true, Ordering::AcqRel) {
            Err(RemoteError::new(RemoteErrorType::AlreadyConnected))
        } else {
            Ok(Welcome::default())
        }
    }

    fn disconnect(&mut self) -> crate::RemoteResult<()> {
        if self.connected.swap(false, Ordering::AcqRel) {
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    fn capabilities(&self) -> Capabilities {
        self.lock_state().capabilities
    }

    fn list_dir(&self, path: &Path) -> crate::RemoteResult<Vec<File>> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        let state = self.lock_state();
        let entry = state.entries.get(path).ok_or_else(Self::missing)?;
        if !entry.metadata.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        Ok(state
            .entries
            .iter()
            .filter(|(child, _)| Self::path_has_direct_child(child, path))
            .map(|(child, entry)| File::new(child.clone(), entry.metadata.clone()))
            .collect())
    }

    fn stat(&self, path: &Path) -> crate::RemoteResult<File> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        self.lock_state()
            .entries
            .get(path)
            .map(|entry| File::new(path, entry.metadata.clone()))
            .ok_or_else(Self::missing)
    }

    fn exists(&self, path: &Path) -> crate::RemoteResult<bool> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        Ok(self.lock_state().entries.contains_key(path))
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::SET_METADATA) {
            return Self::unsupported();
        }
        let mut state = self.lock_state();
        let entry = state.entries.get_mut(path).ok_or_else(Self::missing)?;
        if let Some(mode) = metadata.mode {
            entry.metadata.mode = Some(mode);
        }
        if let Some(uid) = metadata.uid {
            entry.metadata.uid = Some(uid);
        }
        if let Some(gid) = metadata.gid {
            entry.metadata.gid = Some(gid);
        }
        if let Some(accessed) = metadata.accessed {
            entry.metadata.accessed = Some(accessed);
        }
        if let Some(modified) = metadata.modified {
            entry.metadata.modified = Some(modified);
        }
        Ok(())
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        let mut state = self.lock_state();
        if state.entries.contains_key(path) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        let parent = path
            .parent()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::InvalidPath))?;
        if !state
            .entries
            .get(parent)
            .is_some_and(|entry| entry.metadata.is_dir())
        {
            return Err(Self::missing());
        }
        let mut metadata = Metadata::default().file_type(FileType::Directory).size(0);
        if state.capabilities.contains(Capabilities::POSIX_MODE) {
            metadata.mode = mode;
        }
        state.entries.insert(
            path.to_path_buf(),
            Entry {
                metadata,
                bytes: Vec::new(),
            },
        );
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        let mut state = self.lock_state();
        let entry = state.entries.get(path).ok_or_else(Self::missing)?;
        if entry.metadata.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        state.entries.remove(path);
        Ok(())
    }

    fn remove_dir(&self, path: &Path) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        let mut state = self.lock_state();
        let entry = state.entries.get(path).ok_or_else(Self::missing)?;
        if !entry.metadata.is_dir() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        if state
            .entries
            .keys()
            .any(|child| Self::path_has_direct_child(child, path))
        {
            return Err(RemoteError::new(RemoteErrorType::DirectoryNotEmpty));
        }
        state.entries.remove(path);
        Ok(())
    }

    fn rename(&self, src: &Path, dest: &Path) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(src)?;
        crate::path::ensure_absolute(dest)?;
        self.ensure_connected()?;
        let mut state = self.lock_state();
        if !state.entries.contains_key(src) {
            return Err(Self::missing());
        }
        if state.entries.contains_key(dest) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        let parent = dest
            .parent()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::InvalidPath))?;
        if !state
            .entries
            .get(parent)
            .is_some_and(|entry| entry.metadata.is_dir())
        {
            return Err(Self::missing());
        }
        if dest.starts_with(src) {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let moved: Vec<_> = state
            .entries
            .range::<PathBuf, _>(src.to_path_buf()..)
            .take_while(|(path, _)| path.starts_with(src))
            .map(|(path, entry)| (path.clone(), entry.metadata.clone(), entry.bytes.clone()))
            .collect();
        for (path, _, _) in &moved {
            state.entries.remove(path);
        }
        for (path, metadata, bytes) in moved {
            let suffix = path.strip_prefix(src).expect("source prefix was checked");
            state
                .entries
                .insert(dest.join(suffix), Entry { metadata, bytes });
        }
        Ok(())
    }

    fn copy(&self, src: &Path, dest: &Path) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(src)?;
        crate::path::ensure_absolute(dest)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::COPY) {
            return Self::unsupported();
        }
        let mut state = self.lock_state();
        if !state.entries.contains_key(src) {
            return Err(Self::missing());
        }
        if state.entries.contains_key(dest) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        let parent = dest
            .parent()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::InvalidPath))?;
        if !state
            .entries
            .get(parent)
            .is_some_and(|entry| entry.metadata.is_dir())
        {
            return Err(Self::missing());
        }
        let copies: Vec<_> = state
            .entries
            .range::<PathBuf, _>(src.to_path_buf()..)
            .take_while(|(path, _)| path.starts_with(src))
            .map(|(path, entry)| (path.clone(), entry.metadata.clone(), entry.bytes.clone()))
            .collect();
        for (path, metadata, bytes) in copies {
            let suffix = path.strip_prefix(src).expect("source prefix was checked");
            state
                .entries
                .insert(dest.join(suffix), Entry { metadata, bytes });
        }
        Ok(())
    }

    fn symlink(&self, path: &Path, target: &Path) -> crate::RemoteResult<()> {
        crate::path::ensure_absolute(path)?;
        crate::path::ensure_absolute(target)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::SYMLINK) {
            return Self::unsupported();
        }
        let mut state = self.lock_state();
        if state.entries.contains_key(path) {
            return Err(RemoteError::new(RemoteErrorType::AlreadyExists));
        }
        let parent = path
            .parent()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::InvalidPath))?;
        if !state
            .entries
            .get(parent)
            .is_some_and(|entry| entry.metadata.is_dir())
        {
            return Err(Self::missing());
        }
        state.entries.insert(
            path.to_path_buf(),
            Entry {
                metadata: Metadata::default()
                    .file_type(FileType::Symlink)
                    .symlink(target),
                bytes: Vec::new(),
            },
        );
        Ok(())
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> crate::RemoteResult<ReadStream> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::STREAM_READ) {
            return Self::unsupported();
        }
        let state = self.lock_state();
        let entry = state.entries.get(path).ok_or_else(Self::missing)?;
        if entry.metadata.is_dir() || entry.metadata.is_symlink() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let bytes = Self::ranged_bytes(&entry.bytes, opts);
        let seekable = state.capabilities.contains(Capabilities::SEEK_READ);
        Ok(ReadStream::new(MockReader::new(
            self.state.clone(),
            bytes,
            seekable,
        )))
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> crate::RemoteResult<WriteStream> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::STREAM_WRITE) {
            return Self::unsupported();
        }
        self.open_writer(path, opts, false)
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> crate::RemoteResult<WriteStream> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::APPEND) {
            return Self::unsupported();
        }
        self.open_writer(path, opts, true)
    }

    fn exec(&self, _cmd: &str) -> crate::RemoteResult<ExecOutput> {
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::EXEC) {
            return Self::unsupported().map(|()| unreachable!());
        }
        Ok(ExecOutput::new(0, ""))
    }
}

#[cfg(feature = "async")]
#[async_trait::async_trait]
impl AsyncRemoteFs for MockRemoteFs {
    async fn connect(&mut self) -> crate::RemoteResult<Welcome> {
        <Self as RemoteFs>::connect(self)
    }

    async fn disconnect(&mut self) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::disconnect(self)
    }

    fn is_connected(&self) -> bool {
        <Self as RemoteFs>::is_connected(self)
    }

    fn capabilities(&self) -> Capabilities {
        <Self as RemoteFs>::capabilities(self)
    }

    async fn list_dir(&self, path: &Path) -> crate::RemoteResult<Vec<File>> {
        <Self as RemoteFs>::list_dir(self, path)
    }

    async fn stat(&self, path: &Path) -> crate::RemoteResult<File> {
        <Self as RemoteFs>::stat(self, path)
    }

    async fn exists(&self, path: &Path) -> crate::RemoteResult<bool> {
        <Self as RemoteFs>::exists(self, path)
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::set_metadata(self, path, metadata)
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::create_dir(self, path, mode)
    }

    async fn remove_file(&self, path: &Path) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::remove_file(self, path)
    }

    async fn remove_dir(&self, path: &Path) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::remove_dir(self, path)
    }

    async fn rename(&self, src: &Path, dest: &Path) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::rename(self, src, dest)
    }

    async fn copy(&self, src: &Path, dest: &Path) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::copy(self, src, dest)
    }

    async fn symlink(&self, path: &Path, target: &Path) -> crate::RemoteResult<()> {
        <Self as RemoteFs>::symlink(self, path, target)
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> crate::RemoteResult<AsyncReadStream> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::STREAM_READ) {
            return Self::unsupported();
        }
        let state = self.lock_state();
        let entry = state.entries.get(path).ok_or_else(Self::missing)?;
        if entry.metadata.is_dir() || entry.metadata.is_symlink() {
            return Err(RemoteError::new(RemoteErrorType::BadFile));
        }
        let bytes = Self::ranged_bytes(&entry.bytes, opts);
        let seekable = state.capabilities.contains(Capabilities::SEEK_READ);
        Ok(AsyncReadStream::new(
            crate::mock::async_io::AsyncMockReader::new(self.state.clone(), bytes, seekable),
        ))
    }

    async fn create(
        &self,
        path: &Path,
        opts: &WriteOptions,
    ) -> crate::RemoteResult<AsyncWriteStream> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::STREAM_WRITE) {
            return Self::unsupported();
        }
        self.open_async_writer(path, opts, false)
    }

    async fn append(
        &self,
        path: &Path,
        opts: &WriteOptions,
    ) -> crate::RemoteResult<AsyncWriteStream> {
        crate::path::ensure_absolute(path)?;
        self.ensure_connected()?;
        if !self.has_capability(Capabilities::APPEND) {
            return Self::unsupported();
        }
        self.open_async_writer(path, opts, true)
    }

    async fn exec(&self, cmd: &str) -> crate::RemoteResult<ExecOutput> {
        <Self as RemoteFs>::exec(self, cmd)
    }
}

impl MockRemoteFs {
    fn open_writer(
        &self,
        path: &Path,
        opts: &WriteOptions,
        append: bool,
    ) -> crate::RemoteResult<WriteStream> {
        let state = self.lock_state();
        if state.require_size_hint && opts.size_hint.is_none() {
            return Err(RemoteError::new(RemoteErrorType::SizeRequired));
        }
        let (bytes, metadata) = match state.entries.get(path) {
            Some(entry) if entry.metadata.is_dir() => {
                return Err(RemoteError::new(RemoteErrorType::BadFile));
            }
            Some(entry) if append => (entry.bytes.clone(), entry.metadata.clone()),
            Some(entry) => (Vec::new(), entry.metadata.clone()),
            None => {
                let parent = path
                    .parent()
                    .ok_or_else(|| RemoteError::new(RemoteErrorType::InvalidPath))?;
                if !state
                    .entries
                    .get(parent)
                    .is_some_and(|entry| entry.metadata.is_dir())
                {
                    return Err(Self::missing());
                }
                (Vec::new(), Metadata::default())
            }
        };
        let seekable = state.capabilities.contains(Capabilities::SEEK_WRITE);
        let mut metadata = metadata;
        if let Some(mode) = opts.mode {
            metadata.mode = Some(mode);
        }
        if let Some(modified) = opts.modified {
            metadata.modified = Some(modified);
        }
        let mut writer = MockWriter::new(
            self.state.clone(),
            path.to_path_buf(),
            bytes,
            metadata,
            seekable,
        );
        if append {
            writer.seek_to_end();
        }
        Ok(WriteStream::new(writer))
    }

    #[cfg(feature = "async")]
    fn open_async_writer(
        &self,
        path: &Path,
        opts: &WriteOptions,
        append: bool,
    ) -> crate::RemoteResult<AsyncWriteStream> {
        let state = self.lock_state();
        if state.require_size_hint && opts.size_hint.is_none() {
            return Err(RemoteError::new(RemoteErrorType::SizeRequired));
        }
        let (bytes, metadata) = match state.entries.get(path) {
            Some(entry) if entry.metadata.is_dir() => {
                return Err(RemoteError::new(RemoteErrorType::BadFile));
            }
            Some(entry) if append => (entry.bytes.clone(), entry.metadata.clone()),
            Some(entry) => (Vec::new(), entry.metadata.clone()),
            None => {
                let parent = path
                    .parent()
                    .ok_or_else(|| RemoteError::new(RemoteErrorType::InvalidPath))?;
                if !state
                    .entries
                    .get(parent)
                    .is_some_and(|entry| entry.metadata.is_dir())
                {
                    return Err(Self::missing());
                }
                (Vec::new(), Metadata::default())
            }
        };
        let seekable = state.capabilities.contains(Capabilities::SEEK_WRITE);
        let mut metadata = metadata;
        if let Some(mode) = opts.mode {
            metadata.mode = Some(mode);
        }
        if let Some(modified) = opts.modified {
            metadata.modified = Some(modified);
        }
        let mut writer = crate::mock::async_io::AsyncMockWriter::new(
            self.state.clone(),
            path.to_path_buf(),
            bytes,
            metadata,
            seekable,
        );
        if append {
            writer.seek_to_end();
        }
        Ok(AsyncWriteStream::new(writer))
    }
}
