#[cfg(feature = "aws-s3")]
mod aws_s3;
#[cfg(feature = "ftp")]
mod ftp;
#[cfg(feature = "kube")]
mod kube;
mod memory;
#[cfg(feature = "smb")]
mod smb;
#[cfg(feature = "ssh")]
mod ssh;
#[cfg(feature = "webdav")]
mod webdav;

use std::path::PathBuf;

use argh::FromArgs;
use remotefs::{fs::UnixPex, RemoteFs, RemoteResult};

#[cfg(feature = "aws-s3")]
use self::aws_s3::AwsS3Args;
#[cfg(feature = "ftp")]
use self::ftp::FtpArgs;
#[cfg(feature = "kube")]
use self::kube::KubeArgs;
use self::memory::MemoryArgs;
#[cfg(feature = "smb")]
use self::smb::SmbArgs;
#[cfg(feature = "ssh")]
use self::ssh::{ScpArgs, SftpArgs};
#[cfg(feature = "webdav")]
use self::webdav::WebdavArgs;

/// RemoteFS FUSE CLI
///
/// CLI tool to mount a remote filesystem using FUSE.
#[derive(FromArgs, Debug)]
pub struct CliArgs {
    /// path where the remote filesystem will be mounted to
    #[argh(option)]
    pub to: PathBuf,
    /// name of mounted filesystem volume
    #[argh(option)]
    pub volume: String,
    /// uid to use for the mounted filesystem
    #[argh(option)]
    pub uid: Option<u32>,
    /// gid to use for the mounted filesystem
    #[argh(option)]
    pub gid: Option<u32>,
    /// default file permissions for those remote file protocols that don't support file permissions.
    ///
    /// this is a 3-digit octal number, e.g. 644
    #[argh(option, from_str_fn(from_octal))]
    pub default_mode: Option<u32>,
    /// enable verbose logging.
    ///
    /// use multiple times to increase verbosity
    #[argh(option, short = 'l', default = r#""info".to_string()"#)]
    log_level: String,
    #[argh(subcommand)]
    remote: RemoteArgs,
}

fn from_octal(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s, 8).map_err(|_| "Invalid octal number".to_string())
}

