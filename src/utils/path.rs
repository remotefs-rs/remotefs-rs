//! Path helpers for resolving user-supplied paths against a working directory.

use std::path::{Path, PathBuf};

/// Resolve `target` against `wrkdir` when it is relative.
///
/// An absolute `target` is returned unchanged. This mirrors what a protocol does
/// with a path a caller passed without a leading separator, and it is what the
/// defaulted [`crate::RemoteFs`] methods use before recursing.
pub fn absolutize(wrkdir: &Path, target: &Path) -> PathBuf {
    match target.is_absolute() {
        true => target.to_path_buf(),
        false => {
            let mut p: PathBuf = wrkdir.to_path_buf();
            p.push(target);
            p
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    fn absolutize_path() {
        assert_eq!(
            absolutize(Path::new("/home/omar"), Path::new("readme.txt")).as_path(),
            Path::new("/home/omar/readme.txt")
        );
        assert_eq!(
            absolutize(Path::new("/home/omar"), Path::new("/tmp/readme.txt")).as_path(),
            Path::new("/tmp/readme.txt")
        );
    }
}
