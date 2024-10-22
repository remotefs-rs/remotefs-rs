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
    #[argh(subcommand)]
    remote: RemoteArgs,
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
