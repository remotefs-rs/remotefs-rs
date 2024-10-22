use remotefs_fuse::Driver;

use std::path::PathBuf;

use remotefs::fs::UnixPex;
use remotefs::RemoteFs;
use remotefs_memory::{node, Inode, MemoryFs, Node, Tree};

pub fn setup_driver() -> Driver {
    let tree = Tree::new(node!(
        PathBuf::from("/"),
        Inode::dir(0, 0, UnixPex::from(0o755)),
    ));

    let fs = MemoryFs::new(tree);

    let fs = Box::new(fs) as Box<dyn RemoteFs>;

    Driver::from(fs)
}
