use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use tempfile::TempDir;

/// Mounts the filesystem in a separate thread.
///
/// The filesystem must be unmounted manually and then the thread must be joined.
fn mount(p: &Path) -> JoinHandle<()> {
    let mountpoint = p.to_path_buf();
    let error_flag = Arc::new(AtomicBool::new(false));
    let error_flag_t = error_flag.clone();

    let join = std::thread::spawn(move || {
        let driver = crate::driver::setup_driver();
        // this operation is blocking and will not return until the filesystem is unmounted
        assert!(remotefs_fuse::mount(driver, &mountpoint, &[]).is_ok());

        // set the error flag if the filesystem was unmounted
        error_flag_t.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // wait for the filesystem to be mounted
    std::thread::sleep(Duration::from_secs(1));
    if error_flag.load(std::sync::atomic::Ordering::Relaxed) {
        panic!("Failed to mount filesystem");
    }

    join
}

/// Unmounts the filesystem.
fn umount(path: &Path) -> Result<(), String> {
    // Converti il Path in una stringa C
    let path_cstr = match std::ffi::CString::new(path.to_str().ok_or("Invalid path")?) {
        Ok(cstr) => cstr,
        Err(_) => return Err("Failed to convert path to CString".into()),
    };

    // Chiamata alla funzione umount della libc
    let result = unsafe { libc::umount(path_cstr.as_ptr()) };

    if result == 0 {
        Ok(())
    } else {
        Err(format!(
            "umount failed with errno: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[test]
fn test_should_mount_fs() {
    let mnt = TempDir::new().expect("Failed to create tempdir");
    // mount
    let join = mount(mnt.path());
    // umount
    assert!(umount(mnt.path()).is_ok());
    // join
    assert!(join.join().is_ok());
}
