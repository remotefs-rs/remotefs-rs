use std::io::Cursor;
use std::path::{Path, PathBuf};

use remotefs::RemoteFs;
use remotefs::fs::{RemoteErrorType, UnixPex, WriteOptions};
use remotefs_memory::{Inode, MemoryFs, Node, Tree, node};

pub fn mounted_file_path() -> &'static Path {
    Path::new("/tmp/mounted.txt")
}

#[cfg(unix)]
pub fn setup_driver() -> MemoryFs {
    let gid = nix::unistd::getgid().as_raw();
    let uid = nix::unistd::getuid().as_raw();

    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(uid, gid, UnixPex::from(0o755)),
    ));

    let mut fs = MemoryFs::new(tree)
        .with_get_gid(|| nix::unistd::getgid().as_raw())
        .with_get_uid(|| nix::unistd::getuid().as_raw());

    fs.connect().expect("Failed to connect to fs");

    make_file_at(&fs, mounted_file_path(), b"Hello, world!");

    fs
}

#[cfg(windows)]
pub fn setup_driver() -> MemoryFs {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755)),
    ));

    let mut fs = MemoryFs::new(tree);

    fs.connect().expect("Failed to connect to fs");

    make_file_at(&mut fs, mounted_file_path(), b"Hello, world!");

    fs
}

/// Make file on the remote fs at `path` with `content`
///
/// If the stems in the path do not exist, they will be created.
fn make_file_at(remote: &MemoryFs, path: &Path, content: &[u8]) {
    let parent_dir = path.parent().expect("Path has no parent");
    make_dir_at(remote, parent_dir);

    remote
        .write_file(
            path,
            &WriteOptions::default().size_hint(content.len() as u64),
            &mut Cursor::new(content.to_vec()),
        )
        .expect("Failed to create file");
}

/// Make directory on the remote fs at `path`
///
/// All the stems in the path will be created if they do not exist.
fn make_dir_at(remote: &MemoryFs, path: &Path) {
    let mut abs_path = Path::new("/").to_path_buf();
    for stem in path.iter() {
        abs_path.push(stem);
        if abs_path == Path::new("/") {
            continue;
        }
        #[cfg(windows)]
        let abs_path = {
            use path_slash::PathBufExt;

            PathBuf::from(abs_path.to_slash_lossy().to_string())
        };
        println!("Creating directory: {abs_path:?}");
        match remote.create_dir(&abs_path, Some(UnixPex::from(0o755))) {
            Ok(()) => {}
            Err(err) if err.kind() == RemoteErrorType::AlreadyExists => {}
            Err(err) => panic!("Failed to create directory: {err}"),
        }
    }
}
