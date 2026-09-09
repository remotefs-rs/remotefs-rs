use std::path::Path;

use super::WorkingDir;
use crate::fs::{RemoteErrorType, RemoteFs};
use crate::mock::MockRemoteFs;

#[test]
fn working_dir_resolves_paths_for_a_boxed_client() {
    let fs = MockRemoteFs::connected();
    let cwd = MockRemoteFs::path("work");
    fs.seed_dir(&cwd);
    fs.seed_file(&cwd.join("note.txt"), b"notes");
    let boxed: Box<dyn RemoteFs> = Box::new(fs);
    let wrapper = WorkingDir::new(boxed, cwd.clone());
    assert_eq!(wrapper.pwd(), cwd);
    assert_eq!(
        wrapper.stat(Path::new("note.txt")).unwrap().metadata.size,
        Some(5)
    );
    assert_eq!(
        wrapper
            .change_dir(Path::new("note.txt"))
            .unwrap_err()
            .kind(),
        RemoteErrorType::BadFile
    );
    assert_eq!(wrapper.pwd(), cwd);
}

#[test]
fn working_dir_handles_absolute_nested_and_empty_paths() {
    let fs = MockRemoteFs::connected();
    let cwd = MockRemoteFs::path("work");
    let nested = cwd.join("nested");
    fs.seed_dir(&cwd);
    fs.seed_dir(&nested);
    let absolute = nested.join("file");
    fs.seed_file(&absolute, b"data");
    let wrapper = WorkingDir::new(fs, cwd.clone());

    assert!(wrapper.stat(Path::new("nested")).unwrap().is_dir());
    assert!(wrapper.stat(&absolute).unwrap().is_file());
    assert_eq!(
        wrapper.stat(Path::new("")).unwrap_err().kind(),
        RemoteErrorType::InvalidPath
    );
    assert_eq!(wrapper.pwd(), cwd);
}

#[test]
fn working_dir_changes_only_after_a_successful_directory_stat() {
    let fs = MockRemoteFs::connected();
    let cwd = MockRemoteFs::path("work");
    let next = cwd.join("next");
    fs.seed_dir(&cwd);
    fs.seed_dir(&next);
    let wrapper = WorkingDir::new(fs, cwd.clone());

    assert_eq!(wrapper.change_dir(Path::new("next")).unwrap(), next);
    assert_eq!(wrapper.pwd(), next);
    assert_eq!(
        wrapper.change_dir(Path::new("missing")).unwrap_err().kind(),
        RemoteErrorType::NoSuchFileOrDirectory
    );
    assert_eq!(wrapper.pwd(), next);
}

#[test]
#[should_panic(expected = "working directory must be absolute")]
fn working_dir_rejects_relative_initial_cwd() {
    let _ = WorkingDir::new(MockRemoteFs::connected(), Path::new("relative"));
}

#[test]
fn working_dir_into_inner_returns_backend() {
    let fs = MockRemoteFs::connected();
    let cwd = MockRemoteFs::path("work");
    fs.seed_dir(&cwd);
    let wrapper = WorkingDir::new(fs, cwd);
    assert!(wrapper.into_inner().is_connected());
}

#[test]
fn working_dir_resolves_both_paths_from_one_snapshot() {
    let fs = MockRemoteFs::connected();
    let cwd = MockRemoteFs::path("work");
    let source = cwd.join("source");
    fs.seed_dir(&cwd);
    fs.seed_file(&source, b"data");
    let wrapper = WorkingDir::new(fs, cwd.clone());
    wrapper
        .rename(Path::new("source"), Path::new("renamed"))
        .unwrap();
    assert!(wrapper.stat(&cwd.join("renamed")).unwrap().is_file());
}

#[test]
fn working_dir_symlink_resolves_path_and_target() {
    let fs = MockRemoteFs::connected();
    let cwd = MockRemoteFs::path("work");
    fs.seed_dir(&cwd);
    let wrapper = WorkingDir::new(fs, cwd.clone());
    wrapper
        .symlink(Path::new("link"), Path::new("target"))
        .unwrap();
    assert_eq!(
        wrapper.stat(Path::new("link")).unwrap().metadata.symlink,
        Some(cwd.join("target"))
    );
}

#[cfg(feature = "async")]
#[test]
fn async_working_dir_resolves_paths_for_a_boxed_client() {
    crate::mock::async_io::run(async {
        use crate::fs::AsyncRemoteFs;
        use crate::working_dir::AsyncWorkingDir;

        let fs = MockRemoteFs::connected();
        let cwd = MockRemoteFs::path("async-work");
        fs.seed_dir(&cwd);
        fs.seed_file(&cwd.join("note.txt"), b"notes");
        let boxed: Box<dyn AsyncRemoteFs> = Box::new(fs);
        let wrapper = AsyncWorkingDir::new(boxed, cwd.clone());
        assert_eq!(wrapper.pwd(), cwd);
        assert_eq!(
            AsyncRemoteFs::stat(&wrapper, Path::new("note.txt"))
                .await
                .unwrap()
                .metadata
                .size,
            Some(5)
        );
        assert_eq!(
            wrapper
                .change_dir(Path::new("note.txt"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::BadFile
        );
        assert_eq!(wrapper.pwd(), cwd);
    });
}
