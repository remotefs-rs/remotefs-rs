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
use self::ssh::{ScpArgs, SftpArgs};
#[cfg(feature = "webdav")]
use self::webdav::WebdavArgs;
use crate::remotefs_wrapper::RemoteFsWrapper;

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
    pub fn remote(self) -> anyhow::Result<RemoteFsWrapper> {
        match self.remote {
            #[cfg(feature = "aws-s3")]
            RemoteArgs::AwsS3(args) => {
                Ok(RemoteFsWrapper::Aws(remotefs_aws_s3::AwsS3Fs::from(args)))
            }
            #[cfg(feature = "ftp")]
            RemoteArgs::Ftp(args) => Ok(RemoteFsWrapper::Ftp(remotefs_ftp::FtpFs::from(args))),
            #[cfg(feature = "gcs")]
            RemoteArgs::Gcs(args) => Ok(RemoteFsWrapper::Gcs(
                remotefs_gcs::GoogleCloudStorageFs::try_from(args)?,
            )),
            #[cfg(feature = "kube")]
            RemoteArgs::Kube(args) => Ok(RemoteFsWrapper::Kube(
                remotefs_kube::KubeMultiPodFs::from(args),
            )),
            RemoteArgs::Memory(args) => Ok(RemoteFsWrapper::Memory(
                remotefs_memory::MemoryFs::from(args),
            )),
            #[cfg(feature = "ssh")]
            RemoteArgs::Scp(args) => Ok(RemoteFsWrapper::Scp(remotefs_ssh::ScpFs::try_from(args)?)),
            #[cfg(feature = "ssh")]
            RemoteArgs::Sftp(args) => {
                Ok(RemoteFsWrapper::Sftp(remotefs_ssh::SftpFs::try_from(args)?))
            }
            #[cfg(all(feature = "smb", target_family = "unix"))]
            RemoteArgs::Smb(args) => Ok(RemoteFsWrapper::Smb(remotefs_smb::PavaoSmbFs::try_from(
                args,
            )?)),
            #[cfg(all(feature = "smb", target_family = "windows"))]
            RemoteArgs::Smb(args) => Ok(RemoteFsWrapper::Smb(remotefs_smb::WNetSmbFs::from(args))),
            #[cfg(feature = "webdav")]
            RemoteArgs::Webdav(args) => Ok(RemoteFsWrapper::Webdav(
                remotefs_webdav::WebDAVFs::from(args),
            )),
        }
    }
}
