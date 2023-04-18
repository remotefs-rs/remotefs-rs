//! ## Path
//!
//! path utilities

/**
 * MIT License
 *
 * remotefs - Copyright (c) 2021 Christian Visintin
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */
use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
use path_slash::PathExt as _;

/// Absolutize target path if relative.
pub fn absolutize(wrkdir: &Path, target: &Path) -> PathBuf {
    match target.is_absolute() {
        true => target.to_path_buf(),
        false => {
            let mut p: PathBuf = wrkdir.to_path_buf();
            let fixed_path = resolve(target);
            p.push(fixed_path);
            p
        }
    }
}

/// Fix provided path; on Windows fixes the backslashes, converting them to slashes
/// While on POSIX does nothing
#[cfg(target_os = "windows")]
fn resolve(p: &Path) -> PathBuf {
    PathBuf::from(p.to_slash_lossy().to_string())
}

#[cfg(target_family = "unix")]
fn resolve(p: &Path) -> PathBuf {
    p.to_path_buf()
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
