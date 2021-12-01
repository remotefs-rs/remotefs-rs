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
use crate::fs::{
    Metadata, RemoteError, RemoteErrorType, RemoteFileSystem, RemoteResult, UnixPex, UnixPexClass,
};
use crate::{Directory, Entry, File};

use ssh2::{Channel, FileStat, OpenFlags, OpenType};
use std::io::{BufReader, BufWriter, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
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
}
