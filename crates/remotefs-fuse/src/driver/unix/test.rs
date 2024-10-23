use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use pretty_assertions::{assert_eq, assert_ne};
use remotefs::fs::{Metadata, UnixPex};
use remotefs::{File, RemoteError, RemoteErrorType, RemoteFs};
use remotefs_memory::{node, Inode, MemoryFs, Node, Tree};

use crate::Driver;

fn setup_driver() -> Driver {
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

    let fs = Box::new(fs) as Box<dyn RemoteFs>;

    Driver::from(fs)
}

/// Make file on the remote fs at `path` with `content`
///
/// If the stems in the path do not exist, they will be created.
fn make_file_at(driver: &mut Driver, path: &Path, content: &[u8]) {
    let parent_dir = path.parent().expect("Path has no parent");
    make_dir_at(driver, parent_dir);

    let reader = std::io::Cursor::new(content.to_vec());
    driver
        .remote
        .create_file(
            path,
            &Metadata::default().size(content.len() as u64),
            Box::new(reader),
        )
        .expect("Failed to create file");
}

/// Make directory on the remote fs at `path`
///
/// All the stems in the path will be created if they do not exist.
fn make_dir_at(driver: &mut Driver, path: &Path) {
    let mut abs_path = Path::new("/").to_path_buf();
    for stem in path.iter() {
        abs_path.push(stem);
        println!("Creating directory: {abs_path:?}");
        match driver.remote.create_dir(&abs_path, UnixPex::from(0o755)) {
            Ok(_)
            | Err(RemoteError {
                kind: RemoteErrorType::DirectoryAlreadyExists,
                ..
            }) => {}
            Err(err) => panic!("Failed to create directory: {err}"),
        }
    }
}

#[test]
fn test_should_get_unique_inode() {
    let p = PathBuf::from("/tmp/test.txt");
    let inode_a = Driver::inode(&p);
    let inode_b = Driver::inode(&p);
    assert_eq!(inode_a, inode_b);

    let p = PathBuf::from("/dev/null");
    let inode_c = Driver::inode(&p);
    assert_ne!(inode_a, inode_c);
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
    assert_eq!(
        driver
            .database
            .get(attrs.ino)
            .expect("inode is not in database"),
        file_path
    );

    // should get the same file if querying by inode
    let (file_b, attrs_b) = driver.get_inode(attrs.ino).expect("failed to get inode");
    assert_eq!(file, file_b);
    assert_eq!(attrs, attrs_b);
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
    let child_inode = Driver::inode(&looked_up_path);
    assert_eq!(
        driver
            .database
            .get(child_inode)
            .expect("child inode is not in database"),
        looked_up_path
    );
}

#[test]
fn test_should_check_access_accessible_for_user() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    };

    assert_eq!(Driver::check_access(&file, 1000, 0, libc::F_OK), true);
}

#[test]
fn test_should_check_access_accessible_for_group() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(500),
    };

    assert_eq!(Driver::check_access(&file, 100, 500, libc::F_OK), true);
}

#[test]
fn test_should_check_access_accessible_for_root() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    };

    assert_eq!(Driver::check_access(&file, 0, 0, libc::F_OK), true);
}

#[test]
fn test_should_check_access_read_for_user() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o600)).uid(10),
    };
    let file_nok_mode = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o000)).uid(1000),
    };

    assert_eq!(Driver::check_access(&file, 1000, 0, libc::R_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 1000, 0, libc::R_OK), false);
    assert_eq!(
        Driver::check_access(&file_nok_mode, 1000, 0, libc::R_OK),
        false
    );
}

#[test]
fn test_should_check_access_read_for_group() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(500),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o640))
            .uid(1000)
            .gid(50),
    };
    let file_nok_mode = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1000)
            .gid(500),
    };

    assert_eq!(Driver::check_access(&file, 100, 500, libc::R_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 100, 500, libc::R_OK), false);
    assert_eq!(
        Driver::check_access(&file_nok_mode, 100, 500, libc::R_OK),
        false
    );
}

#[test]
fn test_should_check_access_read_for_root() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1000)
            .gid(1000),
    };

    assert_eq!(Driver::check_access(&file, 0, 0, libc::R_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 0, 0, libc::R_OK), true); // root can read any file
}

#[test]
fn test_should_check_access_write_for_user() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o644)).uid(1000),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o600)).uid(10),
    };
    let file_nok_mode = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o400)).uid(1000),
    };

    assert_eq!(Driver::check_access(&file, 1000, 0, libc::W_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 1000, 0, libc::W_OK), false);
    assert_eq!(
        Driver::check_access(&file_nok_mode, 1000, 0, libc::W_OK),
        false
    );
}

#[test]
fn test_should_check_access_write_for_group() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o664))
            .uid(1000)
            .gid(500),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o664))
            .uid(1000)
            .gid(5),
    };
    let file_nok_mode = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(500),
    };

    assert_eq!(Driver::check_access(&file, 100, 500, libc::W_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 100, 500, libc::W_OK), false);
    assert_eq!(
        Driver::check_access(&file_nok_mode, 100, 500, libc::W_OK),
        false
    );
}

#[test]
fn test_should_check_access_write_for_root() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o600))
            .uid(1000)
            .gid(1000),
    };

    assert_eq!(Driver::check_access(&file, 0, 0, libc::R_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 0, 0, libc::R_OK), true); // root can read any file
}

#[test]
fn test_should_check_access_exec_for_user() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o775)).uid(1000),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o744)).uid(10),
    };
    let file_nok_mode = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default().mode(UnixPex::from(0o600)).uid(1000),
    };

    assert_eq!(Driver::check_access(&file, 1000, 0, libc::X_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 1000, 0, libc::X_OK), false);
    assert_eq!(
        Driver::check_access(&file_nok_mode, 1000, 0, libc::X_OK),
        false
    );
}

#[test]
fn test_should_check_access_exec_for_group() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o775))
            .uid(1000)
            .gid(500),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o774))
            .uid(1000)
            .gid(5),
    };
    let file_nok_mode = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o744))
            .uid(1000)
            .gid(500),
    };

    assert_eq!(Driver::check_access(&file, 100, 500, libc::X_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 100, 500, libc::X_OK), false);
    assert_eq!(
        Driver::check_access(&file_nok_mode, 100, 500, libc::X_OK),
        false
    );
}

#[test]
fn test_should_check_access_exec_for_root() {
    let file = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o744))
            .uid(1000)
            .gid(1000),
    };
    let file_nok = File {
        path: PathBuf::from("/tmp/test.txt"),
        metadata: Metadata::default()
            .mode(UnixPex::from(0o644))
            .uid(1000)
            .gid(1000),
    };

    assert_eq!(Driver::check_access(&file, 0, 0, libc::X_OK), true);
    assert_eq!(Driver::check_access(&file_nok, 0, 0, libc::X_OK), false); // root can't execute any file
}
