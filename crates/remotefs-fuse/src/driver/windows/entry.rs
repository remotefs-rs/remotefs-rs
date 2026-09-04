use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};

use remotefs::File;
use widestring::{U16Str, U16String};

use super::security::SecurityDescriptor;
use super::{AltStream, PendingWriteState};

/// The per-open-handle context Dokan associates with a file object.
pub struct StatHandle {
    /// The shared [`Stat`] of the file this handle was opened against.
    pub stat: Arc<RwLock<Stat>>,
    /// The alternate data stream this handle addresses, if any.
    pub alt_stream: RwLock<Option<Arc<RwLock<AltStream>>>>,
    /// Whether the file should be deleted once this handle is closed.
    pub delete_on_close: bool,
    /// A write staged for this handle by `write_file`, not yet persisted to the remote. See
    /// [`PendingWriteState`].
    pub pending_write: Mutex<Option<PendingWriteState>>,
}

impl std::fmt::Debug for StatHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatHandle")
            .field("stat", &self.stat)
            .field("alt_stream", &self.alt_stream)
            .field("delete_on_close", &self.delete_on_close)
            .field(
                "pending_write",
                &self.pending_write.lock().ok().map(|guard| guard.is_some()),
            )
            .finish()
    }
}

/// The state remotefs-fuse tracks for a single remote file, shared by all open handles to it.
#[derive(Debug)]
pub struct Stat {
    /// The remote file this entry mirrors.
    pub file: File,
    /// The Windows security descriptor reported to Dokan.
    pub sec_desc: SecurityDescriptor,
    /// Whether the file has been marked for deletion by a prior `delete_file`/`delete_directory` call.
    pub delete_pending: bool,
    /// Whether the file should be deleted once its last open handle is closed.
    pub delete_on_close: bool,
    /// NTFS alternate data streams associated with this file, keyed by stream name.
    pub alt_streams: HashMap<EntryName, Arc<RwLock<AltStream>>>,
}

impl Stat {
    /// Create a new [`Stat`] for `file`, with no pending delete and no alternate streams.
    pub fn new(file: File, sec_desc: SecurityDescriptor) -> Self {
        Self {
            file,
            sec_desc,
            delete_pending: false,
            delete_on_close: false,
            alt_streams: HashMap::new(),
        }
    }
}

/// A borrowed, case-insensitively hashed and compared alternate-stream name.
#[derive(Debug, Eq)]
#[repr(transparent)]
pub struct EntryNameRef(U16Str);

fn u16_tolower(c: u16) -> u16 {
    if c >= 'A' as u16 && c <= 'Z' as u16 {
        c + 'a' as u16 - 'A' as u16
    } else {
        c
    }
}

impl Hash for EntryNameRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for c in self.0.as_slice() {
            state.write_u16(u16_tolower(*c));
        }
    }
}

impl PartialEq for EntryNameRef {
    fn eq(&self, other: &Self) -> bool {
        if self.0.len() != other.0.len() {
            false
        } else {
            self.0
                .as_slice()
                .iter()
                .zip(other.0.as_slice())
                .all(|(c1, c2)| u16_tolower(*c1) == u16_tolower(*c2))
        }
    }
}

impl EntryNameRef {
    /// Reinterpret a [`U16Str`] reference as an [`EntryNameRef`] reference without copying.
    pub fn new(s: &U16Str) -> &Self {
        // SAFETY: `EntryNameRef` is `#[repr(transparent)]` over `U16Str`, so a reference to one
        // is a valid reference to the other.
        unsafe { &*(s as *const U16Str as *const Self) }
    }
}

/// An owned, case-insensitively hashed and compared alternate-stream name.
#[derive(Debug, Clone)]
pub struct EntryName(pub U16String);

impl Borrow<EntryNameRef> for EntryName {
    fn borrow(&self) -> &EntryNameRef {
        EntryNameRef::new(&self.0)
    }
}

impl Hash for EntryName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Borrow::<EntryNameRef>::borrow(self).hash(state)
    }
}

impl PartialEq for EntryName {
    fn eq(&self, other: &Self) -> bool {
        Borrow::<EntryNameRef>::borrow(self).eq(other.borrow())
    }
}

impl Eq for EntryName {}
