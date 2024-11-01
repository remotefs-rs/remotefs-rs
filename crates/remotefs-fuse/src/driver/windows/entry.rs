use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use remotefs::File;
use widestring::{U16Str, U16String};

use super::security::SecurityDescriptor;
use super::AltStream;

#[derive(Debug)]
pub struct StatHandle {
    pub stat: Arc<RwLock<Stat>>,
    pub alt_stream: RwLock<Option<Arc<RwLock<AltStream>>>>,
    pub delete_on_close: bool,
    pub mtime_delayed: Mutex<Option<SystemTime>>,
    pub atime_delayed: Mutex<Option<SystemTime>>,
    pub ctime_enabled: AtomicBool,
    pub mtime_enabled: AtomicBool,
    pub atime_enabled: AtomicBool,
}

#[derive(Debug)]
pub struct Stat {
    pub file: File,
    pub sec_desc: SecurityDescriptor,
    pub handle_count: u32,
    pub delete_pending: bool,
    pub delete_on_close: bool,
    pub alt_streams: HashMap<EntryName, Arc<RwLock<AltStream>>>,
}

impl Stat {
    pub fn new(file: File, sec_desc: SecurityDescriptor) -> Self {
        Self {
            file,
            sec_desc,
            handle_count: 0,
            delete_pending: false,
            delete_on_close: false,
            alt_streams: HashMap::new(),
        }
    }
}

#[derive(Debug, Eq)]
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
    pub fn new(s: &U16Str) -> &Self {
        unsafe { &*(s as *const _ as *const Self) }
    }
}

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
