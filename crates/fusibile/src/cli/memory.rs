use std::path::PathBuf;

use argh::FromArgs;
use remotefs::fs::UnixPex;
use remotefs_memory::{node, Inode, MemoryFs, Node, Tree};

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "memory")]
/// Mount a Virtual Memory filesystem
pub struct MemoryArgs {}

impl From<MemoryArgs> for MemoryFs {
    fn from(_: MemoryArgs) -> Self {
        #[cfg(unix)]
        let uid = nix::unistd::getuid().as_raw();
        #[cfg(windows)]
        let uid = 0;

        #[cfg(unix)]
        let gid = nix::unistd::getgid().as_raw();
        #[cfg(windows)]
        let gid = 0;

        let tree = Tree::new(node!(
            PathBuf::from("/"),
            Inode::dir(uid, gid, UnixPex::from(0o755)),
        ));

        MemoryFs::new(tree)
            .with_get_gid(|| {
                #[cfg(unix)]
                {
                    nix::unistd::getgid().as_raw()
                }
                #[cfg(windows)]
                {
                    0
                }
            })
            .with_get_uid(|| {
                #[cfg(unix)]
                {
                    nix::unistd::getuid().as_raw()
                }
                #[cfg(windows)]
                {
                    0
                }
            })
    }
}
