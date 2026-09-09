//! Additional in-memory fixture tests.

use super::MockRemoteFs;
#[cfg(feature = "async")]
use crate::fs::ReadOptions;
use crate::fs::{Capabilities, RemoteErrorType, RemoteFs, WriteOptions};
#[cfg(feature = "async")]
use crate::mock::async_io::{AsyncCursor, run};

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

#[cfg(feature = "async")]
#[test]
fn boxed_async_client_runs_default_transfers() {
    run(async {
        use crate::fs::AsyncRemoteFs;

        fn assert_send<T: Send>(_: T) {}

        let mut fs: Box<dyn AsyncRemoteFs> = Box::new(MockRemoteFs::new());
        AsyncRemoteFs::connect(&mut fs).await.unwrap();
        let path = MockRemoteFs::path("async-upload");
        let mut input = AsyncCursor::new(b"hello".to_vec());
        assert_send(AsyncRemoteFs::stat(&fs, &path));
        assert_eq!(
            AsyncRemoteFs::write_file(&fs, &path, &WriteOptions::default(), &mut input)
                .await
                .unwrap(),
            5
        );
        let mut output = AsyncCursor::new(Vec::new());
        AsyncRemoteFs::read_file(&fs, &path, &ReadOptions::default(), &mut output)
            .await
            .unwrap();
        assert_eq!(output.into_inner(), b"hello");
    });
}

#[cfg(feature = "async")]
#[test]
fn async_defaults_mirror_stream_and_tree_behavior() {
    run(async {
        use crate::fs::AsyncRemoteFs;

        let fs = MockRemoteFs::connected();
        let path = MockRemoteFs::path("async-file");
        let mut input = AsyncCursor::new(b"old".to_vec());
        assert_eq!(
            AsyncRemoteFs::write_file(&fs, &path, &WriteOptions::default(), &mut input)
                .await
                .unwrap(),
            3
        );
        let mut input = AsyncCursor::new(b"!".to_vec());
        AsyncRemoteFs::append_file(&fs, &path, &WriteOptions::default(), &mut input)
            .await
            .unwrap();
        assert_eq!(fs.contents(&path), b"old!");

        let mut output = AsyncCursor::new(Vec::new());
        AsyncRemoteFs::read_file(
            &fs,
            &path,
            &ReadOptions::default().offset(1).length(2),
            &mut output,
        )
        .await
        .unwrap();
        assert_eq!(output.into_inner(), b"ld");
        assert_eq!(fs.finish_count(), 3);
        assert_eq!(fs.unfinished_count(), 0);
    });
}

#[cfg(feature = "async")]
#[test]
fn async_defaults_remove_tree_without_following_links() {
    run(async {
        use crate::fs::AsyncRemoteFs;

        let fs = MockRemoteFs::connected();
        let root = MockRemoteFs::path("async-tree");
        let nested = root.join("nested");
        let file = nested.join("file");
        fs.seed_dir(&root);
        fs.seed_dir(&nested);
        fs.seed_file(&file, b"data");
        fs.seed_symlink(&nested.join("cycle"), &root);

        AsyncRemoteFs::remove_dir_all(&fs, &root).await.unwrap();
        assert!(!RemoteFs::exists(&fs, &root).unwrap());
        assert!(!RemoteFs::exists(&fs, &file).unwrap());
    });
}

#[cfg(feature = "async")]
#[test]
fn async_mock_validates_paths_capabilities_and_size_hints() {
    run(async {
        use crate::fs::{AsyncRemoteFs, RemoteErrorType};

        let fs = MockRemoteFs::new();
        let relative = std::path::Path::new("relative");
        assert_eq!(
            AsyncRemoteFs::stat(&fs, relative).await.unwrap_err().kind(),
            RemoteErrorType::InvalidPath
        );
        let path = MockRemoteFs::path("async-file");
        assert_eq!(
            AsyncRemoteFs::stat(&fs, &path).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );

        let fs = MockRemoteFs::connected();
        fs.set_capabilities(Capabilities::empty());
        assert_eq!(
            AsyncRemoteFs::open(&fs, &path, &ReadOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        fs.require_size_hint(true);
        assert_eq!(
            AsyncRemoteFs::create(&fs, &path, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::UnsupportedFeature
        );
        fs.set_capabilities(Capabilities::STREAM_WRITE);
        assert_eq!(
            AsyncRemoteFs::create(&fs, &path, &WriteOptions::default())
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::SizeRequired
        );
        assert_eq!(fs.finish_count(), 0);
    });
}

#[cfg(feature = "async")]
#[test]
fn async_mock_does_not_commit_after_finish_failure() {
    run(async {
        use crate::fs::{AsyncRemoteFs, RemoteErrorType};

        let fs = MockRemoteFs::connected();
        fs.fail_finish(Some(RemoteErrorType::ProtocolError));
        let path = MockRemoteFs::path("async-failed");
        let mut input = AsyncCursor::new(b"data".to_vec());
        let error = AsyncRemoteFs::write_file(&fs, &path, &WriteOptions::default(), &mut input)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::ProtocolError);
        assert_eq!(fs.finish_count(), 1);
        assert_eq!(fs.unfinished_count(), 0);
        assert!(!RemoteFs::exists(&fs, &path).unwrap());
    });
}
