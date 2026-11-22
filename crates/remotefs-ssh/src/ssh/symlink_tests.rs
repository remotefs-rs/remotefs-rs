//! Symlink contract tests shared by the SSH transports.

use std::path::Path;

use remotefs::RemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};

pub(super) fn check(client: &dyn RemoteFs, root: &Path) {
    let dir = root.join("dir");
    let sibling = root.join("sibling");
    client.create_dir(&dir, None).unwrap();
    client.create_dir(&sibling, None).unwrap();
    let target = sibling.join("target.txt");
    client
        .write_file(
            &target,
            &WriteOptions::default().size_hint(4),
            &mut &b"data"[..],
        )
        .unwrap();
    for (name, payload) in [
        ("abs", target.as_path()),
        ("abs-missing", sibling.join("missing.txt").as_path()),
        ("rel", Path::new("../sibling/target.txt")),
        ("rel-missing", Path::new("../missing.txt")),
        ("dir-spelling", Path::new("../sibling//./dir/")),
        ("backslash", Path::new(r"a\b")),
    ] {
        let link = dir.join(name);
        client.symlink(&link, payload).unwrap();
        let entry = client.stat(&link).unwrap();
        assert!(entry.is_symlink());
        assert_eq!(
            entry.metadata().symlink.as_ref().unwrap().as_os_str(),
            payload.as_os_str()
        );
    }
    let mut data = Vec::new();
    client
        .read_file(&dir.join("rel"), &ReadOptions::default(), &mut data)
        .unwrap();
    assert_eq!(data, b"data");

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let payload = std::ffi::OsStr::from_bytes(b"../\xff'\n");
        let link = dir.join("raw");
        client.symlink(&link, Path::new(payload)).unwrap();
        // Read the stored bytes independently of how a backend displays metadata.
        let quoted = super::scp::shell::quote(&link);
        let output = client
            .exec(&format!("readlink -n {quoted} | od -An -tx1 | tr -d ' '"))
            .unwrap();
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout.trim_end(), "2e2e2fff270a");
        client.remove_file(&link).unwrap();
    }
}

#[cfg(any(feature = "libssh", feature = "libssh2"))]
pub(super) fn check_sftp_missing_parent(client: &dyn RemoteFs, root: &Path) {
    let link = root.join("missing-parent/link");
    for target in [
        Path::new("target"),
        Path::new(r"a\b"),
        #[cfg(unix)]
        Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"\xff")),
    ] {
        let err = client.symlink(&link, target).unwrap_err();
        assert_eq!(err.kind(), remotefs::RemoteErrorType::NoSuchFileOrDirectory);
    }
}

pub(super) fn check_occupied_directory(client: &dyn RemoteFs, root: &Path) {
    let dir = root.join("occupied");
    client.create_dir(&dir, None).unwrap();
    let link = root.join("dir-link");
    client.symlink(&link, Path::new("occupied")).unwrap();
    for occupied in [&dir, &link] {
        let err = client
            .symlink(occupied, Path::new("../missing"))
            .unwrap_err();
        assert_eq!(err.kind(), remotefs::RemoteErrorType::AlreadyExists);
        assert!(!client.exists(&dir.join("missing")).unwrap());
    }
}

pub(super) fn check_explicit_order(client: &dyn RemoteFs, root: &Path) {
    // OpenSSH consumes the first operand as the target. Overriding it to
    // link-first makes the reversed operands observable on the server.
    let first = root.join("first");
    let second = root.join("second");
    client.symlink(&first, &second).unwrap();
    assert_eq!(
        client.stat(&first).unwrap_err().kind(),
        remotefs::RemoteErrorType::NoSuchFileOrDirectory
    );
    assert_eq!(
        client.stat(&second).unwrap().metadata().symlink.as_deref(),
        Some(first.as_path())
    );
}
