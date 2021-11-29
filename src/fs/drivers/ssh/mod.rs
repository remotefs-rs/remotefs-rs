//! ## SSH
//!
//! implements the file transfer for SSH based protocols: SFTP and SCP

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
// -- ext
use std::path::Path;
// -- modules
// mod scp;
// mod sftp;
// -- export
// pub use scp::ScpFileTransfer;
// pub use sftp::SftpFileTransfer;

// -- Ssh key storage

/// This trait must be implemented in order to use ssh keys for authentication for sftp/scp.
/// You must provide the SFTP/SCP file transfer with a struct implementing this trait.
/// If you can't/dont' want to support ssh key storage, just implement a struct which always returns `None`.
pub trait SshKeyStorage {
    /// Return RSA key path from host and username
    fn resolve(&self, host: &str, username: &str) -> Option<&Path>;
}
