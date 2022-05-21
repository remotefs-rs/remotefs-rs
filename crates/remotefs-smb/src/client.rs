//! # client
//!
//! Smb fs client

use pavao::{
    SmbClient, SmbCredentials, SmbDirent, SmbDirentType, SmbMode, SmbOpenOptions, SmbShareMode,
    SmbStat,
};
use remotefs::{RemoteError, RemoteErrorType, RemoteFs, RemoteResult};
use std::path::{Path, PathBuf};

pub struct SmbFs {
    client: SmbClient,
    wrkrdir: PathBuf,
}
