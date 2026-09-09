use crate::mock::MockRemoteFs;

#[test]
fn search_includes_matching_directories_and_nested_files() {
    let fs = MockRemoteFs::connected();
    let root = MockRemoteFs::path("search");
    fs.seed_dir(&root);
    fs.seed_dir(&root.join("docs.rs"));
    fs.seed_file(&root.join("docs.rs/lib.rs"), b"rust");
    fs.seed_file(&root.join("readme.txt"), b"text");
    let result = crate::find(&fs, &root, "*.rs").unwrap();
    let paths: Vec<_> = result.into_iter().map(|file| file.path).collect();
    assert_eq!(
        paths,
        vec![root.join("docs.rs"), root.join("docs.rs/lib.rs")]
    );
}

#[test]
fn search_matches_names_and_does_not_include_the_start_directory() {
    let fs = MockRemoteFs::connected();
    let root = MockRemoteFs::path("search-names");
    fs.seed_dir(&root);
    fs.seed_dir(&root.join("docs"));
    fs.seed_file(&root.join("docs/readme.md"), b"markdown");
    fs.seed_file(&root.join("other.txt"), b"text");

    let result = crate::find(&fs, &root, "?.md").unwrap();
    assert!(result.is_empty());
    let result = crate::find(&fs, &root, "*.md").unwrap();
    assert_eq!(result[0].path, root.join("docs/readme.md"));
    assert!(!result.iter().any(|file| file.path == root));
}

#[cfg(feature = "async")]
#[test]
fn async_search_matches_sync_depth_first_results() {
    crate::mock::async_io::run(async {
        let fs = MockRemoteFs::connected();
        let root = MockRemoteFs::path("async-search");
        fs.seed_dir(&root);
        fs.seed_dir(&root.join("docs.rs"));
        fs.seed_file(&root.join("docs.rs/lib.rs"), b"rust");
        fs.seed_file(&root.join("readme.txt"), b"text");
        let sync = crate::find(&fs, &root, "*.rs").unwrap();
        let asynchronous = crate::find_async(&fs, &root, "*.rs").await.unwrap();
        let sync_paths: Vec<_> = sync.into_iter().map(|file| file.path).collect();
        let async_paths: Vec<_> = asynchronous.into_iter().map(|file| file.path).collect();
        assert_eq!(async_paths, sync_paths);
    });
}

#[cfg(feature = "async")]
#[test]
fn async_search_rejects_relative_roots() {
    crate::mock::async_io::run(async {
        use crate::fs::{AsyncRemoteFs, RemoteErrorType};

        let fs = MockRemoteFs::connected();
        let error = crate::find_async(&fs, std::path::Path::new("relative"), "*")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), RemoteErrorType::InvalidPath);
        assert!(AsyncRemoteFs::is_connected(&fs));
    });
}
