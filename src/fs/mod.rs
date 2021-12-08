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
// -- ext
use std::io;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use wildmatch::WildMatch;
// -- modules
mod errors;
mod file;
mod welcome;

// -- export
pub use errors::{RemoteError, RemoteErrorType, RemoteResult};
pub use file::{Directory, Entry, File, Metadata, UnixPex, UnixPexClass};
pub use welcome::Welcome;

/// Defines the methods which must be implemented in order to setup a Remote file system
pub trait RemoteFs {
    /// Connect to the remote server and authenticate.
    /// Can return banner / welcome message on success.
    /// If client has already established connection, then `AlreadyConnected` error is returned.
    fn connect(&mut self) -> RemoteResult<Welcome>;

    /// Disconnect from the remote server
    fn disconnect(&mut self) -> RemoteResult<()>;

    /// Indicates whether the client is connected to remote
    fn is_connected(&self) -> bool;

    /// Print working directory
    fn pwd(&mut self) -> RemoteResult<PathBuf>;

    /// Change working directory.
    /// Returns the realpath of new directory
    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf>;

    /// List directory entries at `path`
    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<Entry>>;

    /// Stat file at `path` and return Entry
    fn stat(&mut self, path: &Path) -> RemoteResult<Entry>;

    /// Set metadata for file at `path`
    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()>;

    /// Returns whether file at `path` exists.
    fn exists(&mut self, path: &Path) -> RemoteResult<bool>;

    /// Remove file at `path`
    fn remove_file(&mut self, path: &Path) -> RemoteResult<()>;

    /// Remove directory at `path`
    /// Directory is removed only if empty
    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()>;

    /// Removes a directory at this path, after removing all its contents. **Use carefully!**
    ///
    /// If path is a `File`, file is removed anyway, as it was a file (after all, directories are files!)
    ///
    /// This function does not follow symbolic links and it will simply remove the symbolic link itself.
    ///
    /// By default this method will combine remove_dir and remove_file to remove all the content.
    /// Implement this method when there is a faster way to achieve this
    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        if self.is_connected() {
            let path = crate::utils::path::absolutize(&self.pwd()?, path);
            debug!("Removing {}...", path.display());
            let entry = self.stat(path.as_path())?;
            match entry {
                Entry::File(_) => self.remove_file(entry.path()),
                Entry::Directory(d) => {
                    // list dir
                    debug!("{} is a directory; removing all directory entries", d.name);
                    let directory_content = self.list_dir(d.abs_path.as_path())?;
                    for entry in directory_content.iter() {
                        self.remove_dir_all(entry.path())?;
                    }
                    trace!(
                        "Removed all files in {}; removing directory",
                        d.abs_path.display()
                    );
                    self.remove_dir(d.abs_path.as_path())
                }
            }
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Create a directory at `path` with specified mode.
    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()>;

    /// Create a symlink at `path` pointing at `target`
    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()>;

    /// Copy `src` to `dest`
    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// move file/directory from `src` to `dest`
    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()>;

    /// Execute a command on remote host if supported by host.
    /// Returns command exit code and output
    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)>;

    /// Open file at `path` for appending data.
    ///
    /// Warning: metadata should be the same of the local file.
    /// In some protocols, such as `scp` the `size` field is used to define the transfer size (required by the protocol)
    fn append(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<Box<dyn Write>>;

    /// Create file at path for write.
    /// If the file already exists, its content will be overwritten
    ///
    /// Warning: metadata should be the same of the local file.
    /// In some protocols, such as `scp` the `size` field is used to define the transfer size (required by the protocol)
    fn create(&mut self, path: &Path, metadata: &Metadata) -> RemoteResult<Box<dyn Write>>;

    /// Open file at path for read.
    fn open(&mut self, path: &Path) -> RemoteResult<Box<dyn Read>>;

    /// Finalize `create_file` and `append_file` methods.
    /// This method must be implemented only if necessary; in case you don't need it, just return `Ok(())`
    /// The purpose of this method is to finalize the connection with the peer when writing data.
    /// This is necessary for some protocols such as FTP.
    /// You must call this method each time you want to finalize the write of the remote file.
    /// By default this function returns already `Ok(())`
    fn on_written(&mut self, _writable: Box<dyn Write>) -> RemoteResult<()> {
        Ok(())
    }

    /// Finalize `open_file` method.
    /// This method must be implemented only if necessary; in case you don't need it, just return `Ok(())`
    /// The purpose of this method is to finalize the connection with the peer when reading data.
    /// This mighe be necessary for some protocols.
    /// You must call this method each time you want to finalize the read of the remote file.
    /// By default this function returns already `Ok(())`
    fn on_read(&mut self, _readable: Box<dyn Read>) -> RemoteResult<()> {
        Ok(())
    }

    /// Blocking implementation of `append`
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// The developer implementing the Remote file system should FIRST try with `create_file` followed by `on_written`
    /// If the function returns error kind() `UnsupportedFeature`, then he should call this function.
    /// By default this function uses the streams function to copy content from reader to writer
    fn append_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<()> {
        if self.is_connected() {
            trace!("Opened remote file");
            let mut stream = self.append(path, metadata)?;
            let sz = io::copy(&mut reader, &mut stream)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string()))?;
            trace!("Written {} bytes to destination", sz);
            self.on_written(stream)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Blocking implementation of `create`
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// The developer implementing the Remote file system should FIRST try with `create_file` followed by `on_written`
    /// If the function returns error kind() `UnsupportedFeature`, then he should call this function.
    /// By default this function uses the streams function to copy content from reader to writer
    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        mut reader: Box<dyn Read>,
    ) -> RemoteResult<()> {
        if self.is_connected() {
            let mut stream = self.create(path, metadata)?;
            trace!("Opened remote file");
            let sz = io::copy(&mut reader, &mut stream)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string()))?;
            trace!("Written {} bytes to destination", sz);
            self.on_written(stream)
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

    /// Blocking implementation of `open`
    /// This method SHOULD be implemented ONLY when streams are not supported by the current file transfer.
    /// (since it would work thanks to the default implementation)
    /// The developer implementing the filetransfer user should FIRST try with `send_file` followed by `on_sent`
    /// If the function returns error kind() `UnsupportedFeature`, then he should call this function.
    /// For safety reasons this function doesn't accept the `Write` trait, but the destination path.
    /// By default this function uses the streams function to copy content from reader to writer
    fn open_file<W>(&mut self, src: &Path, dest: &mut W) -> RemoteResult<()>
    where
        W: Write + Send,
    {
        if self.is_connected() {
            let mut stream = self.open(src)?;
            trace!("File opened");
            let sz = io::copy(&mut stream, dest)
                .map_err(|e| RemoteError::new_ex(RemoteErrorType::ProtocolError, e.to_string()))?;
            self.on_read(stream)?;
            trace!("Copied {} bytes to destination", sz);
            Ok(())
        } else {
            Err(RemoteError::new(RemoteErrorType::NotConnected))
        }
    }

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
            false => Err(RemoteError::new(RemoteErrorType::NotConnected)),
        }
    }

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