impl CliArgs {
    pub fn init_logger(&self) -> anyhow::Result<()> {
        match self.log_level.as_str() {
            "error" => env_logger::builder()
                .filter_level(log::LevelFilter::Error)
                .init(),
            "warn" => env_logger::builder()
                .filter_level(log::LevelFilter::Warn)
                .init(),
            "info" => env_logger::builder()
                .filter_level(log::LevelFilter::Info)
                .init(),
            "debug" => env_logger::builder()
                .filter_level(log::LevelFilter::Debug)
                .init(),
            "trace" => env_logger::builder()
                .filter_level(log::LevelFilter::Trace)
                .init(),
            _ => anyhow::bail!("Invalid log level: {}", self.log_level),
        }

        Ok(())
    }
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
pub enum RemoteArgs {
    #[cfg(feature = "aws-s3")]
    AwsS3(AwsS3Args),
    #[cfg(feature = "ftp")]
    Ftp(FtpArgs),
    #[cfg(feature = "kube")]
    Kube(KubeArgs),
    Memory(MemoryArgs),
    #[cfg(feature = "ssh")]
    Scp(ScpArgs),
    #[cfg(feature = "ssh")]
    Sftp(SftpArgs),
    #[cfg(feature = "smb")]
    Smb(SmbArgs),
    #[cfg(feature = "webdav")]
    Webdav(WebdavArgs),
}

impl CliArgs {
    /// Create a RemoteFs instance from the CLI arguments
    pub fn remote(self) -> RemoteFsWrapper {
        match self.remote {
            #[cfg(feature = "aws-s3")]
            RemoteArgs::AwsS3(args) => RemoteFsWrapper::Aws(remotefs_aws_s3::AwsS3Fs::from(args)),
            #[cfg(feature = "ftp")]
            RemoteArgs::Ftp(args) => RemoteFsWrapper::Ftp(remotefs_ftp::FtpFs::from(args)),
            #[cfg(feature = "kube")]
            RemoteArgs::Kube(args) => {
                RemoteFsWrapper::Kube(remotefs_kube::KubeMultiPodFs::from(args))
            }
            RemoteArgs::Memory(args) => {
                RemoteFsWrapper::Memory(remotefs_memory::MemoryFs::from(args))
            }
            #[cfg(feature = "ssh")]
            RemoteArgs::Scp(args) => RemoteFsWrapper::Scp(remotefs_ssh::ScpFs::from(args)),
            #[cfg(feature = "ssh")]
            RemoteArgs::Sftp(args) => RemoteFsWrapper::Sftp(remotefs_ssh::SftpFs::from(args)),
            #[cfg(feature = "smb")]
            RemoteArgs::Smb(args) => RemoteFsWrapper::Smb(remotefs_smb::SmbFs::from(args)),
            #[cfg(feature = "webdav")]
            RemoteArgs::Webdav(args) => {
                RemoteFsWrapper::Webdav(remotefs_webdav::WebDAVFs::from(args))
            }
        }
    }
}

enum RemoteFsWrapper {
    #[cfg(feature = "aws-s3")]
    Aws(remotefs_aws_s3::AwsS3Fs),
    #[cfg(feature = "ftp")]
    Ftp(remotefs_ftp::FtpFs),
    #[cfg(feature = "kube")]
    Kube(remotefs_kube::KubeMultiPodFs),
    Memory(remotefs_memory::MemoryFs),
    #[cfg(feature = "ssh")]
    Scp(remotefs_ssh::ScpFs),
    #[cfg(feature = "ssh")]
    Sftp(remotefs_ssh::SftpFs),
    #[cfg(feature = "smb")]
    Smb(remotefs_smb::SmbFs),
    #[cfg(feature = "webdav")]
    Webdav(remotefs_webdav::WebDAVFs),
}

impl RemoteFsWrapper {
    fn on_remote<F, T>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut dyn RemoteFs) -> T,
    {
        match self {
            #[cfg(feature = "aws-s3")]
            RemoteFsWrapper::Aws(fs) => f(fs),
            #[cfg(feature = "ftp")]
            RemoteFsWrapper::Ftp(fs) => f(fs),
            #[cfg(feature = "kube")]
            RemoteFsWrapper::Kube(fs) => f(fs),
            RemoteFsWrapper::Memory(fs) => f(fs),
            #[cfg(feature = "ssh")]
            RemoteFsWrapper::Scp(fs) => f(fs),
            #[cfg(feature = "ssh")]
            RemoteFsWrapper::Sftp(fs) => f(fs),
            #[cfg(feature = "smb")]
            RemoteFsWrapper::Smb(fs) => f(fs),
            #[cfg(feature = "webdav")]
            RemoteFsWrapper::Webdav(fs) => f(fs),
        }
    }
}

