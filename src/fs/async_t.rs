use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(feature = "find")]
use wildmatch::WildMatch;

use super::{
    File, Metadata, ReadStream, RemoteError, RemoteErrorType, UnixPex, Welcome, WriteStream,
};
use crate::RemoteResult;

/// Defines the methods which must be implemented in order to setup a Remote file system
///
/// AsyncRemoteFs doesn't allow the creation of trait objects, so it can't be used as a trait object
pub trait AsyncRemoteFs: Send {
    /// Connect to the remote server and authenticate.
    /// Can return banner / welcome message on success.
    /// If client has already established connection, then `AlreadyConnected` error is returned.
    fn connect(&mut self) -> impl std::future::Future<Output = RemoteResult<Welcome>> + Send;

    /// Disconnect from the remote server
    fn disconnect(&mut self) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Gets whether the client is connected to remote
    fn is_connected(&mut self) -> impl std::future::Future<Output = bool> + Send;

    /// Get working directory
    fn pwd(&mut self) -> impl std::future::Future<Output = RemoteResult<PathBuf>> + Send;

    /// Change working directory.
    /// Returns the realpath of new directory
    fn change_dir(
        &mut self,
        dir: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<PathBuf>> + Send;

    /// List directory entries at specified `path`
    fn list_dir(
        &mut self,
        path: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<Vec<File>>> + Send;

    /// Stat file at specified `path` and return Entry
    fn stat(&mut self, path: &Path)
        -> impl std::future::Future<Output = RemoteResult<File>> + Send;

    /// Set metadata for file at specified `path`
    fn setstat(
        &mut self,
        path: &Path,
        metadata: Metadata,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Returns whether file at specified `path` exists.
    fn exists(
        &mut self,
        path: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<bool>> + Send;

    /// Remove file at specified `path`.
    /// Fails if is not a file or doesn't exist
    fn remove_file(
        &mut self,
        path: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Remove directory at specified `path`
    /// Directory is removed only if empty
    fn remove_dir(
        &mut self,
        path: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Removes a directory at this path, after removing all its contents. **Use carefully!**
    ///
    /// If path is a `File`, file is removed anyway, as it was a file (after all, directories are files!)
    ///
    /// This function does not follow symbolic links and it will simply remove the symbolic link itself.
    ///
    /// ### Default implementation
    ///
    /// By default this method will combine `remove_file` and `remove_file` to remove all the content.
    /// Implement this method when there is a faster way to achieve this
    fn remove_dir_all(
        &mut self,
        path: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send {
        async {
            async fn remove_dir_all_impl<T: AsyncRemoteFs + ?Sized>(
                client: &mut T,
                path: &Path,
            ) -> RemoteResult<()> {
                if client.is_connected().await {
                    let path = crate::utils::path::absolutize(&client.pwd().await?, path);
                    debug!("Removing {}...", path.display());
                    let entry = client.stat(path.as_path()).await?;
                    if entry.is_dir() {
                        // list dir
                        debug!(
                            "{} is a directory; removing all directory entries",
                            entry.name()
                        );
                        let directory_content = client.list_dir(entry.path()).await?;
                        for entry in directory_content {
                            client.remove_dir_all(entry.path()).await?;
                        }
                        trace!(
                            "Removed all files in {}; removing directory",
                            entry.path().display()
                        );
                        client.remove_dir(entry.path()).await
                    } else {
                        client.remove_file(entry.path()).await
                    }
                } else {
                    Err(RemoteError::new(RemoteErrorType::NotConnected))
                }
            }

            remove_dir_all_impl(self, path).await
        }
    }

    /// Create a directory at `path` with specified mode.
    fn create_dir(
        &mut self,
        path: &Path,
        mode: UnixPex,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Create a symlink at `path` pointing at `target`
    fn symlink(
        &mut self,
        path: &Path,
        target: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Copy `src` to `dest`
    fn copy(
        &mut self,
        src: &Path,
        dest: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// move file/directory from `src` to `dest`
    fn mov(
        &mut self,
        src: &Path,
        dest: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send;

    /// Execute a command on remote host if supported by host.
    /// Returns command exit code and output (stdout)
    fn exec(
        &mut self,
        cmd: &str,
    ) -> impl std::future::Future<Output = RemoteResult<(u32, String)>> + Send;

    /// Open file at `path` for appending data.
    /// If the file doesn't exist, the file is created.
    ///
    /// ### ⚠️ Warning
    ///
    /// metadata should be the same of the local file.
    /// In some protocols, such as `scp` the `size` field is used to define the transfer size (required by the protocol)
    fn append(
        &mut self,
        path: &Path,
        metadata: &Metadata,
    ) -> impl std::future::Future<Output = RemoteResult<WriteStream>> + Send;

    /// Create file at path for write.
    /// If the file already exists, its content will be overwritten
    ///
    /// ### ⚠️ Warning
    ///
    /// metadata should be the same of the local file.
    /// In some protocols, such as `scp` the `size` field is used to define the transfer size (required by the protocol)
    fn create(
        &mut self,
        path: &Path,
        metadata: &Metadata,
    ) -> impl std::future::Future<Output = RemoteResult<WriteStream>> + Send;

    /// Open file at specified path for read.
    fn open(
        &mut self,
        path: &Path,
    ) -> impl std::future::Future<Output = RemoteResult<ReadStream>> + Send;

    /// Finalize `create_file` and `append_file` methods.
    /// This method must be implemented only if necessary; in case you don't need it, just return `Ok(())`
    /// The purpose of this method is to finalize the connection with the peer when writing data.
    /// This is necessary for some protocols such as FTP.
    /// You must call this method each time you want to finalize the write of the remote file.
    ///
    /// ### Default implementation
    ///
    /// By default this function returns already `Ok(())`
    fn on_written(
        &mut self,
        _writable: WriteStream,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send {
        async { Ok(()) }
    }

    /// Finalize `open_file` method.
    /// This method must be implemented only if necessary; in case you don't need it, just return `Ok(())`
    /// The purpose of this method is to finalize the connection with the peer when reading data.
    /// This might be necessary for some protocols.
    /// You must call this method each time you want to finalize the read of the remote file.
    ///
    /// ### Default implementation
    ///
    /// By default this function returns already `Ok(())`
    fn on_read(
        &mut self,
        _readable: ReadStream,
    ) -> impl std::future::Future<Output = RemoteResult<()>> + Send {
        async { Ok(()) }
    }

    /// Blocking implementation of `append`
    /// This method **SHOULD** be implemented **ONLY** when streams are not supported by the current file transfer.
    /// The developer using the client should FIRST try with `create` followed by `on_written`
    /// If the function returns error of kind `UnsupportedFeature`, then he should call this function.
    /// In case of success, returns the amount of bytes written to the remote file
    ///
    /// ### Default implementation
    ///
    /// By default this function uses the streams function to copy content from reader to writer
    fn append_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read + Send>,
    ) -> impl std::future::Future<Output = RemoteResult<u64>> + Send {
        async move {
            if self.is_connected().await {
                trace!("Opened remote file");
                let mut stream = self.append(path, metadata).await?;
                let sz = io::copy(&mut reader, &mut stream).map_err(|e| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string())
                })?;
                self.on_written(stream).await?;
                trace!("Written {} bytes to destination", sz);
                Ok(sz)
            } else {
                Err(RemoteError::new(RemoteErrorType::NotConnected))
            }
        }
    }

    /// Blocking implementation of `create`
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// The developer using the client should FIRST try with `create` followed by `on_written`
    /// If the function returns error of kind `UnsupportedFeature`, then he should call this function.
    /// In case of success, returns the amount of bytes written to the remote file
    ///
    /// ### Default implementation
    ///
    /// By default this function uses the streams function to copy content from reader to writer
    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read + Send>,
    ) -> impl std::future::Future<Output = RemoteResult<u64>> + Send {
        async move {
            if self.is_connected().await {
                let mut stream = self.create(path, metadata).await?;
                trace!("Opened remote file");
                let sz = io::copy(&mut reader, &mut stream).map_err(|e| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string())
                })?;
                self.on_written(stream).await?;
                trace!("Written {} bytes to destination", sz);
                Ok(sz)
            } else {
                Err(RemoteError::new(RemoteErrorType::NotConnected))
            }
        }
    }

    /// Blocking implementation of `open`
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// (since it would work thanks to the default implementation)
    /// The developer using the client should FIRST try with `open` followed by `on_sent`
    /// If the function returns error of kind `UnsupportedFeature`, then he should call this function.
    /// In case of success, returns the amount of bytes written to the local stream
    ///
    /// ### Default implementation
    ///
    /// By default this function uses the streams function to copy content from reader to writer
    fn open_file(
        &mut self,
        src: &Path,
        mut dest: Box<dyn Write + Send>,
    ) -> impl std::future::Future<Output = RemoteResult<u64>> + Send {
        async move {
            if self.is_connected().await {
                let mut stream = self.open(src).await?;
                trace!("File opened");
                let sz = io::copy(&mut stream, &mut dest).map_err(|e| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string())
                })?;
                self.on_read(stream).await?;
                trace!("Copied {} bytes to destination", sz);
                Ok(sz)
            } else {
                Err(RemoteError::new(RemoteErrorType::NotConnected))
            }
        }
    }

    /// Find files from current directory (in all subdirectories) whose name matches the provided search
    /// Search supports wildcards ('?', '*')
    #[cfg(feature = "find")]
    fn find(
        &mut self,
        search: &str,
    ) -> impl std::future::Future<Output = RemoteResult<Vec<File>>> + Send {
        async move {
            match self.is_connected().await {
                true => {
                    // Starting from current directory, iter dir
                    match self.pwd().await {
                        Ok(p) => self.iter_search(p.as_path(), &WildMatch::new(search)).await,
                        Err(err) => Err(err),
                    }
                }
                false => Err(RemoteError::new(RemoteErrorType::NotConnected)),
            }
        }
    }

    /// Search recursively in `dir` for file matching the wildcard.
    ///
    /// ### ⚠️ Warning
    ///
    /// NOTE: DON'T RE-IMPLEMENT THIS FUNCTION, unless the file transfer provides a faster way to do so
    /// NOTE: don't call this method from outside; consider it as private
    #[cfg(feature = "find")]
    fn iter_search(
        &mut self,
        dir: &Path,
        filter: &WildMatch,
    ) -> impl std::future::Future<Output = RemoteResult<Vec<File>>> + Send {
        async {
            async fn iter_search_impl<T: AsyncRemoteFs + ?Sized>(
                client: &mut T,
                dir: &Path,
                filter: &WildMatch,
            ) -> RemoteResult<Vec<File>> {
                let mut drained: Vec<File> = Vec::new();
                // Scan directory
                match client.list_dir(dir).await {
                    Ok(entries) => {
                        /* For each entry:
                        - if is dir: call iter_search with `dir`
                            - push `iter_search` result to `drained`
                        - if is file: check if it matches `filter`
                            - if it matches `filter`: push to to filter
                        */
                        for entry in entries.into_iter() {
                            if entry.is_dir() {
                                // If directory name, matches wildcard, push it to drained
                                if filter.matches(entry.name().as_str()) {
                                    drained.push(entry.clone());
                                }
                                drained
                                    .append(&mut client.iter_search(entry.path(), filter).await?);
                            } else if filter.matches(entry.name().as_str()) {
                                drained.push(entry);
                            }
                        }
                        Ok(drained)
                    }
                    Err(err) => Err(err),
                }
            }

            iter_search_impl(self, dir, filter).await
        }
    }
}
