use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fuser::MountOption;
use remotefs_fuse::{Mount, Umount};
use tempfile::TempDir;

use crate::driver::mounted_file_path;

pub type UmountLock = Arc<Mutex<Option<Umount>>>;

/// Mounts the filesystem in a separate thread.
///
/// The filesystem must be unmounted manually and then the thread must be joined.
fn mount(p: &Path) -> (UmountLock, JoinHandle<()>) {
    let mountpoint = p.to_path_buf();

    let error_flag = Arc::new(AtomicBool::new(false));
    let error_flag_t = error_flag.clone();

    let umount = Arc::new(Mutex::new(None));
    let umount_t = umount.clone();

    let join = std::thread::spawn(move || {
        let mut mount = Mount::mount(
            crate::driver::setup_driver(),
            &mountpoint,
            &[
                MountOption::AllowRoot,
                MountOption::RW,
                MountOption::Exec,
                MountOption::Sync,
            ],
        )
        .expect("failed to mount");

        let umount = mount.unmounter();
        *umount_t.lock().unwrap() = Some(umount);

        mount.run().expect("failed to run filesystem event loop");

        // set the error flag if the filesystem was unmounted
        error_flag_t.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // wait for the filesystem to be mounted
    std::thread::sleep(Duration::from_secs(1));
    if error_flag.load(std::sync::atomic::Ordering::Relaxed) {
        panic!("Failed to mount filesystem");
    }

    (umount, join)
}

fn umount(umount: UmountLock) {
    umount
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .umount()
        .expect("Failed to unmount");
}

#[test]
fn test_should_mount_fs() {
    let mnt = TempDir::new().expect("Failed to create tempdir");
    // mount
    let (umounter, join) = mount(mnt.path());
    // mounted file exists
    let mounted_file_path = PathBuf::from(format!(
        "{}{}",
        mnt.path().display(),
        mounted_file_path().display()
    ));
    println!("Mounted file path: {:?}", mounted_file_path);
    assert!(mounted_file_path.exists());

    // unmount
    umount(umounter);

    join.join().expect("Failed to join thread");
}
