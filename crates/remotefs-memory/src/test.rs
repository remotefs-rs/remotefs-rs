use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use pretty_assertions::assert_eq;
use remotefs::fs::{Capabilities, FileType, ReadOptions, SetMetadata, UnixPex, WriteOptions};
use remotefs::{RemoteErrorType, RemoteFs};

use super::*;

const TMP: &str = "/tmp";

fn setup_client() -> MemoryFs {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755)),
        node!(PathBuf::from(TMP), Inode::dir(0, 0, UnixPex::from(0o755)))
    ));
    let mut client = MemoryFs::new(tree);
    client.connect().expect("connect must succeed");
    client
}

fn finalize_client(mut client: MemoryFs) {
    client
        .remove_dir_all(Path::new(TMP))
        .expect("remove_dir_all must succeed");
    client.disconnect().expect("disconnect must succeed");
}

#[test]
fn should_connect_and_disconnect_once() {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755))
    ));
    let mut client = MemoryFs::new(tree);
    assert!(!client.is_connected());
    assert_eq!(
        client.disconnect().unwrap_err().kind(),
        RemoteErrorType::NotConnected
    );
    client.connect().unwrap();
    assert!(client.is_connected());
    assert_eq!(
        client.connect().unwrap_err().kind(),
        RemoteErrorType::AlreadyConnected
    );
    client.disconnect().unwrap();
    assert!(!client.is_connected());
}

#[test]
fn should_reject_relative_paths_before_checking_connection() {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755))
    ));
    let client = MemoryFs::new(tree);
    assert_eq!(
        client.stat(Path::new("relative.txt")).unwrap_err().kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        client.stat(Path::new("/absolute.txt")).unwrap_err().kind(),
        RemoteErrorType::NotConnected
    );
}

