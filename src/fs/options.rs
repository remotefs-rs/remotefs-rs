//! Options and result values shared by filesystem operations.

use std::time::SystemTime;

use super::UnixPex;

/// Controls the range returned by a remote read.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    /// Number of bytes to skip before reading, when specified.
    pub offset: Option<u64>,
    /// Maximum number of bytes to return, when specified.
    pub length: Option<u64>,
}

impl ReadOptions {
    /// Sets the byte offset, including an explicit zero.
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Sets the maximum read length, including an explicit zero.
    pub fn length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }
}

/// Controls creation and appending of a remote file.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// Size required or suggested by the protocol before writing.
    pub size_hint: Option<u64>,
    /// POSIX mode to apply when supported.
    pub mode: Option<UnixPex>,
    /// Modification time to apply when supported.
    pub modified: Option<SystemTime>,
}

impl WriteOptions {
    /// Sets the transfer size hint, including an explicit zero.
    pub fn size_hint(mut self, size_hint: u64) -> Self {
        self.size_hint = Some(size_hint);
        self
    }

    /// Sets the POSIX mode.
    pub fn mode(mut self, mode: UnixPex) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Sets the modification time.
    pub fn modified(mut self, modified: SystemTime) -> Self {
        self.modified = Some(modified);
        self
    }
}

/// Output returned by a remote shell command.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    /// Exit status returned by the remote command.
    pub exit_code: u32,
    /// Standard output emitted by the remote command.
    pub stdout: String,
}

impl ExecOutput {
    /// Creates shell output from an exit code and standard output.
    pub fn new(exit_code: u32, stdout: impl Into<String>) -> Self {
        Self {
            exit_code,
            stdout: stdout.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_preserve_explicit_zero() {
        let read = ReadOptions::default().offset(0).length(0);
        assert_eq!(read.offset, Some(0));
        assert_eq!(read.length, Some(0));
        assert_eq!(WriteOptions::default().size_hint, None);
        assert_eq!(WriteOptions::default().size_hint(0).size_hint, Some(0));
        assert_eq!(crate::fs::Metadata::default().size, None);
        assert_eq!(crate::fs::Metadata::default().size(0).size, Some(0));
    }
}
