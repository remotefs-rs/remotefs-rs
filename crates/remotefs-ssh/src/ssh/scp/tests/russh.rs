use std::io::Cursor;
use std::sync::Arc;
use std::time::SystemTime;

use pretty_assertions::assert_eq;
use remotefs::fs::FileType;
use ssh2_config::ParseRule;

use super::*;
use crate::mock::ssh as ssh_mock;
use crate::ssh::backend::NoCheckServerKey;
use crate::ssh::container::OpensshServer;

fn test_runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
    )
}

#[test]
fn should_not_append_to_file() {
    crate::mock::logger();

    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "Hello, world!\n";
    let reader = Cursor::new(file_data.as_bytes());
    assert!(
        client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_change_directory() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let pwd = client.pwd().ok().unwrap();
    assert!(client.change_dir(Path::new("/tmp")).is_ok());
    assert!(client.change_dir(pwd.as_path()).is_ok());
    finalize_client(client);
}

#[test]
fn should_not_change_directory() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(
        client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_copy_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    assert!(client.copy(p, Path::new("b.txt")).is_ok());
    assert!(client.stat(p).is_ok());
    assert!(client.stat(Path::new("b.txt")).is_ok());
    finalize_client(client);
}

#[test]
fn should_not_copy_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
    finalize_client(client);
}

#[test]
fn should_create_directory() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(
        client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok()
    );
    let p = PathBuf::from(format!("{}/mydir", client.pwd().unwrap().display()));
    assert!(client.exists(&p).unwrap());
    finalize_client(client);
}

#[test]
fn should_not_create_directory_cause_already_exists() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(
        client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok()
    );
    assert_eq!(
        client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .err()
            .unwrap()
            .kind,
        RemoteErrorType::DirectoryAlreadyExists
    );
    finalize_client(client);
}

#[test]
fn should_not_create_directory() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(
        client
            .create_dir(
                Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"),
                UnixPex::from(0o755)
            )
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_create_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert_eq!(
        client
            .create_file(p, &metadata, Box::new(reader))
            .ok()
            .unwrap(),
        10
    );
    assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
    finalize_client(client);
}

#[test]
fn should_create_big_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = vec![1; 2 * 1024 * 1024]; // 2MB
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    let reader = Cursor::new(file_data);
    assert_eq!(
        client
            .create_file(p, &metadata, Box::new(reader))
            .ok()
            .unwrap(),
        2 * 1024 * 1024
    );
    assert_eq!(
        client.stat(p).ok().unwrap().metadata().size,
        2 * 1024 * 1024
    );
    finalize_client(client);
}

#[test]
fn should_read_big_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = vec![1; 2 * 1024 * 1024]; // 2MB
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    let reader = Cursor::new(file_data);
    assert_eq!(
        client
            .create_file(p, &metadata, Box::new(reader))
            .ok()
            .unwrap(),
        2 * 1024 * 1024
    );
    assert_eq!(
        client.stat(p).ok().unwrap().metadata().size,
        2 * 1024 * 1024
    );
    let dest = std::io::sink();
    assert_eq!(
        client
            .open_file(p, Box::new(dest))
            .expect("Cannot read file"),
        2 * 1024 * 1024
    );
    finalize_client(client);
}

#[test]
fn should_not_create_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_err());
    finalize_client(client);
}

#[test]
fn should_exec_command() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert_eq!(
        client.exec("echo 5").ok().unwrap(),
        (0, String::from("5\n"))
    );
    finalize_client(client);
}

#[test]
fn should_tell_whether_file_exists() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    assert_eq!(client.exists(p).ok().unwrap(), true);
    assert_eq!(client.exists(Path::new("b.txt")).ok().unwrap(), false);
    assert_eq!(
        client.exists(Path::new("/tmp/ppppp/bhhrhu")).ok().unwrap(),
        false
    );
    assert_eq!(client.exists(Path::new("/tmp")).ok().unwrap(), true);
    finalize_client(client);
}

