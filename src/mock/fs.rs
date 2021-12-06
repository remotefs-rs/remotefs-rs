//! ## Fs mock
//!
//! Contains mock for file system

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
use crate::fs::{File, Metadata};
// ext
use std::fs::File as StdFile;
#[cfg(any(feature = "with-containers", feature = "with-s3-ci"))]
use std::fs::OpenOptions;
#[cfg(any(feature = "with-containers", feature = "with-s3-ci"))]
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub fn create_sample_file_entry() -> (File, NamedTempFile) {
    // Write
    let tmpfile = create_sample_file();
    (
        File {
            name: tmpfile
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            abs_path: tmpfile.path().to_path_buf(),
            extension: None,
            metadata: Metadata::default(),
        },
        tmpfile,
    )
}

pub fn create_sample_file() -> NamedTempFile {
    // Write
    let mut tmpfile: tempfile::NamedTempFile = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        tmpfile,
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit.Mauris ultricies consequat eros,nec scelerisque magna imperdiet metus."
    )
    .unwrap();
    tmpfile
}

/// ### make_file_at
///
/// Make a file with `name` at specified path
pub fn make_file_at(dir: &Path, filename: &str) {
    let mut p: PathBuf = PathBuf::from(dir);
    p.push(filename);
    let mut file = StdFile::create(p.as_path()).expect("Failed to create file");
    writeln!(
        file,
        "Lorem ipsum dolor sit amet, consectetur adipiscing elit.Mauris ultricies consequat eros,nec scelerisque magna imperdiet metus."
    ).expect("Failed to write file");
}

/// ### make_dir_at
///
/// Make a directory in `dir`
pub fn make_dir_at(dir: &Path, dirname: &str) {
    let mut p: PathBuf = PathBuf::from(dir);
    p.push(dirname);
    std::fs::create_dir(p.as_path()).expect("Failed to create directory")
}

#[cfg(any(feature = "with-containers", feature = "with-s3-ci"))]
pub fn write_file(file: &NamedTempFile, writable: &mut impl Write) {
    let mut fhnd = OpenOptions::new()
        .create(false)
        .read(true)
        .write(false)
        .open(file.path())
        .ok()
        .unwrap();
    // Read file
    let mut buffer: [u8; 65536] = [0; 65536];
    assert!(fhnd.read(&mut buffer).is_ok());
    // Write file
    assert!(writable.write(&buffer).is_ok());
}
