//! # fusibile
//!
//! A CLI to mount remote file systems locally via FUSE (Unix) or Dokany (Windows), backed by
//! [`remotefs-fuse`](https://docs.rs/remotefs-fuse).
//!
//! ## Feature flags
//!
//! Each flag enables the corresponding remote backend as a mountable subcommand. Use
//! `--no-default-features --features <subset>` to build with only the backends you need.
//!
//! | name      | description                        | default |
//! |-----------|-------------------------------------|---------|
//! | `aws-s3`  | Mount an AWS S3 bucket.             | ✔       |
//! | `ftp`     | Mount an FTP/FTPS server.           | ✔       |
//! | `gcs`     | Mount a Google Cloud Storage bucket. | ✔      |
//! | `kube`    | Mount a Kubernetes pod filesystem.  | ✔       |
//! | `smb`     | Mount an SMB share.                 | ✔       |
//! | `ssh`     | Mount an SCP or SFTP server.        | ✔       |
//! | `webdav`  | Mount a WebDAV server.              | ✔       |

mod cli;
mod remotefs_wrapper;

use clap::Parser;
use remotefs_fuse::Mount;

fn main() -> anyhow::Result<()> {
    let args = cli::CliArgs::parse();
    args.init_logger();
    #[cfg(unix)]
    let volume = args.volume.clone();
    let mount_path = args.to.clone();

    // make options
    let mut options = vec![
        #[cfg(unix)]
        remotefs_fuse::MountOption::AllowRoot,
        #[cfg(unix)]
        remotefs_fuse::MountOption::RW,
        #[cfg(unix)]
        remotefs_fuse::MountOption::Exec,
        #[cfg(unix)]
        remotefs_fuse::MountOption::Sync,
        #[cfg(unix)]
        remotefs_fuse::MountOption::FSName(volume),
    ];
    options.extend(args.option.clone());

    #[cfg(unix)]
    if let Some(uid) = args.uid {
        log::info!("Default uid: {uid}");
        options.push(remotefs_fuse::MountOption::Uid(uid));
    }
    #[cfg(unix)]
    if let Some(gid) = args.gid {
        log::info!("Default gid: {gid}");
        options.push(remotefs_fuse::MountOption::Gid(gid));
    }
    #[cfg(unix)]
    if let Some(default_mode) = args.default_mode {
        log::info!("Default mode: {default_mode:o}");
        options.push(remotefs_fuse::MountOption::DefaultMode(default_mode));
    }

    log::info!("Mounting remote fs at {}", mount_path.display());

    // create the mount point if it does not exist
    #[cfg(unix)]
    if !mount_path.exists() {
        log::info!("creating mount point at {}", mount_path.display());
        std::fs::create_dir_all(&mount_path)?;
    }

    // Mount the remote file system
    let remote = args.remote()?;
    let mut mount = Mount::mount(remote, &mount_path, &options)?;
    let mut umount = mount.unmounter();

    // setup signal handler
    ctrlc::set_handler(move || {
        log::info!("Received SIGINT, unmounting filesystem");
        if let Err(err) = umount.unmount() {
            log::error!("Failed to unmount: {err}");
        }
    })?;

    log::info!("Running filesystem event loop");
    mount.run()?;

    Ok(())
}