impl RemoteFs for RemoteFsWrapper {
    fn append(
        &mut self,
        path: &std::path::Path,
        metadata: &remotefs::fs::Metadata,
    ) -> RemoteResult<remotefs::fs::WriteStream> {
        self.on_remote(|fs| fs.append(path, metadata))
    }
    fn append_file(
        &mut self,
        path: &std::path::Path,
        metadata: &remotefs::fs::Metadata,
        reader: Box<dyn std::io::Read + Send>,
    ) -> RemoteResult<u64> {
        self.on_remote(|fs| fs.append_file(path, metadata, reader))
    }
    fn create_dir(&mut self, path: &std::path::Path, mode: UnixPex) -> RemoteResult<()> {
        self.on_remote(|fs| fs.create_dir(path, mode))
    }
    fn change_dir(&mut self, dir: &std::path::Path) -> RemoteResult<PathBuf> {
        self.on_remote(|fs| fs.change_dir(dir))
    }
    fn connect(&mut self) -> RemoteResult<remotefs::fs::Welcome> {
        self.on_remote(|fs| fs.connect())
    }
    fn copy(&mut self, src: &std::path::Path, dest: &std::path::Path) -> RemoteResult<()> {
        self.on_remote(|fs| fs.copy(src, dest))
    }
    fn create(
        &mut self,
        path: &std::path::Path,
        metadata: &remotefs::fs::Metadata,
    ) -> RemoteResult<remotefs::fs::WriteStream> {
        self.on_remote(|fs| fs.create(path, metadata))
    }
    fn create_file(
        &mut self,
        path: &std::path::Path,
        metadata: &remotefs::fs::Metadata,
        reader: Box<dyn std::io::Read + Send>,
    ) -> RemoteResult<u64> {
        self.on_remote(|fs| fs.create_file(path, metadata, reader))
    }
    fn disconnect(&mut self) -> RemoteResult<()> {
        self.on_remote(|fs| fs.disconnect())
    }
    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)> {
        self.on_remote(|fs| fs.exec(cmd))
    }
    fn exists(&mut self, path: &std::path::Path) -> RemoteResult<bool> {
        self.on_remote(|fs| fs.exists(path))
    }
    fn find(&mut self, search: &str) -> RemoteResult<Vec<remotefs::File>> {
        self.on_remote(|fs| fs.find(search))
    }
    fn is_connected(&mut self) -> bool {
        self.on_remote(|fs| fs.is_connected())
    }

    fn list_dir(&mut self, path: &std::path::Path) -> RemoteResult<Vec<remotefs::File>> {
        self.on_remote(|fs| fs.list_dir(path))
    }
    fn mov(&mut self, src: &std::path::Path, dest: &std::path::Path) -> RemoteResult<()> {
        self.on_remote(|fs| fs.mov(src, dest))
    }
    fn on_read(&mut self, readable: remotefs::fs::ReadStream) -> RemoteResult<()> {
        self.on_remote(|fs| fs.on_read(readable))
    }
    fn on_written(&mut self, writable: remotefs::fs::WriteStream) -> RemoteResult<()> {
        self.on_remote(|fs| fs.on_written(writable))
    }
    fn open(&mut self, path: &std::path::Path) -> RemoteResult<remotefs::fs::ReadStream> {
        self.on_remote(|fs| fs.open(path))
    }
    fn open_file(
        &mut self,
        src: &std::path::Path,
        dest: Box<dyn std::io::Write + Send>,
    ) -> RemoteResult<u64> {
        self.on_remote(|fs| fs.open_file(src, dest))
    }
    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        self.on_remote(|fs| fs.pwd())
    }
    fn remove_dir(&mut self, path: &std::path::Path) -> RemoteResult<()> {
        self.on_remote(|fs| fs.remove_dir(path))
    }
    fn remove_dir_all(&mut self, path: &std::path::Path) -> RemoteResult<()> {
        self.on_remote(|fs| fs.remove_dir_all(path))
    }
    fn remove_file(&mut self, path: &std::path::Path) -> RemoteResult<()> {
        self.on_remote(|fs| fs.remove_file(path))
    }
    fn setstat(
        &mut self,
        path: &std::path::Path,
        metadata: remotefs::fs::Metadata,
    ) -> RemoteResult<()> {
        self.on_remote(|fs| fs.setstat(path, metadata))
    }
    fn stat(&mut self, path: &std::path::Path) -> RemoteResult<remotefs::File> {
        self.on_remote(|fs| fs.stat(path))
    }
    fn symlink(&mut self, path: &std::path::Path, target: &std::path::Path) -> RemoteResult<()> {
        self.on_remote(|fs| fs.symlink(path, target))
    }
}
