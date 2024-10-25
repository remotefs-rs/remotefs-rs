use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

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
        let mut mount =
            Mount::mount(crate::driver::setup_driver(), &mountpoint).expect("failed to mount");

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

/// Mounts the filesystem and calls the provided closure with the mountpoint.
fn with_mounted_drive<F>(f: F)
where
    F: FnOnce(&Path),
{
    let _ = env_logger::try_init();
    let mnt = TempDir::new().expect("Failed to create tempdir");
    // mount
    let (umounter, join) = mount(mnt.path());
    f(mnt.path());
    // unmount
    umount(umounter);
    join.join().expect("Failed to join thread");
}

#[test]
fn test_should_mount_fs() {
    with_mounted_drive(|mnt| {
        let mounted_file_path = PathBuf::from(format!(
            "{}{}",
            mnt.display(),
            mounted_file_path().display()
        ));
        println!("Mounted file path: {:?}", mounted_file_path);
        assert!(mounted_file_path.exists());
    });
}

#[test]
fn test_should_create_file() {
    with_mounted_drive(|mnt| {
        let file_path = mnt.to_path_buf().join("test.txt");
        let file_content = "Hello, World!";
        std::fs::write(&file_path, file_content).expect("Failed to write to file");

        // read from file
        let read_content = std::fs::read_to_string(&file_path).expect("Failed to read from file");
        assert_eq!(file_content, read_content);
    });
}

#[test]
fn test_should_unlink_file() {
    with_mounted_drive(|mnt| {
        let file_path = mnt.to_path_buf().join("test.txt");
        let file_content = "Hello, World!";
        std::fs::write(&file_path, file_content).expect("Failed to write to file");

        // unlink file
        std::fs::remove_file(&file_path).expect("Failed to unlink file");
        assert!(!file_path.exists());
    });
}

#[test]
fn test_should_make_and_remove_directory() {
    with_mounted_drive(|mnt| {
        let dir_path = mnt.to_path_buf().join("test_dir");
        std::fs::create_dir(&dir_path).expect("Failed to create directory");
        assert!(dir_path.exists());

        // remove directory
        std::fs::remove_dir(&dir_path).expect("Failed to remove directory");
        assert!(!dir_path.exists());
    });
}

#[test]
#[ignore = "something is wrong with the symlink implementation in Rust."]
fn test_should_make_symlink() {
    with_mounted_drive(|mnt| {
        let file_path = mnt.to_path_buf().join("test.txt");
        let file_content = "Hello, World!";
        log::warn!("writing file to: {:?}", file_path);
        std::fs::write(&file_path, file_content).expect("Failed to write to file");

        log::warn!("create symlink");
        // symlink path should be relative to the mountpoint
        // but Rust expects an absolute path??? So if we use a relative path, it will work,
        // but it will create the symlink in the wrong place.
        let symlink_path = PathBuf::from("/test_symlink.txt");
        std::os::unix::fs::symlink(&file_path, &symlink_path).expect("Failed to create symlink");

        // read from symlink
        log::warn!("read file from: {:?}", symlink_path);
        let read_content =
            std::fs::read_to_string(&symlink_path).expect("Failed to read from symlink");
        assert_eq!(file_content, read_content);
    });
}
