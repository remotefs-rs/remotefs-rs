use std::ffi::OsStr;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use nix::unistd::AccessFlags;
use pretty_assertions::{assert_eq, assert_ne};
use remotefs::fs::{Metadata, ReadOptions, RemoteErrorType, UnixPex, WriteOptions};
use remotefs::{File, RemoteFs};
use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};

use super::Driver;
use crate::MountOption;

fn setup_driver() -> Driver<MemoryFs> {
    let gid = nix::unistd::getgid().as_raw();
    let uid = nix::unistd::getuid().as_raw();

    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(uid, gid, UnixPex::from(0o755)),
    ));

    let mut fs = MemoryFs::new(tree)
        .with_get_gid(|| nix::unistd::getgid().as_raw())
        .with_get_uid(|| nix::unistd::getuid().as_raw());

    fs.connect().expect("Failed to connect");
    assert!(fs.is_connected());

    Driver::new(
        fs,
        vec![
            MountOption::AllowRoot,
            MountOption::RW,
            MountOption::Exec,
            MountOption::Sync,
        ],
    )
}

fn setup_driver_with_mode(mode: u32) -> Driver<MemoryFs> {
    let gid = nix::unistd::getgid().as_raw();
    let uid = nix::unistd::getuid().as_raw();

    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(uid, gid, UnixPex::from(0o755)),
    ));

    let mut fs = MemoryFs::new(tree)
        .with_get_gid(|| nix::unistd::getgid().as_raw())
        .with_get_uid(|| nix::unistd::getuid().as_raw());

    fs.connect().expect("Failed to connect");
    assert!(fs.is_connected());

    Driver::new(
        fs,
        vec![
            MountOption::AllowRoot,
            MountOption::RW,
            MountOption::Exec,
            MountOption::Sync,
            MountOption::DefaultMode(mode),
        ],
    )
}

fn setup_driver_with_uid(uid: u32, gid: u32) -> Driver<MemoryFs> {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(uid, gid, UnixPex::from(0o755)),
    ));

    let mut fs = MemoryFs::new(tree)
        .with_get_gid(move || uid)
        .with_get_uid(move || gid);

    fs.connect().expect("Failed to connect");
    assert!(fs.is_connected());

    Driver::new(
        fs,
        vec![
            MountOption::AllowRoot,
            MountOption::RW,
            MountOption::Exec,
            MountOption::Sync,
            MountOption::Uid(uid),
            MountOption::Gid(gid),
        ],
    )
}

/// Make file on the remote fs at `path` with `content`
///
/// If the stems in the path do not exist, they will be created.
fn make_file_at(driver: &mut Driver<MemoryFs>, path: &Path, content: &[u8]) {
    let parent_dir = path.parent().expect("Path has no parent");
    make_dir_at(driver, parent_dir);

    driver.with_inner(|inner| {
        inner
            .remote
            .write_file(
                path,
                &WriteOptions::default().size_hint(content.len() as u64),
                &mut Cursor::new(content.to_vec()),
            )
            .expect("Failed to create file");
    });
}

/// Make directory on the remote fs at `path`
///
/// All the stems in the path will be created if they do not exist.
fn make_dir_at(driver: &mut Driver<MemoryFs>, path: &Path) {
    let mut abs_path = Path::new("/").to_path_buf();
    for stem in path.iter() {
        abs_path.push(stem);
        if abs_path == Path::new("/") {
            continue;
        }
        println!("Creating directory: {abs_path:?}");
        driver.with_inner(|inner| {
            match inner
                .remote
                .create_dir(&abs_path, Some(UnixPex::from(0o755)))
            {
                Ok(()) => {}
                Err(err) if err.kind() == RemoteErrorType::AlreadyExists => {}
                Err(err) => panic!("Failed to create directory: {err}"),
            }
        });
    }
}

#[test]
fn test_should_get_configured_gid() {
    let driver = setup_driver_with_uid(1001, 1002);
    assert_eq!(driver.gid(), Some(1002));
    assert_eq!(driver.uid(), Some(1001));
}

#[test]
fn test_should_get_unique_inode() {
    let driver = setup_driver();

    let p = PathBuf::from("/tmp/test.txt");
    let inode_a = driver.inode_for(&p);
    let inode_b = driver.inode_for(&p);
    assert_eq!(inode_a, inode_b);

    let p = PathBuf::from("/dev/null");
    let inode_c = driver.inode_for(&p);
    assert_ne!(inode_a, inode_c);
}

