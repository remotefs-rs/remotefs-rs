use std::path::{Path, PathBuf};

use pretty_assertions::{assert_eq, assert_ne};
use remotefs::fs::{FileType, Metadata, UnixPex};
use remotefs::File;
use remotefs_memory::MemoryFs;
use widestring::U16CString;

use super::Driver;
use crate::driver::windows::ROOT_ID;

#[test]
fn test_should_get_file_index() {
    let index = Driver::<MemoryFs>::file_index(&File {
        path: PathBuf::from("C:\\Users\\user\\Desktop\\file.txt"),
        metadata: Default::default(),
    });
    assert_ne!(index, ROOT_ID);

    let index = Driver::<MemoryFs>::file_index(&File {
        path: PathBuf::from("/"),
        metadata: Default::default(),
    });

    assert_eq!(index, ROOT_ID);
}

#[test]
fn test_should_get_filename() {
    let filename = Driver::<MemoryFs>::file_name(Path::new("C:\\Users\\user\\Desktop\\file.txt"));
    let expected = U16CString::from_str("file.txt").unwrap().to_ucstring();
    assert_eq!(filename, expected);
}

#[test]
fn test_should_make_attributes_from_file() {
    let file = File {
        path: PathBuf::from("C:\\Users\\user\\Desktop\\file.txt"),
        metadata: Metadata::default().file_type(FileType::File),
    };

    let attributes = Driver::<MemoryFs>::attributes_from_file(&file);
    assert_eq!(attributes & winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY, 0);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_NORMAL,
        winapi::um::winnt::FILE_ATTRIBUTE_NORMAL
    );
    assert_eq!(attributes & winapi::um::winnt::FILE_ATTRIBUTE_READONLY, 0);

    let file = File {
        path: PathBuf::from("C:\\Users\\user\\Desktop"),
        metadata: Metadata::default().file_type(FileType::Directory),
    };

    let attributes = Driver::<MemoryFs>::attributes_from_file(&file);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY,
        winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY
    );

    let file = File {
        path: PathBuf::from("C:\\Users\\user\\Desktop"),
        metadata: Metadata::default()
            .file_type(FileType::File)
            .mode(UnixPex::from(0o444)),
    };

    let attributes = Driver::<MemoryFs>::attributes_from_file(&file);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_READONLY,
        winapi::um::winnt::FILE_ATTRIBUTE_READONLY
    );

    let file = File {
        path: PathBuf::from("C:\\Users\\user\\Desktop\\.gitignore"),
        metadata: Metadata::default().file_type(FileType::File),
    };

    let attributes = Driver::<MemoryFs>::attributes_from_file(&file);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_HIDDEN,
        winapi::um::winnt::FILE_ATTRIBUTE_HIDDEN
    );
}

#[test]
fn test_should_get_path_info() {
    let p = U16CString::from_str("/dev/null").unwrap();

    let path_info = Driver::<MemoryFs>::path_info(&p);

    assert_eq!(path_info.path, PathBuf::from("/dev/null"));
    assert_eq!(
        path_info.file_name,
        U16CString::from_str("/dev/null").unwrap().to_ucstring()
    );
    assert_eq!(path_info.parent, PathBuf::from("/dev"));
}
