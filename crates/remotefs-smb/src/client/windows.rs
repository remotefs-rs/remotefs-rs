//! # Windows client
//!
//! Windows implementation of Smb fs client

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use remotefs::fs::{File, Metadata, ReadStream, UnixPex, Welcome, WriteStream};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};

/// SMB file system client
pub struct SmbFs {
    wrkdir: PathBuf,
}
