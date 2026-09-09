//! Operation capabilities advertised by a remote file system.

use std::fmt;
use std::ops::BitOr;

/// Operations a remote file system can perform natively.
#[non_exhaustive]
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Capabilities(u32);

impl Capabilities {
    /// Returns a capability set with no enabled operations.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Opens a remote file as a stream for reading.
    pub const STREAM_READ: Self = Self(1 << 0);
    /// Creates or truncates a remote file as a stream for writing.
    pub const STREAM_WRITE: Self = Self(1 << 1);
    /// Appends to a remote file as a stream or one-shot transfer.
    pub const APPEND: Self = Self(1 << 2);
    /// Honors read offsets natively instead of emulating them by skipping.
    pub const RANGE_READ: Self = Self(1 << 3);
    /// May return seekable read streams.
    pub const SEEK_READ: Self = Self(1 << 4);
    /// May return seekable write streams.
    pub const SEEK_WRITE: Self = Self(1 << 5);
    /// Copies entries on the remote host.
    pub const COPY: Self = Self(1 << 6);
    /// Creates symbolic links on the remote host.
    pub const SYMLINK: Self = Self(1 << 7);
    /// Changes metadata on remote entries.
    pub const SET_METADATA: Self = Self(1 << 8);
    /// Uses POSIX modes for supported operations.
    pub const POSIX_MODE: Self = Self(1 << 9);
    /// Executes commands on the remote host.
    pub const EXEC: Self = Self(1 << 10);

    /// Returns whether all capabilities in `other` are enabled.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the union of two capability sets.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn names(self) -> impl Iterator<Item = CapabilityName> {
        [
            (Self::STREAM_READ, "STREAM_READ"),
            (Self::STREAM_WRITE, "STREAM_WRITE"),
            (Self::APPEND, "APPEND"),
            (Self::RANGE_READ, "RANGE_READ"),
            (Self::SEEK_READ, "SEEK_READ"),
            (Self::SEEK_WRITE, "SEEK_WRITE"),
            (Self::COPY, "COPY"),
            (Self::SYMLINK, "SYMLINK"),
            (Self::SET_METADATA, "SET_METADATA"),
            (Self::POSIX_MODE, "POSIX_MODE"),
            (Self::EXEC, "EXEC"),
        ]
        .into_iter()
        .filter_map(move |(flag, name)| self.contains(flag).then_some(CapabilityName(name)))
    }
}

struct CapabilityName(&'static str);

impl fmt::Debug for CapabilityName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl BitOr for Capabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_set().entries(self.names()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Capabilities = Capabilities::STREAM_READ
        .union(Capabilities::STREAM_WRITE)
        .union(Capabilities::APPEND)
        .union(Capabilities::RANGE_READ)
        .union(Capabilities::SEEK_READ)
        .union(Capabilities::SEEK_WRITE)
        .union(Capabilities::COPY)
        .union(Capabilities::SYMLINK)
        .union(Capabilities::SET_METADATA)
        .union(Capabilities::POSIX_MODE)
        .union(Capabilities::EXEC);

    #[test]
    fn capabilities_support_flags_and_debug_names() {
        let empty = Capabilities::empty();
        assert!(!empty.contains(Capabilities::STREAM_READ));
        let combined = Capabilities::STREAM_READ | Capabilities::COPY;
        assert!(combined.contains(Capabilities::STREAM_READ));
        assert!(combined.contains(Capabilities::COPY));
        assert!(!combined.contains(Capabilities::SYMLINK));
        assert_eq!(format!("{empty:?}"), "{}");
        assert_eq!(
            format!("{ALL:?}"),
            "{STREAM_READ, STREAM_WRITE, APPEND, RANGE_READ, SEEK_READ, SEEK_WRITE, COPY, SYMLINK, SET_METADATA, POSIX_MODE, EXEC}"
        );
    }
}
