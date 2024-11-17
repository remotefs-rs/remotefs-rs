mod cli;
mod remotefs_wrapper;

use remotefs_fuse::Mount;

fn main() -> anyhow::Result<()> {
    let args = argh::from_env::<cli::CliArgs>();
    args.init_logger()?;
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
    let remote = args.remote();
    let mut mount = Mount::mount(remote, &mount_path, &options)?;
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