#[test]
fn should_list_dir() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let wrkdir = client.pwd().ok().unwrap();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    let file = client
        .list_dir(wrkdir.as_path())
        .ok()
        .unwrap()
        .first()
        .unwrap()
        .clone();
    assert_eq!(file.name().as_str(), "a.txt");
    let mut expected_path = wrkdir;
    expected_path.push(p);
    assert_eq!(file.path.as_path(), expected_path.as_path());
    assert_eq!(file.extension().as_deref().unwrap(), "txt");
    assert_eq!(file.metadata.size, 10);
    assert_eq!(file.metadata.mode.unwrap(), UnixPex::from(0o644));
    finalize_client(client);
}

#[test]
fn should_not_list_dir() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
    finalize_client(client);
}

#[test]
fn should_move_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    let dest = Path::new("b.txt");
    assert!(client.mov(p, dest).is_ok());
    assert_eq!(client.exists(p).ok().unwrap(), false);
    assert_eq!(client.exists(dest).ok().unwrap(), true);
    finalize_client(client);
}

#[test]
fn should_not_move_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    let dest = Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt");
    assert!(client.mov(p, dest).is_err());
    assert!(
        client
            .mov(Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"), p)
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_open_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let metadata = Metadata::default().size(file_data.len() as u64);
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
    assert_eq!(client.open_file(p, buffer).ok().unwrap(), 10);
    finalize_client(client);
}

#[test]
fn should_not_open_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
    assert!(
        client
            .open_file(Path::new("/tmp/aashafb/hhh"), buffer)
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_print_working_directory() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(client.pwd().is_ok());
    finalize_client(client);
}

#[test]
fn should_remove_dir_all() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let mut dir_path = client.pwd().ok().unwrap();
    dir_path.push(Path::new("test/"));
    assert!(
        client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok()
    );
    let mut file_path = dir_path.clone();
    file_path.push(Path::new("a.txt"));
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(
        client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
            .is_ok()
    );
    assert!(client.remove_dir_all(dir_path.as_path()).is_ok());
    finalize_client(client);
}

#[test]
fn should_not_remove_dir_all() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    assert!(
        client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_remove_dir() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let mut dir_path = client.pwd().ok().unwrap();
    dir_path.push(Path::new("test/"));
    assert!(
        client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok()
    );
    assert!(client.remove_dir(dir_path.as_path()).is_ok());
    finalize_client(client);
}

#[test]
fn should_not_remove_dir() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let mut dir_path = client.pwd().ok().unwrap();
    dir_path.push(Path::new("test/"));
    assert!(
        client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok()
    );
    let mut file_path = dir_path.clone();
    file_path.push(Path::new("a.txt"));
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(
        client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
            .is_ok()
    );
    assert!(client.remove_dir(dir_path.as_path()).is_err());
    finalize_client(client);
}

#[test]
fn should_remove_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.txt");
    let file_data = "test data\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    assert!(client.remove_file(p).is_ok());
    finalize_client(client);
}

#[test]
fn should_setstat_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.sh");
    let file_data = "echo 5\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());

    assert!(
        client
            .setstat(
                p,
                Metadata {
                    accessed: Some(SystemTime::UNIX_EPOCH),
                    created: None,
                    file_type: FileType::File,
                    gid: Some(1000),
                    mode: Some(UnixPex::from(0o755)),
                    modified: Some(SystemTime::UNIX_EPOCH),
                    size: 7,
                    symlink: None,
                    uid: Some(1000),
                }
            )
            .is_ok()
    );
    let entry = client.stat(p).ok().unwrap();
    let stat = entry.metadata();
    // SCP stat comes from ls parsing; access time may not be available
    assert_eq!(stat.created, None);
    assert_eq!(stat.modified, Some(SystemTime::UNIX_EPOCH));
    assert_eq!(stat.mode.unwrap(), UnixPex::from(0o755));
    assert_eq!(stat.size, 7);

    finalize_client(client);
}

