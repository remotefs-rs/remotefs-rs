#[cfg(feature = "aws-s3")]
mod aws_s3;
#[cfg(feature = "ftp")]
mod ftp;
#[cfg(feature = "gcs")]
mod gcs;
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

use clap::{Parser, Subcommand};
use remotefs::AsyncRemoteFs;
use remotefs::adapters::r#async::Unblock;
use remotefs_fuse::MountOption;

#[cfg(feature = "aws-s3")]
use self::aws_s3::AwsS3Args;
#[cfg(feature = "ftp")]
use self::ftp::FtpArgs;
#[cfg(feature = "gcs")]
use self::gcs::GcsArgs;
#[cfg(feature = "kube")]
use self::kube::KubeArgs;
use self::memory::MemoryArgs;
#[cfg(feature = "smb")]
use self::smb::SmbArgs;
#[cfg(feature = "ssh")]
use self::ssh::SshArgs;
#[cfg(feature = "ssh")]
use self::ssh::{ScpClient, SftpClient};
#[cfg(feature = "webdav")]
use self::webdav::WebdavArgs;

/// RemoteFS FUSE CLI
///
/// CLI tool to mount a remote filesystem using FUSE.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct CliArgs {
    /// path where the remote filesystem will be mounted to
    #[arg(long)]
    pub to: PathBuf,
    /// name of mounted filesystem volume
    #[cfg(unix)]
    #[arg(long)]
    pub volume: String,
    /// uid to use for the mounted filesystem
    #[cfg(unix)]
    #[arg(long)]
    pub uid: Option<u32>,
    /// gid to use for the mounted filesystem
    #[arg(long)]
    #[cfg(unix)]
    pub gid: Option<u32>,
    /// default file permissions for those remote file protocols that don't support file permissions.
    ///
    /// this is a 3-digit octal number, e.g. 644
    #[arg(long, value_parser = from_octal)]
    #[cfg(unix)]
    pub default_mode: Option<u32>,
    /// mount options
    ///
    /// Mount options are specific to the underlying filesystem and are passed as key=value pairs.
    #[arg(short = 'o', long = "option")]
    pub option: Vec<MountOption>,
    /// log verbosity level
    #[arg(short = 'l', long, default_value_t = log::LevelFilter::Info)]
    log_level: log::LevelFilter,
    #[command(subcommand)]
    remote: RemoteArgs,
}

#[cfg(unix)]
fn from_octal(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s, 8).map_err(|_| "Invalid octal number".to_string())
}

impl CliArgs {
    pub fn init_logger(&self) {
        env_logger::builder().filter_level(self.log_level).init();
    }
}

#[derive(Subcommand, Debug)]
pub enum RemoteArgs {
    #[cfg(feature = "aws-s3")]
    #[command(name = "aws-s3")]
    AwsS3(AwsS3Args),
    #[cfg(feature = "ftp")]
    Ftp(FtpArgs),
    #[cfg(feature = "kube")]
    Kube(KubeArgs),
    #[cfg(feature = "gcs")]
    Gcs(GcsArgs),
    Memory(MemoryArgs),
    #[cfg(feature = "ssh")]
    Scp(SshArgs),
    #[cfg(feature = "ssh")]
    Sftp(SshArgs),
    #[cfg(feature = "smb")]
    Smb(SmbArgs),
    #[cfg(feature = "webdav")]
    Webdav(WebdavArgs),
}

impl CliArgs {
    /// Build the remote filesystem client selected on the command line.
    ///
    /// Blocking-only backends (in-memory, SMB) are adapted with
    /// `remotefs::adapters::r#async::Unblock` so every backend is driven the
    /// same way. This function must be called from inside the Tokio runtime.
    pub fn remote(self) -> anyhow::Result<Box<dyn AsyncRemoteFs>> {
        Ok(match self.remote {
            #[cfg(feature = "aws-s3")]
            RemoteArgs::AwsS3(args) => Box::new(remotefs_aws_s3::AwsS3Fs::from(args)),
            #[cfg(feature = "ftp")]
            RemoteArgs::Ftp(args) => Box::new(remotefs_ftp::TokioFtpFs::from(args)),
            #[cfg(feature = "gcs")]
            RemoteArgs::Gcs(args) => Box::new(remotefs_gcs::GoogleCloudStorageFs::try_from(args)?),
            #[cfg(feature = "kube")]
            RemoteArgs::Kube(args) => Box::new(remotefs_kube::KubeMultiPodFs::from(args)),
            RemoteArgs::Memory(args) => {
                Box::new(Unblock::new(remotefs_memory::MemoryFs::from(args)))
            }
            #[cfg(feature = "ssh")]
            RemoteArgs::Scp(args) => Box::new(ScpClient::try_from(args)?),
            #[cfg(feature = "ssh")]
            RemoteArgs::Sftp(args) => Box::new(SftpClient::try_from(args)?),
            #[cfg(all(feature = "smb", target_family = "unix"))]
            RemoteArgs::Smb(args) => {
                Box::new(Unblock::new(remotefs_smb::PavaoSmbFs::try_from(args)?))
            }
            #[cfg(all(feature = "smb", target_family = "windows"))]
            RemoteArgs::Smb(args) => Box::new(Unblock::new(remotefs_smb::WNetSmbFs::from(args))),
            #[cfg(feature = "webdav")]
            RemoteArgs::Webdav(args) => Box::new(remotefs_webdav::WebDAVFs::try_from(args)?),
        })
    }
}
