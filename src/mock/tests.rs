//! Additional in-memory fixture tests.

use super::MockRemoteFs;
use crate::fs::{Capabilities, RemoteErrorType, RemoteFs, WriteOptions};

#[test]
fn fixture_seeding_preserves_entry_kinds() {
    let fs = MockRemoteFs::connected();
    let directory = MockRemoteFs::path("directory");
    let file = directory.join("file");
    let link = directory.join("link");
    fs.seed_dir(&directory);
    fs.seed_file(&file, b"data");
    fs.seed_symlink(&link, &file);

    assert!(fs.stat(&directory).unwrap().is_dir());
    assert!(fs.stat(&file).unwrap().is_file());
    assert!(fs.stat(&link).unwrap().is_symlink());
}

#[test]
fn fixture_controls_change_transfer_behavior() {
    let fs = MockRemoteFs::connected();
    fs.set_capabilities(Capabilities::empty());
    assert_eq!(fs.capabilities(), Capabilities::empty());
    fs.require_size_hint(true);
    let path = MockRemoteFs::path("required");
    assert_eq!(
        fs.create(&path, &WriteOptions::default())
            .unwrap_err()
            .kind(),
        RemoteErrorType::UnsupportedFeature
    );
    fs.fail_finish(Some(RemoteErrorType::ProtocolError));
    assert_eq!(fs.finish_count(), 0);
}