#[test]
fn test_should_write_multiple_chunks_to_the_same_handle_without_truncating() {
    let mut driver = setup_driver();
    let file_path = Path::new("/tmp/test.txt");
    make_file_at(&mut driver, file_path, b"");

    let (file, _) = driver
        .get_inode_from_path(file_path)
        .expect("failed to get inode");

    // simulate the kernel splitting one large write into several write() calls on the same
    // file handle, as it does for any file bigger than a single write buffer
    let pid = 1;
    let fh = 42;
    driver
        .write_to_handle(pid, fh, &file, b"hello ", 0)
        .expect("failed to write first chunk");
    driver
        .write_to_handle(pid, fh, &file, b"world", 6)
        .expect("failed to write second chunk");
    driver
        .finalize_write(pid, fh)
        .expect("failed to finalize write");

    let mut buffer = [0u8; 11];
    driver
        .read_remote_file(file_path, &mut buffer, 0)
        .expect("failed to read file back");
    assert_eq!(&buffer, b"hello world");
}

#[test]
fn test_should_write_at_offset_leaving_a_gap() {
    let mut driver = setup_driver();
    let file_path = Path::new("/tmp/test.txt");
    make_file_at(&mut driver, file_path, b"");

    let (file, _) = driver
        .get_inode_from_path(file_path)
        .expect("failed to get inode");

    let pid = 1;
    let fh = 43;
    // non-sequential write: leaves a zero-filled gap before the offset
    driver
        .write_to_handle(pid, fh, &file, b"world", 6)
        .expect("failed to write chunk");
    driver
        .finalize_write(pid, fh)
        .expect("failed to finalize write");

    let mut buffer = [0u8; 11];
    driver
        .read_remote_file(file_path, &mut buffer, 0)
        .expect("failed to read file back");
    assert_eq!(&buffer[6..], b"world");
}

#[test]
fn test_should_get_inode_from_path() {
    let mut driver = setup_driver();
    // make file
    let file_path = Path::new("/tmp/test.txt");
    make_file_at(&mut driver, file_path, b"hello world");

    // get inode from path
    let (file, attrs) = driver
        .get_inode_from_path(file_path)
        .expect("failed to get inode");
    assert_eq!(file.path(), file_path);
    assert_eq!(attrs.size, 11);

    // file should be in the database
    driver.with_inner(|inner| {
        assert_eq!(
            inner
                .state
                .database
                .get(attrs.ino.0)
                .expect("inode is not in database"),
            file_path
        );
    });

    // should get the same file if querying by inode
    let (file_b, attrs_b) = driver.get_inode(attrs.ino).expect("failed to get inode");
    assert_eq!(file, file_b);
    assert_eq!(attrs, attrs_b);
}

#[test]
fn test_should_read_file_at_offset() {
    let mut driver = setup_driver();
    let file_path = Path::new("/tmp/test.txt");
    make_file_at(&mut driver, file_path, b"hello world");

    let mut buffer = [0u8; 5];
    let read = driver
        .read_remote_file(file_path, &mut buffer, 6)
        .expect("failed to read file at offset");

    assert_eq!(read, 5);
    assert_eq!(&buffer, b"world");
}

#[test]
fn test_should_read_nothing_past_end_of_file() {
    let mut driver = setup_driver();
    let file_path = Path::new("/tmp/test.txt");
    make_file_at(&mut driver, file_path, b"hello world");

    let mut buffer = [0u8; 5];
    // offset is far beyond the 11-byte file; this must yield zero bytes, not
    // attempt to allocate or read a huge amount of data, and not fail
    let read = driver
        .read_remote_file(file_path, &mut buffer, 1_000_000)
        .expect("reading past EOF must not fail");

    assert_eq!(read, 0);
}

#[test]
fn test_should_truncate_file_via_transfer_helper() {
    let mut driver = setup_driver();
    let file_path = Path::new("/tmp/test.txt");
    make_file_at(&mut driver, file_path, b"hello world");

    let (file, _) = driver
        .get_inode_from_path(file_path)
        .expect("failed to get inode");
    driver
        .with_inner(|inner| crate::driver::transfer::truncate_file(&inner.remote, &file, 5))
        .expect("failed to truncate");

    driver.with_inner(|inner| {
        let mut out = Cursor::new(Vec::new());
        inner
            .remote
            .read_file(file_path, &ReadOptions::default(), &mut out)
            .expect("failed to read file");
        assert_eq!(out.into_inner(), b"hello");
    });
}

