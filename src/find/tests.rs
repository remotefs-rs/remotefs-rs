use std::path::Path;

use crate::fs::{
    Capabilities, ExecOutput, File, ReadOptions, ReadStream, RemoteResult, SetMetadata, UnixPex,
    WriteOptions, WriteStream,
};
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

// This listing preserves remote paths verbatim instead of deriving parents with
// the client platform's path parser.
struct RemoteListing {
    root: std::path::PathBuf,
    entries: Vec<crate::File>,
}

impl crate::RemoteFs for RemoteListing {
    fn connect(&mut self) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn is_connected(&self) -> bool {
        unimplemented!("search only lists directories")
    }

    fn capabilities(&self) -> Capabilities {
        unimplemented!("search only lists directories")
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        assert_eq!(path.as_os_str(), self.root.as_os_str());
        Ok(self.entries.clone())
    }

    fn stat(&self, _: &Path) -> RemoteResult<File> {
        unimplemented!("search only lists directories")
    }

    fn exists(&self, _: &Path) -> RemoteResult<bool> {
        unimplemented!("search only lists directories")
    }

    fn set_metadata(&self, _: &Path, _: &SetMetadata) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn create_dir(&self, _: &Path, _: Option<UnixPex>) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn remove_file(&self, _: &Path) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn remove_dir(&self, _: &Path) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn rename(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn copy(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn symlink(&self, _: &Path, _: &Path) -> RemoteResult<()> {
        unimplemented!("search only lists directories")
    }

    fn open(&self, _: &Path, _: &ReadOptions) -> RemoteResult<ReadStream> {
        unimplemented!("search only lists directories")
    }

    fn create(&self, _: &Path, _: &WriteOptions) -> RemoteResult<WriteStream> {
        unimplemented!("search only lists directories")
    }

    fn append(&self, _: &Path, _: &WriteOptions) -> RemoteResult<WriteStream> {
        unimplemented!("search only lists directories")
    }

    fn exec(&self, _: &str) -> RemoteResult<ExecOutput> {
        unimplemented!("search only lists directories")
    }
}

#[test]
fn search_matches_foreign_remote_names_exactly() {
    for (root, path, name) in [
        (r"C:\logs", r"C:\logs\report.txt", "report.txt"),
        (r"\\server\share", r"\\server\share\.hidden", ".hidden"),
        ("/logs", r"/logs/report\part.txt", r"report\part.txt"),
    ] {
        let fs = RemoteListing {
            root: root.into(),
            entries: vec![File::new(path, crate::fs::Metadata::default())],
        };
        let result = crate::find(&fs, Path::new(root), name).unwrap();
        assert_eq!(result.len(), 1, "path: {path:?}");
        assert_eq!(result[0].path.as_os_str(), Path::new(path).as_os_str());
    }
}
