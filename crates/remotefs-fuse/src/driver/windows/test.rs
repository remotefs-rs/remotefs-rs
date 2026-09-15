use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use dokan_sys::win32::{FILE_NON_DIRECTORY_FILE, FILE_OPEN};
#[cfg(feature = "tokio")]
use futures_util::io::Cursor;
use pretty_assertions::{assert_eq, assert_ne};
#[cfg(feature = "tokio")]
use remotefs::AsyncRemoteFs;
use remotefs::File;
#[cfg(feature = "tokio")]
use remotefs::adapters::r#async::Unblock;
use remotefs::fs::{FileType, Metadata, UnixPex};
#[cfg(feature = "tokio")]
use remotefs_memory::{Inode, MemoryFs, Tree, node};
#[cfg(feature = "tokio")]
use tokio::sync::RwLock as AsyncRwLock;
use widestring::U16CString;

#[cfg(feature = "tokio")]
use super::r#async::{AsyncDriver, AsyncStatHandle};
use super::common;
use super::common::CreatePlan;
use super::entry::Stat;
use super::security::SecurityDescriptor;
use crate::driver::windows::ROOT_ID;

#[test]
fn test_should_get_file_index() {
    let index = common::file_index(&File::new(
        PathBuf::from("C:\\Users\\user\\Desktop\\file.txt"),
        Default::default(),
    ));
    assert_ne!(index, ROOT_ID);

    let index = common::file_index(&File::new(PathBuf::from("/"), Default::default()));

    assert_eq!(index, ROOT_ID);
}

#[test]
fn test_should_get_filename() {
    let filename = common::file_name(Path::new("C:\\Users\\user\\Desktop\\file.txt"));
    let expected = U16CString::from_str("file.txt").unwrap().to_ucstring();
    assert_eq!(filename, expected);
}

#[test]
fn test_should_get_empty_filename_for_root() {
    let filename = common::file_name(Path::new("/"));

    assert_eq!(filename, U16CString::default().to_ucstring());
}

#[test]
fn test_should_make_attributes_from_file() {
    let file = File::new(
        PathBuf::from("C:\\Users\\user\\Desktop\\file.txt"),
        Metadata::default().file_type(FileType::File),
    );

    let attributes = common::attributes_from_file(&file);
    assert_eq!(attributes & winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY, 0);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_NORMAL,
        winapi::um::winnt::FILE_ATTRIBUTE_NORMAL
    );
    assert_eq!(attributes & winapi::um::winnt::FILE_ATTRIBUTE_READONLY, 0);

    let file = File::new(
        PathBuf::from("C:\\Users\\user\\Desktop"),
        Metadata::default().file_type(FileType::Directory),
    );

    let attributes = common::attributes_from_file(&file);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY,
        winapi::um::winnt::FILE_ATTRIBUTE_DIRECTORY
    );

    let file = File::new(
        PathBuf::from("C:\\Users\\user\\Desktop"),
        Metadata::default()
            .file_type(FileType::File)
            .mode(UnixPex::from(0o444)),
    );

    let attributes = common::attributes_from_file(&file);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_READONLY,
        winapi::um::winnt::FILE_ATTRIBUTE_READONLY
    );

    let file = File::new(
        PathBuf::from("C:\\Users\\user\\Desktop\\.gitignore"),
        Metadata::default().file_type(FileType::File),
    );

    let attributes = common::attributes_from_file(&file);
    assert_eq!(
        attributes & winapi::um::winnt::FILE_ATTRIBUTE_HIDDEN,
        winapi::um::winnt::FILE_ATTRIBUTE_HIDDEN
    );
}

#[test]
fn test_should_get_path_info() {
    let p = U16CString::from_str("/dev/null").unwrap();

    let path_info = common::path_info(&p);

    assert_eq!(path_info.path, PathBuf::from("/dev/null"));
    assert_eq!(
        path_info.file_name,
        U16CString::from_str("/dev/null").unwrap().to_ucstring()
    );
    assert_eq!(path_info.parent, PathBuf::from("/dev"));
}

#[test]
fn test_should_plan_missing_file_and_directory_creation() {
    let file_name = U16CString::from_str("/new").unwrap();
    assert!(matches!(
        common::plan_create(None, &file_name, 0, FILE_OPEN, FILE_NON_DIRECTORY_FILE),
        Err(winapi::shared::ntstatus::STATUS_OBJECT_NAME_NOT_FOUND)
    ));
    assert!(matches!(
        common::plan_create(
            None,
            &file_name,
            0,
            dokan_sys::win32::FILE_OPEN_IF,
            FILE_NON_DIRECTORY_FILE
        ),
        Ok(CreatePlan::CreateFile)
    ));
    assert!(matches!(
        common::plan_create(None, &file_name, 0, dokan_sys::win32::FILE_OPEN_IF, 0),
        Ok(CreatePlan::CreateDirectory)
    ));
}

#[test]
fn test_should_reject_create_for_existing_file() {
    let file = File::new(
        PathBuf::from("/existing"),
        Metadata::default().file_type(FileType::File),
    );
    let stat = Arc::new(RwLock::new(Stat::new(
        file,
        SecurityDescriptor::new_default().unwrap(),
    )));
    let file_name = U16CString::from_str("/existing").unwrap();
    assert!(matches!(
        common::plan_create(Some(&stat), &file_name, 0, dokan_sys::win32::FILE_CREATE, 0),
        Err(winapi::shared::ntstatus::STATUS_OBJECT_NAME_COLLISION)
    ));
}

#[test]
fn test_should_reject_negative_file_offsets() {
    assert_eq!(
        common::nonnegative_offset(-1),
        Err(winapi::shared::ntstatus::STATUS_INVALID_PARAMETER)
    );
    assert_eq!(common::nonnegative_offset(42), Ok(42));
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_should_finalize_pending_write_before_delete() {
    let mut remote = Unblock::new(MemoryFs::new(Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755)),
    ))));
    remote.connect().await.expect("connect");
    let remote = Arc::new(AsyncRwLock::new(remote));
    let driver = AsyncDriver::new(
        Arc::clone(&remote),
        vec![MountOption::RW],
        tokio::runtime::Handle::current(),
    );
    let path = Path::new("/file");
    remote
        .read()
        .await
        .write_file(
            path,
            &remotefs::fs::WriteOptions::default().size_hint(0),
            &mut Cursor::new(Vec::new()),
        )
        .await
        .expect("create file");
    let file = remote.read().await.stat(path).await.expect("stat");
    let context = AsyncStatHandle {
        stat: Arc::new(RwLock::new(Stat::new(
            file.clone(),
            SecurityDescriptor::new_default().expect("security descriptor"),
        ))),
        alt_stream: RwLock::new(None),
        delete_on_close: true,
        pending_write: tokio::sync::Mutex::new(None),
    };

    driver
        .write_to_handle(&context, &file, b"pending", 0)
        .await
        .expect("write");
    driver.cleanup_pending_write(&context, &file, true).await;

    let mut data = Cursor::new(Vec::new());
    remote
        .read()
        .await
        .read_file(path, &Default::default(), &mut data)
        .await
        .expect("read");
    assert_eq!(data.into_inner(), b"pending");
}
