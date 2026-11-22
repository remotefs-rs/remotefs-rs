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

use clap::Parser;
use remotefs_fuse::{AsyncMount, AsyncUnmount};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        tokio::fs::create_dir_all(&mount_path).await?;
    }

    // Mount the remote file system
    let remote = args.remote()?;
    let mut mount = AsyncMount::mount(remote, &mount_path, &options).await?;
    let unmount = mount.unmounter();

    log::info!("Running filesystem event loop");
    let mut run = tokio::spawn(mount.run());

    tokio::select! {
        result = &mut run => {
            result??;
            log::info!("Filesystem unmounted");
            Ok(())
        }
        signal = shutdown_signal() => {
            signal?;
            log::info!("Received shutdown signal, unmounting filesystem");
            unmount_and_wait(unmount, run).await
        }
    }
}

/// Resolve on `SIGINT` (Ctrl+C) or, on Unix, `SIGTERM`.
async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

async fn wait_for_run_after_unmount<U, R>(unmount: U, run: R) -> anyhow::Result<()>
where
    U: std::future::Future<Output = Result<(), std::io::Error>>,
    R: std::future::Future<Output = anyhow::Result<()>>,
{
    let unmount_result = unmount.await.map_err(anyhow::Error::from);
    let run_result = run.await;
    match (unmount_result, run_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(unmount), Ok(())) => Err(unmount),
        (Ok(()), Err(run)) => Err(run),
        (Err(unmount), Err(run)) => Err(anyhow::anyhow!(
            "unmount failed: {unmount}; event loop failed: {run}"
        )),
    }
}

/// Unmount, then wait for the event loop and client disconnect to finish.
async fn unmount_and_wait(
    unmount: AsyncUnmount,
    run: tokio::task::JoinHandle<std::io::Result<()>>,
) -> anyhow::Result<()> {
    wait_for_run_after_unmount(unmount.unmount(), async move {
        run.await
            .map_err(anyhow::Error::from)?
            .map_err(anyhow::Error::from)
    })
    .await?;
    log::info!("Filesystem unmounted");
    Ok(())
}

#[cfg(test)]
mod test {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::wait_for_run_after_unmount;

    #[tokio::test]
    async fn test_should_wait_for_run_even_when_unmount_fails() {
        let run_completed = Arc::new(AtomicBool::new(false));
        let run_completed_clone = Arc::clone(&run_completed);
        let error = wait_for_run_after_unmount(
            async { Err(std::io::Error::other("unmount failed")) },
            async move {
                run_completed_clone.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await
        .expect_err("unmount failure must be returned");

        assert!(run_completed.load(Ordering::Acquire));
        assert_eq!(error.to_string(), "unmount failed");
    }
}
