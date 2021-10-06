//! ## Fs
//!
//! `fs` is the module which provides remote file system entities

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
// -- local
use self::params::RemoteParams;
// -- ext
use std::fmt;
use std::fs::File as FsFile;
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use wildmatch::WildMatch;
// -- modules
pub mod drivers;
mod file;
pub mod params;

// -- export
pub use file::{Directory, Entry, File, UnixPex};

/// ## RemoteError
///
/// RemoteError defines the possible errors available for a file transfer
#[derive(Debug)]
pub struct RemoteError {
    code: RemoteErrorType,
    msg: Option<String>,
}

/// ## RemoteErrorType
///
/// RemoteErrorType defines the possible errors available for a file transfer
#[derive(Error, Debug, Clone, Copy, PartialEq)]
pub enum RemoteErrorType {
    #[error("Authentication failed")]
    AuthenticationFailed,
    #[error("Bad address syntax")]
    BadAddress,
    #[error("Connection error")]
    ConnectionError,
    #[error("SSL error")]
    SslError,
    #[error("Could not stat directory")]
    DirStatFailed,
    #[error("Directory already exists")]
    DirectoryAlreadyExists,
    #[error("Failed to create file")]
    FileCreateDenied,
    #[error("No such file or directory")]
    NoSuchFileOrDirectory,
    #[error("Not enough permissions")]
    PexError,
    #[error("Protocol error")]
    ProtocolError,
    #[error("Uninitialized session")]
    UninitializedSession,
    #[error("Unsupported feature")]
    UnsupportedFeature,
}

impl RemoteError {
    /// ### new
    ///
    /// Instantiates a new RemoteError
    pub fn new(code: RemoteErrorType) -> RemoteError {
        RemoteError { code, msg: None }
    }

    /// ### new_ex
    ///
    /// Instantiates a new RemoteError with message
    pub fn new_ex(code: RemoteErrorType, msg: String) -> RemoteError {
        let mut err: RemoteError = RemoteError::new(code);
        err.msg = Some(msg);
        err
    }

    /// ### kind
    ///
    /// Returns the error kind
    pub fn kind(&self) -> RemoteErrorType {
        self.code
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.msg {
            Some(msg) => write!(f, "{} ({})", self.code, msg),
            None => write!(f, "{}", self.code),
        }
    }
}

/// ## RemoteResult
///
/// Result type returned by a `FileTransfer` implementation
pub type RemoteResult<T> = Result<T, RemoteError>;

/// ## RemoteFileSystem
///
/// Defines the methods which must be implemented in order to setup a Remote file system
pub trait RemoteFileSystem {
    /// ### connect
    ///
    /// Connect to the remote server
    /// Can return banner / welcome message on success
    fn connect(&mut self, params: &RemoteParams) -> RemoteResult<Option<String>>;

    /// ### disconnect
    ///
    /// Disconnect from the remote server
    fn disconnect(&mut self) -> RemoteResult<()>;

    /// ### is_connected
    ///
    /// Indicates whether the client is connected to remote
    fn is_connected(&self) -> bool;

    /// ### pwd
    ///
    /// Print working directory
    fn pwd(&mut self) -> RemoteResult<PathBuf>;

    /// ### change_dir
    ///
    /// Change working directory.
    /// Returns the realpath of new directory
    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf>;

    /// ### list_dir
    ///
    /// List directory entries at `path`
    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<Entry>>;

    /// ### stat
    ///
    /// Stat file at `path` and return Entry
    fn stat(&mut self, path: &Path) -> RemoteResult<Entry>;

    /// ### exists
    ///
    /// Returns whether file at `path` exists.
    fn exists(&mut self, path: &Path) -> RemoteResult<bool>;

    /// ### remove_file
    ///
    /// Remove file at `path`
    fn remove_file(&mut self, path: &Path) -> RemoteResult<()>;

    /// ### remove_dir
    ///
    /// Remove directory at `path`
    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()>;

    /// ### create_dir
    ///
    /// Create a directory at `path`
    fn create_dir(&mut self, path: &Path) -> RemoteResult<()>;