#[test]
fn should_validate_all_paths_before_checking_connection() {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755))
    ));
    let client = MemoryFs::new(tree);
    assert_eq!(
        client
            .rename(Path::new("/source"), Path::new("relative"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        client
            .copy(Path::new("/source"), Path::new("relative"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        client
            .symlink(Path::new("/link"), Path::new("relative"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
}

#[test]
fn should_reject_root_mutations() {
    let client = setup_client();

    assert_eq!(
        client.create_dir(Path::new("/"), None).unwrap_err().kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        client
            .create(Path::new("/"), &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        client
            .append(Path::new("/"), &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(
        client
            .symlink(Path::new("/"), Path::new("/"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    assert!(client.stat(Path::new("/")).unwrap().is_dir());
    assert_eq!(client.list_dir(Path::new("/")).unwrap().len(), 1);
    finalize_client(client);
}

#[test]
fn should_advertise_capabilities() {
    let client = setup_client();
    let capabilities = client.capabilities();
    for expected in [
        Capabilities::STREAM_READ,
        Capabilities::STREAM_WRITE,
        Capabilities::APPEND,
        Capabilities::RANGE_READ,
        Capabilities::SEEK_READ,
        Capabilities::SEEK_WRITE,
        Capabilities::COPY,
        Capabilities::SYMLINK,
        Capabilities::SET_METADATA,
        Capabilities::POSIX_MODE,
    ] {
        assert!(capabilities.contains(expected), "missing {expected:?}");
    }
    assert!(!capabilities.contains(Capabilities::EXEC));
    finalize_client(client);
}

#[test]
fn should_create_directory() {
    let client = setup_client();
    let dir = Path::new("/tmp/mydir");
    client.create_dir(dir, Some(UnixPex::from(0o700))).unwrap();
    let entry = client.stat(dir).unwrap();
    assert!(entry.is_dir());
    assert_eq!(entry.metadata().mode, Some(UnixPex::from(0o700)));
    client.create_dir(Path::new("/tmp/default"), None).unwrap();
    assert_eq!(
        client
            .stat(Path::new("/tmp/default"))
            .unwrap()
            .metadata()
            .mode,
        Some(UnixPex::from(0o755))
    );
    finalize_client(client);
}

#[test]
fn should_not_create_directory_cause_already_exists() {
    let client = setup_client();
    let dir = Path::new("/tmp/mydir");
    client.create_dir(dir, None).unwrap();
    assert_eq!(
        client.create_dir(dir, None).unwrap_err().kind(),
        RemoteErrorType::AlreadyExists
    );
    finalize_client(client);
}

#[test]
fn should_not_create_directory_without_parent() {
    let client = setup_client();
    assert_eq!(
        client
            .create_dir(Path::new("/tmp/missing/child"), None)
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_tell_whether_file_exists() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/mydir"), None).unwrap();
    assert!(client.exists(Path::new("/tmp/mydir")).unwrap());
    assert!(!client.exists(Path::new("/tmp/nope")).unwrap());
    finalize_client(client);
}

#[test]
fn should_list_dir() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/a"), None).unwrap();
    client.create_dir(Path::new("/tmp/b"), None).unwrap();
    let mut names: Vec<String> = client
        .list_dir(Path::new(TMP))
        .unwrap()
        .iter()
        .map(|entry| entry.name())
        .collect();
    names.sort();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    assert_eq!(
        client.list_dir(Path::new("/tmp/nope")).unwrap_err().kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_stat_file() {
    let client = setup_client();
    let dir = Path::new("/tmp/mydir");
    client.create_dir(dir, Some(UnixPex::from(0o755))).unwrap();
    let entry = client.stat(dir).unwrap();
    assert_eq!(entry.name(), "mydir");
    assert_eq!(entry.path(), dir);
    assert_eq!(entry.metadata().file_type, FileType::Directory);
    assert_eq!(
        client.stat(Path::new("/tmp/nope")).unwrap_err().kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_set_metadata_partially() {
    let client = setup_client();
    let dir = Path::new("/tmp/mydir");
    client.create_dir(dir, Some(UnixPex::from(0o755))).unwrap();
    let before = client.stat(dir).unwrap().metadata().clone();
    client
        .set_metadata(
            dir,
            &SetMetadata::default()
                .uid(1000)
                .gid(1000)
                .modified(SystemTime::UNIX_EPOCH),
        )
        .unwrap();
    let after = client.stat(dir).unwrap();
    let metadata = after.metadata();
    assert_eq!(metadata.uid, Some(1000));
    assert_eq!(metadata.gid, Some(1000));
    assert_eq!(metadata.modified, Some(SystemTime::UNIX_EPOCH));
    assert_eq!(metadata.mode, before.mode);
    assert_eq!(metadata.accessed, before.accessed);
    assert_eq!(metadata.file_type, FileType::Directory);
    assert_eq!(
        client
            .set_metadata(Path::new("/tmp/nope"), &SetMetadata::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_remove_dir() {
    let client = setup_client();
    let dir = Path::new("/tmp/mydir");
    client.create_dir(dir, None).unwrap();
    client
        .create_dir(Path::new("/tmp/mydir/child"), None)
        .unwrap();
    assert_eq!(
        client.remove_dir(dir).unwrap_err().kind(),
        RemoteErrorType::DirectoryNotEmpty
    );
    client.remove_dir(Path::new("/tmp/mydir/child")).unwrap();
    client.remove_dir(dir).unwrap();
    assert!(!client.exists(dir).unwrap());
    assert_eq!(
        client.remove_dir(dir).unwrap_err().kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_remove_dir_all_without_following_symlinks() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/keep"), None).unwrap();
    client.create_dir(Path::new("/tmp/tree"), None).unwrap();
    client
        .create_dir(Path::new("/tmp/tree/nested"), None)
        .unwrap();
    client
        .symlink(Path::new("/tmp/tree/nested/link"), Path::new("/tmp/keep"))
        .unwrap();
    client.remove_dir_all(Path::new("/tmp/tree")).unwrap();
    assert!(!client.exists(Path::new("/tmp/tree")).unwrap());
    assert!(client.exists(Path::new("/tmp/keep")).unwrap());
    assert_eq!(
        client
            .remove_dir_all(Path::new("/tmp/tree"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_make_symlink() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/target"), None).unwrap();
    let link = Path::new("/tmp/link");
    client.symlink(link, Path::new("/tmp/target")).unwrap();
    let entry = client.stat(link).unwrap();
    assert!(entry.is_symlink());
    assert_eq!(
        entry.metadata().symlink.as_deref(),
        Some(Path::new("/tmp/target"))
    );
    assert_eq!(
        client
            .symlink(link, Path::new("/tmp/target"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::AlreadyExists
    );
    assert_eq!(
        client
            .symlink(Path::new("/tmp/other"), Path::new("/tmp/nope"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    assert_eq!(
        client
            .symlink(Path::new("/tmp/other"), Path::new("target"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::InvalidPath
    );
    client.remove_file(link).unwrap();
    assert!(client.exists(Path::new("/tmp/target")).unwrap());
    finalize_client(client);
}

#[test]
fn should_not_remove_directory_as_file() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/mydir"), None).unwrap();
    assert_eq!(
        client
            .remove_file(Path::new("/tmp/mydir"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::CouldNotRemoveFile
    );
    finalize_client(client);
}

#[test]
fn should_not_exec_command() {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755))
    ));
    let disconnected = MemoryFs::new(tree);
    assert_eq!(
        disconnected.exec("echo 5").unwrap_err().kind(),
        RemoteErrorType::NotConnected
    );

    let client = setup_client();
    assert_eq!(
        client.exec("echo 5").unwrap_err().kind(),
        RemoteErrorType::UnsupportedFeature
    );
    finalize_client(client);
}

#[test]
fn should_set_gid_and_uid() {
    let client = setup_client().with_get_gid(|| 1000).with_get_uid(|| 100);
    let dir = Path::new("/tmp/test");
    client.create_dir(dir, None).unwrap();
    let entry = client.stat(dir).unwrap();
    assert_eq!(entry.metadata().gid, Some(1000));
    assert_eq!(entry.metadata().uid, Some(100));
    finalize_client(client);
}

#[test]
fn should_be_shareable_across_threads() {
    let client = Arc::new(setup_client());
    let handles: Vec<_> = (0..4)
        .map(|index| {
            let client = Arc::clone(&client);
            std::thread::spawn(move || {
                let dir = PathBuf::from(format!("/tmp/dir{index}"));
                client.create_dir(&dir, None).unwrap();
                assert!(client.exists(&dir).unwrap());
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(client.list_dir(Path::new(TMP)).unwrap().len(), 4);
}

#[test]
fn should_create_file_and_commit_on_finish() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut stream = client
        .create(path, &WriteOptions::default().mode(UnixPex::from(0o644)))
        .unwrap();
    assert!(stream.seekable());
    stream.write_all(b"test data\n").unwrap();
    stream.flush().unwrap();
    // The inode exists but no bytes are committed yet.
    assert_eq!(client.stat(path).unwrap().metadata().size, Some(0));
    stream.finish().unwrap();
    let entry = client.stat(path).unwrap();
    assert_eq!(entry.metadata().size, Some(10));
    assert_eq!(entry.metadata().mode, Some(UnixPex::from(0o644)));
    assert!(entry.metadata().modified.is_some());
    finalize_client(client);
}

#[test]
fn should_write_file_and_read_file_one_shot() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"hello world".to_vec());
    assert_eq!(
        client
            .write_file(path, &WriteOptions::default(), &mut input)
            .unwrap(),
        11
    );
    let mut output = Vec::new();
    assert_eq!(
        client
            .read_file(path, &ReadOptions::default(), &mut output)
            .unwrap(),
        11
    );
    assert_eq!(output, b"hello world");
    finalize_client(client);
}

#[test]
fn should_truncate_on_create_and_preserve_on_append() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"old".to_vec());
    client
        .write_file(path, &WriteOptions::default(), &mut input)
        .unwrap();
    let mut input = Cursor::new(b"new".to_vec());
    client
        .write_file(path, &WriteOptions::default(), &mut input)
        .unwrap();
    let mut input = Cursor::new(b"!".to_vec());
    assert_eq!(
        client
            .append_file(path, &WriteOptions::default(), &mut input)
            .unwrap(),
        1
    );
    let mut output = Vec::new();
    client
        .read_file(path, &ReadOptions::default(), &mut output)
        .unwrap();
    assert_eq!(output, b"new!");
    assert_eq!(client.stat(path).unwrap().metadata().size, Some(4));
    finalize_client(client);
}

#[test]
fn should_apply_modified_from_write_options() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"x".to_vec());
    client
        .write_file(
            path,
            &WriteOptions::default().modified(SystemTime::UNIX_EPOCH),
            &mut input,
        )
        .unwrap();
    assert_eq!(
        client.stat(path).unwrap().metadata().modified,
        Some(SystemTime::UNIX_EPOCH)
    );
    finalize_client(client);
}

#[test]
fn should_append_to_missing_file() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"abc".to_vec());
    client
        .append_file(path, &WriteOptions::default(), &mut input)
        .unwrap();
    assert_eq!(client.stat(path).unwrap().metadata().size, Some(3));
    finalize_client(client);
}

#[test]
fn should_not_create_file_without_parent_or_over_directory() {
    let client = setup_client();
    assert_eq!(
        client
            .create(Path::new("/tmp/missing/a.txt"), &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    client.create_dir(Path::new("/tmp/dir"), None).unwrap();
    assert_eq!(
        client
            .create(Path::new("/tmp/dir"), &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert_eq!(
        client
            .append(Path::new("/tmp/dir"), &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    finalize_client(client);
}

#[test]
fn should_discard_staged_bytes_when_stream_is_dropped() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"keep".to_vec());
    client
        .write_file(path, &WriteOptions::default(), &mut input)
        .unwrap();
    {
        let mut stream = client.append(path, &WriteOptions::default()).unwrap();
        stream.write_all(b" dropped").unwrap();
    }
    let mut output = Vec::new();
    client
        .read_file(path, &ReadOptions::default(), &mut output)
        .unwrap();
    assert_eq!(output, b"keep");
    finalize_client(client);
}

#[test]
fn should_preserve_append_metadata_when_stream_is_dropped() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"keep".to_vec());
    client
        .write_file(
            path,
            &WriteOptions::default().mode(UnixPex::from(0o640)),
            &mut input,
        )
        .unwrap();
    {
        let _stream = client
            .append(path, &WriteOptions::default().mode(UnixPex::from(0o600)))
            .unwrap();
    }
    assert_eq!(
        client.stat(path).unwrap().metadata().mode,
        Some(UnixPex::from(0o640))
    );
    finalize_client(client);
}

#[test]
fn should_reject_append_to_symlink() {
    let client = setup_client();
    let target = Path::new("/tmp/target");
    let link = Path::new("/tmp/link");
    client.create_dir(target, None).unwrap();
    client.symlink(link, target).unwrap();

    assert_eq!(
        client
            .append(link, &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    let entry = client.stat(link).unwrap();
    assert!(entry.is_symlink());
    assert_eq!(entry.metadata().symlink.as_deref(), Some(target));
    finalize_client(client);
}

#[test]
fn should_reject_stale_append_streams() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"base".to_vec());
    client
        .write_file(path, &WriteOptions::default(), &mut input)
        .unwrap();

    let mut first = client.append(path, &WriteOptions::default()).unwrap();
    first.write_all(b" first").unwrap();
    let mut second = client.append(path, &WriteOptions::default()).unwrap();
    second.write_all(b" second").unwrap();
    second.finish().unwrap();
    assert_eq!(
        first.finish().unwrap_err().kind(),
        RemoteErrorType::ProtocolError
    );

    let mut output = Vec::new();
    client
        .read_file(path, &ReadOptions::default(), &mut output)
        .unwrap();
    assert_eq!(output, b"base second");
    finalize_client(client);
}

#[test]
fn should_honor_read_offset_and_length() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut input = Cursor::new(b"abcdef".to_vec());
    client
        .write_file(path, &WriteOptions::default(), &mut input)
        .unwrap();

    let mut output = Vec::new();
    client
        .read_file(
            path,
            &ReadOptions::default().offset(2).length(2),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"cd");

    let mut output = Vec::new();
    client
        .read_file(
            path,
            &ReadOptions::default().offset(2).length(0),
            &mut output,
        )
        .unwrap();
    assert!(output.is_empty());

    let mut output = Vec::new();
    client
        .read_file(path, &ReadOptions::default().offset(100), &mut output)
        .unwrap();
    assert!(output.is_empty());

    let mut output = Vec::new();
    client
        .read_file(
            path,
            &ReadOptions::default().offset(4).length(100),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"ef");
    finalize_client(client);
}

#[test]
fn should_seek_read_and_write_streams() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut stream = client.create(path, &WriteOptions::default()).unwrap();
    stream.write_all(b"xxxxxx").unwrap();
    stream.seek(SeekFrom::Start(2)).unwrap();
    stream.write_all(b"yy").unwrap();
    stream.finish().unwrap();

    let mut stream = client.open(path, &ReadOptions::default()).unwrap();
    assert!(stream.seekable());
    stream.seek(SeekFrom::Start(2)).unwrap();
    let mut output = String::new();
    stream.read_to_string(&mut output).unwrap();
    stream.finish().unwrap();
    assert_eq!(output, "yyxx");
    finalize_client(client);
}

#[test]
fn should_not_open_directory_or_missing_file() {
    let client = setup_client();
    assert_eq!(
        client
            .open(Path::new(TMP), &ReadOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert_eq!(
        client
            .open(Path::new("/tmp/nope"), &ReadOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_open_symlink_content() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/target"), None).unwrap();
    client
        .symlink(Path::new("/tmp/link"), Path::new("/tmp/target"))
        .unwrap();
    let mut output = Vec::new();
    client
        .read_file(Path::new("/tmp/link"), &ReadOptions::default(), &mut output)
        .unwrap();
    assert_eq!(output, b"/tmp/target");
    finalize_client(client);
}

#[test]
fn should_fail_finish_when_file_was_removed() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut stream = client.create(path, &WriteOptions::default()).unwrap();
    stream.write_all(b"data").unwrap();
    client.remove_file(path).unwrap();
    assert_eq!(
        stream.finish().unwrap_err().kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    finalize_client(client);
}

#[test]
fn should_fail_finish_when_file_was_replaced() {
    let client = setup_client();
    let path = Path::new("/tmp/a.txt");
    let mut stream = client.create(path, &WriteOptions::default()).unwrap();
    stream.write_all(b"stale").unwrap();
    client.remove_file(path).unwrap();
    let mut replacement = Cursor::new(b"replacement".to_vec());
    client
        .write_file(path, &WriteOptions::default(), &mut replacement)
        .unwrap();
    assert_eq!(
        stream.finish().unwrap_err().kind(),
        RemoteErrorType::ProtocolError
    );
    let mut output = Vec::new();
    client
        .read_file(path, &ReadOptions::default(), &mut output)
        .unwrap();
    assert_eq!(output, b"replacement");
    finalize_client(client);
}

#[test]
fn should_rename_directory_with_children() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/src"), None).unwrap();
    client.create_dir(Path::new("/tmp/src/sub"), None).unwrap();
    let mut input = Cursor::new(b"data".to_vec());
    client
        .write_file(
            Path::new("/tmp/src/sub/file.txt"),
            &WriteOptions::default(),
            &mut input,
        )
        .unwrap();
    client
        .rename(Path::new("/tmp/src"), Path::new("/tmp/dest"))
        .unwrap();
    assert!(!client.exists(Path::new("/tmp/src")).unwrap());
    assert!(client.exists(Path::new("/tmp/dest/sub")).unwrap());
    let entry = client.stat(Path::new("/tmp/dest/sub/file.txt")).unwrap();
    assert_eq!(entry.path(), Path::new("/tmp/dest/sub/file.txt"));
    let listed: Vec<PathBuf> = client
        .list_dir(Path::new("/tmp/dest/sub"))
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    assert_eq!(listed, vec![PathBuf::from("/tmp/dest/sub/file.txt")]);
    let mut output = Vec::new();
    client
        .read_file(
            Path::new("/tmp/dest/sub/file.txt"),
            &ReadOptions::default(),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"data");
    finalize_client(client);
}

#[test]
fn should_rename_file_over_existing_destination() {
    let client = setup_client();
    let mut input = Cursor::new(b"new".to_vec());
    client
        .write_file(
            Path::new("/tmp/a.txt"),
            &WriteOptions::default(),
            &mut input,
        )
        .unwrap();
    let mut input = Cursor::new(b"old".to_vec());
    client
        .write_file(
            Path::new("/tmp/b.txt"),
            &WriteOptions::default(),
            &mut input,
        )
        .unwrap();
    client
        .rename(Path::new("/tmp/a.txt"), Path::new("/tmp/b.txt"))
        .unwrap();
    let mut output = Vec::new();
    client
        .read_file(
            Path::new("/tmp/b.txt"),
            &ReadOptions::default(),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"new");
    assert_eq!(client.list_dir(Path::new(TMP)).unwrap().len(), 1);
    finalize_client(client);
}

#[test]
fn should_not_rename_into_itself_or_without_parent() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/src"), None).unwrap();
    client
        .create_dir(Path::new("/tmp/src/child"), None)
        .unwrap();
    assert_eq!(
        client
            .rename(Path::new("/tmp/src"), Path::new("/tmp/src/inner"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert_eq!(
        client
            .rename(Path::new("/tmp/src"), Path::new("/tmp/missing/dest"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    assert_eq!(
        client
            .rename(Path::new("/tmp/nope"), Path::new("/tmp/dest"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    assert_eq!(
        client
            .rename(Path::new("/tmp/src/child"), Path::new("/tmp/src"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert!(client.exists(Path::new("/tmp/src")).unwrap());
    assert!(client.exists(Path::new("/tmp/src/child")).unwrap());
    finalize_client(client);
}

#[test]
fn should_copy_directory_with_children() {
    let client = setup_client();
    client.create_dir(Path::new("/tmp/src"), None).unwrap();
    let mut input = Cursor::new(b"data".to_vec());
    client
        .write_file(
            Path::new("/tmp/src/file.txt"),
            &WriteOptions::default(),
            &mut input,
        )
        .unwrap();
    client
        .copy(Path::new("/tmp/src"), Path::new("/tmp/dest"))
        .unwrap();
    assert!(client.exists(Path::new("/tmp/src/file.txt")).unwrap());
    assert!(client.exists(Path::new("/tmp/dest/file.txt")).unwrap());
    // The copy is independent from the source.
    let mut input = Cursor::new(b"changed".to_vec());
    client
        .write_file(
            Path::new("/tmp/dest/file.txt"),
            &WriteOptions::default(),
            &mut input,
        )
        .unwrap();
    let mut output = Vec::new();
    client
        .read_file(
            Path::new("/tmp/src/file.txt"),
            &ReadOptions::default(),
            &mut output,
        )
        .unwrap();
    assert_eq!(output, b"data");
    assert_eq!(
        client
            .copy(Path::new("/tmp/src"), Path::new("/tmp/src/copy"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert_eq!(
        client
            .copy(Path::new("/tmp/src"), Path::new(TMP))
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert!(client.exists(Path::new("/tmp/src/file.txt")).unwrap());
    finalize_client(client);
}