#[test]
fn test_should_lookup_name() {
    let mut driver = setup_driver();
    // make dir
    let parent_dir = Path::new("/home/user/.config");
    make_dir_at(&mut driver, parent_dir);
    // create inode for it
    let inode = driver
        .get_inode_from_path(parent_dir)
        .expect("failed to get inode")
        .1
        .ino;

    // lookup name
    let looked_up_path = driver
        .lookup_name(inode, OsStr::new("test.txt"))
        .expect("failed to lookup name");

    let expected_file_path = Path::new("/home/user/.config/test.txt");
    assert_eq!(looked_up_path, expected_file_path);

    // inode for looked up file should be in the database
    let child_inode = driver.inode_for(&looked_up_path);
    driver.with_inner(|inner| {
        assert_eq!(
            inner
                .state
                .database
                .get(child_inode)
                .expect("child inode is not in database"),
            looked_up_path
        );
    });
}

#[test]
fn test_should_check_access_accessible_for_user() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    );

    assert_eq!(driver.check_access(&file, 1000, 0, AccessFlags::F_OK), true);
}

#[test]
fn test_should_check_access_accessible_for_group() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(500),
    );

    assert_eq!(
        driver.check_access(&file, 100, 500, AccessFlags::F_OK),
        true
    );
}

#[test]
fn test_should_check_access_accessible_for_root() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    );

    assert_eq!(driver.check_access(&file, 0, 0, AccessFlags::F_OK), true);
}

#[test]
fn test_should_check_access_read_for_user() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o600)).uid(10),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o000)).uid(1000),
    );

    assert_eq!(driver.check_access(&file, 1000, 0, AccessFlags::R_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 1000, 0, AccessFlags::R_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 1000, 0, AccessFlags::R_OK),
        false
    );
}

#[test]
fn test_should_check_access_read_for_group() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(500),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o640))
            .uid(1000)
            .gid(50),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1000)
            .gid(500),
    );

    assert_eq!(
        driver.check_access(&file, 100, 500, AccessFlags::R_OK),
        true
    );
    assert_eq!(
        driver.check_access(&file_nok, 100, 500, AccessFlags::R_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 100, 500, AccessFlags::R_OK),
        false
    );
}

#[test]
fn test_should_check_access_read_for_root() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1000)
            .gid(1000),
    );

    assert_eq!(driver.check_access(&file, 0, 0, AccessFlags::R_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 0, 0, AccessFlags::R_OK),
        true
    ); // root can read any file
}

#[test]
fn test_should_check_access_write_for_user() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o600)).uid(10),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o400)).uid(1000),
    );

    assert_eq!(driver.check_access(&file, 1000, 0, AccessFlags::W_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 1000, 0, AccessFlags::W_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 1000, 0, AccessFlags::W_OK),
        false
    );
}

#[test]
fn test_should_check_access_write_for_group() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o664))
            .uid(1000)
            .gid(500),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o664))
            .uid(1000)
            .gid(5),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(500),
    );

    assert_eq!(
        driver.check_access(&file, 100, 500, AccessFlags::W_OK),
        true
    );
    assert_eq!(
        driver.check_access(&file_nok, 100, 500, AccessFlags::W_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 100, 500, AccessFlags::W_OK),
        false
    );
}

#[test]
fn test_should_check_access_write_for_root() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1000)
            .gid(1000),
    );

    assert_eq!(driver.check_access(&file, 0, 0, AccessFlags::R_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 0, 0, AccessFlags::R_OK),
        true
    ); // root can read any file
}

#[test]
fn test_should_check_access_exec_for_user() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o775)).uid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o744)).uid(10),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o600)).uid(1000),
    );

    assert_eq!(driver.check_access(&file, 1000, 0, AccessFlags::X_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 1000, 0, AccessFlags::X_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 1000, 0, AccessFlags::X_OK),
        false
    );
}