    /// ### copy
    ///
    /// Copy `src` to `dest`
    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// ### mov
    ///
    /// move file/directory from `src` to `dest`
    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// ### exec
    ///
    /// Execute a command on remote host if supported by host.
    /// Returns command exit code and output
    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)>;

    /// ### append_file
    ///
    /// Open file at `path` for appending data.
    fn append_file(&mut self, path: &Path) -> RemoteResult<Box<dyn Write>>;

    /// ### create_file
    ///
    /// Create file at path for write.
    fn create_file(&mut self, path: &Path) -> RemoteResult<Box<dyn Write>>;

    /// ### open_file
    ///
    /// Open file at path for read.
    fn open_file(&mut self, path: &Path) -> RemoteResult<Box<dyn Read>>;

    /// ### on_written
    ///
    /// Finalize `create_file` and `append_file` methods.
    /// This method must be implemented only if necessary; in case you don't need it, just return `Ok(())`
    /// The purpose of this method is to finalize the connection with the peer when writing data.
    /// This is necessary for some protocols such as FTP.
    /// You must call this method each time you want to finalize the write of the remote file.
    /// By default this function returns already `Ok(())`
    fn on_written(&mut self, _writable: Box<dyn Write>) -> RemoteResult<()> {
        Ok(())
    }

    /// ### on_read
    ///
    /// Finalize `open_file` method.
    /// This method must be implemented only if necessary; in case you don't need it, just return `Ok(())`
    /// The purpose of this method is to finalize the connection with the peer when reading data.
    /// This mighe be necessary for some protocols.
    /// You must call this method each time you want to finalize the read of the remote file.
    /// By default this function returns already `Ok(())`
    fn on_read(&mut self, _readable: Box<dyn Read>) -> RemoteResult<()> {
        Ok(())
    }

    /// ### append_file_block
    ///
    /// Append content of `file` to remote `path` blocking.
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// The developer implementing the Remote file system should FIRST try with `create_file` followed by `on_written`
    /// If the function returns error kind() `UnsupportedFeature`, then he should call this function.
    /// By default this function uses the streams function to copy content from reader to writer
    fn append_file_block(
        &mut self,
        path: &Path,
        file: &mut FsFile,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<()> {
        match self.is_connected() {
            true => {
                let mut stream = self.append_file(path)?;
                io::copy(&mut reader, &mut stream).map_err(|e| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string())
                })?;
                self.on_written(stream)
            }
            false => Err(RemoteError::new(RemoteErrorType::UninitializedSession)),
        }
    }

    /// ### create_file_block
    ///
    /// Create a file on remote blocking.
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// The developer implementing the Remote file system should FIRST try with `create_file` followed by `on_written`
    /// If the function returns error kind() `UnsupportedFeature`, then he should call this function.
    /// By default this function uses the streams function to copy content from reader to writer
    fn create_file_block(
        &mut self,
        path: &Path,
        file: &mut FsFile,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<()> {
        match self.is_connected() {
            true => {
                let mut stream = self.create_file(path)?;
                io::copy(&mut reader, &mut stream).map_err(|e| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string())
                })?;
                self.on_written(stream)
            }
            false => Err(RemoteError::new(RemoteErrorType::UninitializedSession)),
        }
    }

    /// ### open_file_block
    ///
    /// Receive a file from remote WITHOUT using streams. So this function is blocking
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// (since it would work thanks to the default implementation)
    /// The developer implementing the filetransfer user should FIRST try with `send_file` followed by `on_sent`
    /// If the function returns error kind() `UnsupportedFeature`, then he should call this function.
    /// For safety reasons this function doesn't accept the `Write` trait, but the destination path.
    /// By default this function uses the streams function to copy content from reader to writer
    fn open_file_block(&mut self, src: &Path, dest: &mut FsFile) -> RemoteResult<()> {
        match self.is_connected() {
            true => {
                let mut stream = self.open_file(src)?;
                io::copy(&mut stream, dest).map(|_| ()).map_err(|e| {
                    RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string())
                })?;
                self.on_read(stream)
            }
            false => Err(RemoteError::new(RemoteErrorType::UninitializedSession)),
        }
    }

    /// ### find
    ///
    /// Find files from current directory (in all subdirectories) whose name matches the provided search
    /// Search supports wildcards ('?', '*')
    fn find(&mut self, search: &str) -> RemoteResult<Vec<Entry>> {
        match self.is_connected() {
            true => {
                // Starting from current directory, iter dir
                match self.pwd() {
                    Ok(p) => self.iter_search(p.as_path(), &WildMatch::new(search)),
                    Err(err) => Err(err),
                }
            }
            false => Err(RemoteError::new(RemoteErrorType::UninitializedSession)),
        }
    }

    /// ### iter_search
    ///
    /// Search recursively in `dir` for file matching the wildcard.
    /// NOTE: DON'T RE-IMPLEMENT THIS FUNCTION, unless the file transfer provides a faster way to do so
    /// NOTE: don't call this method from outside; consider it as private
    fn iter_search(&mut self, dir: &Path, filter: &WildMatch) -> RemoteResult<Vec<Entry>> {
        let mut drained: Vec<Entry> = Vec::new();
        // Scan directory
        match self.list_dir(dir) {
            Ok(entries) => {
                /* For each entry:
                - if is dir: call iter_search with `dir`
                    - push `iter_search` result to `drained`
                - if is file: check if it matches `filter`
                    - if it matches `filter`: push to to filter
                */
                for entry in entries.iter() {
                    match entry {
                        Entry::Directory(dir) => {
                            // If directory name, matches wildcard, push it to drained
                            if filter.matches(dir.name.as_str()) {
                                drained.push(Entry::Directory(dir.clone()));
                            }
                            drained.append(&mut self.iter_search(dir.abs_path.as_path(), filter)?);
                        }
                        Entry::File(file) => {
                            if filter.matches(file.name.as_str()) {
                                drained.push(Entry::File(file.clone()));
                            }
                        }
                    }
                }
                Ok(drained)
            }
            Err(err) => Err(err),
        }
    }
}

