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
