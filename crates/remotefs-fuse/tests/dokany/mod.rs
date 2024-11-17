use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use remotefs_fuse::{Mount, Umount};
use serial_test::serial;

use crate::driver::mounted_file_path;

pub type UmountLock = Arc<Mutex<Option<Umount>>>;

static AVAILABLE_DRIVES: &[&str] = &["Z", "Y", "X", "W", "V", "U", "T", "S", "R", "Q"];
static CURRENT_DRIVE: AtomicUsize = AtomicUsize::new(0);

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
            Mount::mount(crate::driver::setup_driver(), &mountpoint, &[]).expect("failed to mount");

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

fn next_driver() -> PathBuf {
    let current = CURRENT_DRIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let drive = AVAILABLE_DRIVES[current % AVAILABLE_DRIVES.len()];
    PathBuf::from(drive)
}

fn path_to_drive(mnt: &Path, path: &Path) -> PathBuf {
    let mut drive_path = PathBuf::from(format!("{}:\\", mnt.display()));
    drive_path.push(path);

    drive_path
}

/// Mounts the filesystem and calls the provided closure with the mountpoint.
fn with_mounted_drive<F>(f: F)
where
    F: FnOnce(&Path),
{
    let _ = env_logger::Builder::new()
        .is_test(true)
        .filter_level(log::LevelFilter::Debug)
        .try_init();
    let mnt = next_driver();
    // mount
    let (umounter, join) = mount(mnt.as_path());
    f(mnt.as_path());
    // unmount
    umount(umounter);
    join.join().expect("Failed to join thread");

    // wait for the filesystem to be unmounted
    std::thread::sleep(Duration::from_secs(3));
}

#[test]
#[serial]
fn test_should_mount_fs() {
    with_mounted_drive(|mnt| {
        let mounted_file_path = PathBuf::from(format!(
            "{}:\\{}",
            mnt.display(),
            mounted_file_path().display()
        ));
        println!("Mounted file path: {:?}", mounted_file_path);
        assert!(mounted_file_path.exists());
    });
}

#[test]
#[serial]
fn test_should_create_file() {
    with_mounted_drive(|mnt| {
        let file_path = PathBuf::from("test.txt");
        let file_path = path_to_drive(mnt, &file_path);
        let file_content = "Hello, World!";
        std::fs::write(&file_path, file_content).expect("Failed to write to file");

        // read from file
        let read_content = std::fs::read_to_string(&file_path).expect("Failed to read from file");
        assert_eq!(file_content, read_content);
    });
}

#[test]
#[serial]
fn test_should_unlink_file() {
    with_mounted_drive(|mnt| {
        let file_path = PathBuf::from("test.txt");
        let file_path = path_to_drive(mnt, &file_path);
        let file_content = "Hello, World!";
        std::fs::write(&file_path, file_content).expect("Failed to write to file");

        // unlink file
        std::fs::remove_file(&file_path).expect("Failed to unlink file");
        assert!(!file_path.exists());
    });
}

#[test]
#[serial]
#[ignore = "Strange behavior when removing the directory"]
fn test_should_make_and_remove_directory() {
    with_mounted_drive(|mnt| {
        let dir_path = PathBuf::from("test");
        let dir_path = path_to_drive(mnt, &dir_path);
        std::fs::create_dir(&dir_path).expect("Failed to create directory");
        assert!(dir_path.exists());

        // wait for the filesystem to cleanup
        std::thread::sleep(Duration::from_secs(1));

        // remove directory
        println!("Removing directory: {:?}", dir_path);
        std::fs::remove_dir_all(&dir_path).expect("Failed to remove directory");
        assert!(!dir_path.exists());
    });
}
