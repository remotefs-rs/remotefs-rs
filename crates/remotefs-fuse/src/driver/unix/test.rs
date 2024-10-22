use std::path::PathBuf;

use remotefs::fs::UnixPex;
use remotefs::RemoteFs;
use remotefs_memory::{node, Inode, MemoryFs, Node, Tree};

use crate::Driver;

fn setup_driver() -> Driver {
    let gid = nix::unistd::getgid().as_raw();
    let uid = nix::unistd::getuid().as_raw();

    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(uid, gid, UnixPex::from(0o755)),
    ));

    let fs = MemoryFs::new(tree)
        .with_get_gid(|| nix::unistd::getgid().as_raw())
        .with_get_uid(|| nix::unistd::getuid().as_raw());

    let fs = Box::new(fs) as Box<dyn RemoteFs>;

    Driver::from(fs)
}
