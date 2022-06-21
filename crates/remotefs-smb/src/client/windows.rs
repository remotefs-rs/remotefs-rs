//! # Windows client
//!
//! Windows implementation of Smb fs client

use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// SMB file system client
pub struct SmbFs {
    wrkdir: PathBuf,
}
