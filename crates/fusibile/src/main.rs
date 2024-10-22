mod cli;

use remotefs_fuse::{Driver, MountOption};

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = argh::from_env::<cli::CliArgs>();
    let volume = args.volume.clone();
    let mount_path = args.to.clone();
    let remote = args.remote();

    let driver = Driver::new(remote);

    // setup signal handler
    ctrlc::set_handler(move || {
        log::warn!(
            "Received interrupt signal. Please, umount file system to terminate the process."
        );
    })?;

    log::info!("Mounting remote fs at {}", mount_path.display());

    // Mount the remote file system
    remotefs_fuse::mount(
        driver,
        &mount_path,
        &[
            MountOption::AllowRoot,
            MountOption::RW,
            MountOption::Exec,
            MountOption::Atime,
            MountOption::Sync,
            MountOption::FSName(volume),
        ],
    )?;

    Ok(())
}
