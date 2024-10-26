use std::path::PathBuf;

use remotefs::fs::UnixPex;
use remotefs::{RemoteFs, RemoteResult};

/// Wrapper around the different [`RemoteFs`] implementations
#[allow(clippy::large_enum_variant)]
pub enum RemoteFsWrapper {
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
    /// Call the given closure with the appropriate [`RemoteFs`] implementation
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
