use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use remotefs_fuse::{Mount, MountOption, Unmount};
use serial_test::serial;

use crate::driver::mounted_file_path;

pub type UnmountLock = Arc<Mutex<Option<Unmount>>>;

static CURRENT_DRIVE: AtomicUsize = AtomicUsize::new(0);

/// Mounts the filesystem in a separate thread.
///
/// The filesystem must be unmounted manually and then the thread must be joined.
fn mount(p: &Path) -> (UnmountLock, JoinHandle<()>) {
    let mountpoint = p.to_path_buf();
    let mountpoint_t = mountpoint.clone();

    let error_flag = Arc::new(AtomicBool::new(false));
    let error_flag_t = error_flag.clone();

    let umount = Arc::new(Mutex::new(None));
    let umount_t = umount.clone();

    let join = std::thread::spawn(move || {
        let mut mount = Mount::mount(crate::driver::setup_driver(), &mountpoint_t, &[])
            .expect("failed to mount");

        let umount = mount.unmounter();
        *umount_t.lock().unwrap() = Some(umount);

        let result = mount.run();
        if result.is_err() {
            error_flag_t.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        result.expect("failed to run filesystem event loop");

        // set the error flag if the filesystem was unmounted
        error_flag_t.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // wait for the filesystem to be mounted
    let deadline = Instant::now() + Duration::from_secs(10);
    while !is_drive_mounted(&mountpoint) && Instant::now() < deadline {
        if error_flag.load(std::sync::atomic::Ordering::Relaxed) {
            panic!("Failed to mount filesystem");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    if !is_drive_mounted(&mountpoint) {
        panic!("Timed out waiting for filesystem to mount");
    }

    (umount, join)
}

fn umount(umount: UnmountLock) {
    umount
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .unmount()
        .expect("Failed to unmount");
}

fn next_driver() -> PathBuf {
    const DRIVE_COUNT: usize = 26;

    let current = CURRENT_DRIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % DRIVE_COUNT;
    let drives = unsafe { winapi::um::fileapi::GetLogicalDrives() };

    for offset in 0..DRIVE_COUNT {
        let index = (current + offset) % DRIVE_COUNT;
        if drives & (1 << index) == 0 {
            return PathBuf::from(char::from(b'A' + index as u8).to_string());
        }
    }

    panic!("No unused drive letter is available");
}

fn is_drive_mounted(drive: &Path) -> bool {
    let drive_index = drive
        .to_string_lossy()
        .as_bytes()
        .first()
        .expect("drive letter is missing")
        .to_ascii_uppercase()
        - b'A';
    let drives = unsafe { winapi::um::fileapi::GetLogicalDrives() };

    drives & (1 << drive_index) != 0
}

#[test]
#[serial]
fn test_should_select_unused_drive() {
    let drive = next_driver();

    assert!(
        !is_drive_mounted(&drive),
        "drive is already in use: {drive:?}"
    );
}

#[cfg(feature = "tokio")]
async fn with_mounted_async_drive<F, Fut>(f: F)
where
    F: FnOnce(PathBuf) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    use remotefs::RemoteFs;
    use remotefs::adapters::r#async::Unblock;
    use remotefs_fuse::AsyncMount;

    let _ = env_logger::try_init();
    let mnt = next_driver();
    let mut remote = crate::driver::setup_driver();
    remote.disconnect().expect("disconnect");
    let mut mount = AsyncMount::mount(Unblock::new(remote), &mnt, &[MountOption::RW])
        .await
        .expect("failed to mount");
    let unmount = mount.unmounter();
    let run = tokio::spawn(mount.run());

    let deadline = Instant::now() + Duration::from_secs(10);
    while !is_drive_mounted(&mnt) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        is_drive_mounted(&mnt),
        "timed out waiting for filesystem to mount"
    );

    f(mnt.clone()).await;

    unmount.unmount().await.expect("Failed to unmount");
    run.await
        .expect("event loop task panicked")
        .expect("failed to run filesystem event loop");
    tokio::time::sleep(Duration::from_secs(3)).await;
}

#[cfg(feature = "tokio")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_should_mount_async_fs_and_round_trip_a_file() {
    with_mounted_async_drive(|mnt| async move {
        let mounted_file = path_to_drive(&mnt, mounted_file_path());
        assert!(mounted_file.exists());
        let file_path = path_to_drive(&mnt, Path::new("async.txt"));
        tokio::fs::write(&file_path, "Hello, async world!")
            .await
            .expect("write");
        assert_eq!(
            tokio::fs::read_to_string(&file_path).await.expect("read"),
            "Hello, async world!"
        );
    })
    .await;
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
