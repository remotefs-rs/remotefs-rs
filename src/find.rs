//! Explicit-root recursive search for remote filesystem entries.

use std::path::Path;

#[cfg(feature = "async")]
use crate::fs::AsyncRemoteFs;
use crate::fs::{File, RemoteFs, RemoteResult};

/// Recursively finds entries below an explicit absolute directory.
///
/// The wildcard pattern is matched against each entry name. Matching
/// directories are included and traversed depth first; symbolic links are not
/// traversed.
///
/// # Errors
///
/// Returns [`crate::fs::RemoteErrorType::InvalidPath`] for a relative start
/// directory or any error returned by the backend while listing it.
///
/// # Examples
///
/// ```
/// # use remotefs::{RemoteFs, RemoteResult};
/// # use remotefs::fs::File;
/// # fn example(fs: &dyn RemoteFs) -> RemoteResult<Vec<File>> {
/// let files = remotefs::find(fs, std::path::Path::new("/var/log"), "*.log")?;
/// # Ok(files)
/// # }
/// ```
pub fn find(fs: &dyn RemoteFs, dir: &Path, pattern: &str) -> RemoteResult<Vec<File>> {
    crate::path::ensure_absolute(dir)?;
    let filter = wildmatch::WildMatch::new(pattern);
    let mut stack = vec![fs.list_dir(dir)?.into_iter()];
    let mut found = Vec::new();
    while let Some(entries) = stack.last_mut() {
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        if filter.matches(&entry.name()) {
            found.push(entry.clone());
        }
        if entry.is_dir() {
            stack.push(fs.list_dir(entry.path())?.into_iter());
        }
    }
    Ok(found)
}

/// Asynchronously finds entries below an explicit absolute directory.
///
/// The wildcard pattern is matched against each entry name. Matching
/// directories are included and traversed depth first; symbolic links are not
/// traversed.
///
/// # Errors
///
/// Returns [`crate::fs::RemoteErrorType::InvalidPath`] for a relative start
/// directory or any error returned by the backend while listing it.
///
/// # Examples
///
/// ```
/// # use remotefs::{AsyncRemoteFs, RemoteResult};
/// # use remotefs::fs::File;
/// # async fn example(fs: &dyn AsyncRemoteFs) -> RemoteResult<Vec<File>> {
/// let files = remotefs::find_async(fs, std::path::Path::new("/var/log"), "*.log").await?;
/// # Ok(files)
/// # }
/// ```
#[cfg(feature = "async")]
pub async fn find_async(
    fs: &dyn AsyncRemoteFs,
    dir: &Path,
    pattern: &str,
) -> RemoteResult<Vec<File>> {
    crate::path::ensure_absolute(dir)?;
    let filter = wildmatch::WildMatch::new(pattern);
    let mut stack = vec![fs.list_dir(dir).await?.into_iter()];
    let mut found = Vec::new();
    while let Some(entries) = stack.last_mut() {
        let Some(entry) = entries.next() else {
            stack.pop();
            continue;
        };
        if filter.matches(&entry.name()) {
            found.push(entry.clone());
        }
        if entry.is_dir() {
            stack.push(fs.list_dir(entry.path()).await?.into_iter());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
