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
use remotefs::RemoteFs;

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
    pub fn remote(self) -> Box<dyn RemoteFs> {
        match self.remote {
            #[cfg(feature = "aws-s3")]
            RemoteArgs::AwsS3(args) => Box::new(remotefs_aws_s3::AwsS3Fs::from(args)),
            #[cfg(feature = "ftp")]
            RemoteArgs::Ftp(args) => Box::new(remotefs_ftp::FtpFs::from(args)),
            #[cfg(feature = "kube")]
            RemoteArgs::Kube(args) => Box::new(remotefs_kube::KubeMultiPodFs::from(args)),
            RemoteArgs::Memory(args) => Box::new(remotefs_memory::MemoryFs::from(args)),
            #[cfg(feature = "ssh")]
            RemoteArgs::Scp(args) => Box::new(remotefs_ssh::ScpFs::from(args)),
            #[cfg(feature = "ssh")]
            RemoteArgs::Sftp(args) => Box::new(remotefs_ssh::SftpFs::from(args)),
            #[cfg(feature = "smb")]
            RemoteArgs::Smb(args) => Box::new(remotefs_smb::SmbFs::from(args)),
            #[cfg(feature = "webdav")]
            RemoteArgs::Webdav(args) => Box::new(remotefs_webdav::WebDAVFs::from(args)),
        }
    }
}
