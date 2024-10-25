mod cli;

use remotefs_fuse::{Driver, Mount, MountOption};

fn main() -> anyhow::Result<()> {
    let args = argh::from_env::<cli::CliArgs>();
    args.init_logger()?;
    let volume = args.volume.clone();
    let mount_path = args.to.clone();

    // make options
    let mut options = vec![
        #[cfg(unix)]
        MountOption::AllowRoot,
        #[cfg(unix)]
        MountOption::RW,
        #[cfg(unix)]
        MountOption::Exec,
        #[cfg(unix)]
        MountOption::Sync,
        #[cfg(unix)]
        MountOption::FSName(volume),
    ];

    if let Some(uid) = args.uid {
        log::info!("Default uid: {uid}");
        options.push(MountOption::Uid(uid));
    }
    if let Some(gid) = args.gid {
        log::info!("Default gid: {gid}");
        options.push(MountOption::Gid(gid));
    }
    if let Some(default_mode) = args.default_mode {
        log::info!("Default mode: {default_mode:o}");
        options.push(MountOption::DefaultMode(default_mode));
    }

    log::info!("Mounting remote fs at {}", mount_path.display());

    // create the mount point if it does not exist
    if !mount_path.exists() {
        log::info!("creating mount point at {}", mount_path.display());
        std::fs::create_dir_all(&mount_path)?;
    }

    // Mount the remote file system
    let mut mount = Mount::mount(Driver::new(args.remote()), &mount_path, &options)?;
    let mut umount = mount.unmounter();

    // setup signal handler
    ctrlc::set_handler(move || {
        log::info!("Received SIGINT, unmounting filesystem");
        umount.umount().expect("Failed to unmount");
    })?;

    log::info!("Running filesystem event loop");
    mount.run()?;

    Ok(())
}