trait IterSearch: RemoteFileSystem {}

#[cfg(test)]
mod tests {

    use super::*;

    use pretty_assertions::assert_eq;

    #[test]
    fn test_filetransfer_mod_error() {
        let err: RemoteError = RemoteError::new_ex(
            RemoteErrorType::NoSuchFileOrDirectory,
            String::from("non va una mazza"),
        );
        assert_eq!(*err.msg.as_ref().unwrap(), String::from("non va una mazza"));
        assert_eq!(
            format!("{}", err),
            String::from("No such file or directory (non va una mazza)")
        );
        assert_eq!(
            format!(
                "{}",
                RemoteError::new(RemoteErrorType::AuthenticationFailed)
            ),
            String::from("Authentication failed")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::BadAddress)),
            String::from("Bad address syntax")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::ConnectionError)),
            String::from("Connection error")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::DirStatFailed)),
            String::from("Could not stat directory")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::FileCreateDenied)),
            String::from("Failed to create file")
        );
        assert_eq!(
            format!(
                "{}",
                RemoteError::new(RemoteErrorType::NoSuchFileOrDirectory)
            ),
            String::from("No such file or directory")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::PexError)),
            String::from("Not enough permissions")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::ProtocolError)),
            String::from("Protocol error")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::SslError)),
            String::from("SSL error")
        );
        assert_eq!(
            format!(
                "{}",
                RemoteError::new(RemoteErrorType::UninitializedSession)
            ),
            String::from("Uninitialized session")
        );
        assert_eq!(
            format!("{}", RemoteError::new(RemoteErrorType::UnsupportedFeature)),
            String::from("Unsupported feature")
        );
        let err = RemoteError::new(RemoteErrorType::UnsupportedFeature);
        assert_eq!(err.kind(), RemoteErrorType::UnsupportedFeature);
    }
}
