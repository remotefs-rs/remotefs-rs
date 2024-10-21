mod cli;

use remotefs_fuse::{Driver, MountOption};
use tempfile::TempDir;

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let args = argh::from_env::<cli::CliArgs>();
    let mount_path = args.to.clone();
    let remote = args.remote();
    let data_dir = TempDir::new()?;

    let driver = Driver::new(data_dir.path(), remote)?;

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
        ],
    )?;

    Ok(())
}
