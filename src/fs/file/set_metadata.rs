//! Settable metadata fields for a remote filesystem entry.

use std::time::SystemTime;

use super::UnixPex;

/// Metadata fields that may be changed on a remote entry.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetMetadata {
    /// New POSIX permissions, when specified.
    pub mode: Option<UnixPex>,
    /// New owning user id, when specified.
    pub uid: Option<u32>,
    /// New owning group id, when specified.
    pub gid: Option<u32>,
    /// New last access time, when specified.
    pub accessed: Option<SystemTime>,
    /// New last modification time, when specified.
    pub modified: Option<SystemTime>,
}

impl SetMetadata {
    /// Sets POSIX permissions.
    pub fn mode(mut self, mode: UnixPex) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Sets the owning user id.
    pub fn uid(mut self, uid: u32) -> Self {
        self.uid = Some(uid);
        self
    }

    /// Sets the owning group id.
    pub fn gid(mut self, gid: u32) -> Self {
        self.gid = Some(gid);
        self
    }

    /// Sets the last access time.
    pub fn accessed(mut self, accessed: SystemTime) -> Self {
        self.accessed = Some(accessed);
        self
    }

    /// Sets the last modification time.
    pub fn modified(mut self, modified: SystemTime) -> Self {
        self.modified = Some(modified);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_store_optional_values() {
        let metadata = SetMetadata::default().uid(1000).gid(1000);
        assert_eq!(metadata.uid, Some(1000));
        assert_eq!(metadata.gid, Some(1000));
    }
}