#[test]
fn should_not_setstat_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("bbbbb/cccc/a.sh");
    assert!(
        client
            .setstat(
                p,
                Metadata {
                    accessed: None,
                    created: None,
                    file_type: FileType::File,
                    gid: Some(1),
                    mode: Some(UnixPex::from(0o755)),
                    modified: None,
                    size: 7,
                    symlink: None,
                    uid: Some(1),
                }
            )
            .is_err()
    );
    finalize_client(client);
}

#[test]
fn should_stat_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.sh");
    let file_data = "echo 5\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert_eq!(
        client
            .create_file(p, &metadata, Box::new(reader))
            .ok()
            .unwrap(),
        7
    );
    let entry = client.stat(p).ok().unwrap();
    assert_eq!(entry.name(), "a.sh");
    let mut expected_path = client.pwd().ok().unwrap();
    expected_path.push("a.sh");
    assert_eq!(entry.path(), expected_path.as_path());
    let meta = entry.metadata();
    assert_eq!(meta.mode.unwrap(), UnixPex::from(0o644));
    assert_eq!(meta.size, 7);
    finalize_client(client);
}

#[test]
fn should_not_stat_file() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.sh");
    assert!(client.stat(p).is_err());
    finalize_client(client);
}

#[test]
fn should_make_symlink() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.sh");
    let file_data = "echo 5\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    let symlink = Path::new("b.sh");
    assert!(client.symlink(symlink, p).is_ok());
    assert!(client.remove_file(symlink).is_ok());
    finalize_client(client);
}

#[test]
fn should_not_make_symlink() {
    crate::mock::logger();
    let TestCtx {
        mut client,
        container: _container,
    } = setup_client();
    let p = Path::new("a.sh");
    let file_data = "echo 5\n";
    let reader = Cursor::new(file_data.as_bytes());
    let mut metadata = Metadata::default();
    metadata.size = file_data.len() as u64;
    assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
    let symlink = Path::new("b.sh");
    let file_data = "echo 5\n";
    let reader = Cursor::new(file_data.as_bytes());
    assert!(
        client
            .create_file(symlink, &metadata, Box::new(reader))
            .is_ok()
    );
    assert!(client.symlink(symlink, p).is_err());
    assert!(client.remove_file(symlink).is_ok());
    assert!(client.symlink(symlink, Path::new("c.sh")).is_err());
    finalize_client(client);
}

// -- test utils

struct TestCtx {
    client: ScpFs<super::super::super::backend::RusshSession<NoCheckServerKey>>,
    #[allow(dead_code)]
    container: OpensshServer,
}

fn setup_client() -> TestCtx {
    let container = OpensshServer::start();
    let port = container.port();

    use crate::SshAgentIdentity;

    let runtime = test_runtime();
    let config_file = ssh_mock::create_ssh_config(port);
    let mut client = ScpFs::russh(
        SshOpts::new("scp")
            .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()))
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .ssh_agent_identity(Some(SshAgentIdentity::All)),
        runtime,
    );
    assert!(client.connect().is_ok());
    let tempdir = PathBuf::from(generate_tempdir());
    assert!(
        client
            .create_dir(tempdir.as_path(), UnixPex::from(0o775))
            .is_ok()
    );
    assert!(client.change_dir(tempdir.as_path()).is_ok());

    TestCtx { client, container }
}

fn finalize_client(
    mut client: ScpFs<super::super::super::backend::RusshSession<NoCheckServerKey>>,
) {
    let wrkdir = client.pwd().ok().unwrap();
    assert!(client.remove_dir_all(wrkdir.as_path()).is_ok());
    assert!(client.disconnect().is_ok());
}

fn generate_tempdir() -> String {
    use rand::distr::Alphanumeric;
    use rand::{Rng, rng};
    let mut rng = rng();
    let name: String = std::iter::repeat(())
        .map(|()| rng.sample(Alphanumeric))
        .map(char::from)
        .take(8)
        .collect();
    format!("/tmp/temp_{name}")
}