#[test]
fn test_should_check_access_exec_for_group() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o775))
            .uid(1000)
            .gid(500),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o774))
            .uid(1000)
            .gid(5),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o744))
            .uid(1000)
            .gid(500),
    );

    assert_eq!(
        driver.check_access(&file, 100, 500, AccessFlags::X_OK),
        true
    );
    assert_eq!(
        driver.check_access(&file_nok, 100, 500, AccessFlags::X_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 100, 500, AccessFlags::X_OK),
        false
    );
}

#[test]
fn test_should_check_access_exec_for_root() {
    let driver = setup_driver();
    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o744))
            .uid(1000)
            .gid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    );

    assert_eq!(driver.check_access(&file, 0, 0, AccessFlags::X_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 0, 0, AccessFlags::X_OK),
        false
    ); // root can't execute any file
}

#[test]
fn test_should_check_access_write_for_configured_uid() {
    let driver = setup_driver_with_uid(5, 1);

    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o400)).uid(10),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o400)).uid(0),
    );

    assert_eq!(driver.check_access(&file, 5, 0, AccessFlags::W_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 5, 0, AccessFlags::W_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 5, 0, AccessFlags::W_OK),
        false
    );
}

#[test]
fn test_should_check_access_write_for_configured_gid() {
    let driver = setup_driver_with_uid(1000, 1);

    let file = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o664))
            .uid(1000)
            .gid(1),
    );
    let file_nok = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(10)
            .gid(1),
    );
    let file_nok_mode = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().mode(UnixPex::from(0o400)).uid(0).gid(1),
    );

    assert_eq!(driver.check_access(&file, 5, 1, AccessFlags::W_OK), true);
    assert_eq!(
        driver.check_access(&file_nok, 5, 1, AccessFlags::W_OK),
        false
    );
    assert_eq!(
        driver.check_access(&file_nok_mode, 5, 1, AccessFlags::W_OK),
        false
    );
}

#[test]
fn test_should_check_access_write_for_configured_mode() {
    let driver = setup_driver_with_mode(0o777);

    let file = File::new(PathBuf::from("/tmp/test.txt"), Metadata::default());

    let file_w_uid = File::new(PathBuf::from("/tmp/test.txt"), Metadata::default().uid(10));

    let file_w_gid = File::new(
        PathBuf::from("/tmp/test.txt"),
        Metadata::default().uid(10).gid(100),
    );

    assert_eq!(driver.check_access(&file, 1000, 0, AccessFlags::W_OK), true);
    assert_eq!(
        driver.check_access(&file_w_uid, 1000, 0, AccessFlags::W_OK),
        true
    );
    assert_eq!(
        driver.check_access(&file_w_gid, 1000, 10000, AccessFlags::W_OK),
        true
    );
}

#[test]
fn test_parse_open_flags() {
    use crate::driver::unix::state::{parse_create_flags, parse_open_flags, parse_opendir_flags};

    let rd = parse_open_flags(libc::O_RDONLY).expect("rdonly");
    assert!(rd.read && !rd.write && rd.access_mask == AccessFlags::R_OK);
    let wr = parse_open_flags(libc::O_WRONLY).expect("wronly");
    assert!(!wr.read && wr.write && wr.access_mask == AccessFlags::W_OK);
    assert_eq!(
        parse_open_flags(libc::O_RDONLY | libc::O_TRUNC).unwrap_err(),
        fuser::Errno::EACCES
    );
    assert_eq!(
        parse_open_flags(libc::O_RDONLY | 0x20)
            .expect("exec")
            .access_mask,
        AccessFlags::X_OK
    );
    assert_eq!(
        parse_opendir_flags(libc::O_RDWR).expect("rdwr").access_mask,
        AccessFlags::R_OK | AccessFlags::W_OK
    );
    assert_eq!(
        parse_create_flags(libc::O_RDWR).expect("rdwr"),
        (true, true)
    );
    assert_eq!(parse_create_flags(3).unwrap_err(), fuser::Errno::EINVAL);
}

#[test]
fn test_setattr_changes_is_none_without_changes() {
    use crate::driver::unix::state::setattr_changes;

    assert!(setattr_changes(None, None, None, None, None).is_none());
    let changes = setattr_changes(Some(0o600), Some(1), None, None, None).expect("changes");
    assert_eq!(changes.mode, Some(UnixPex::from(0o600)));
    assert_eq!(changes.uid, Some(1));
}
