//! Forwarding implementations for boxed filesystem trait objects.

use std::io::{Read, Write};
use std::path::Path;

use super::{
    Capabilities, ExecOutput, File, ReadOptions, ReadStream, RemoteFs, RemoteResult, SetMetadata,
    UnixPex, Welcome, WriteOptions, WriteStream,
};

impl<T: RemoteFs + ?Sized> RemoteFs for Box<T> {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        (**self).connect()
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        (**self).disconnect()
    }

    fn is_connected(&self) -> bool {
        (**self).is_connected()
    }

    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }

    fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        (**self).list_dir(path)
    }

    fn stat(&self, path: &Path) -> RemoteResult<File> {
        (**self).stat(path)
    }

    fn exists(&self, path: &Path) -> RemoteResult<bool> {
        (**self).exists(path)
    }

    fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        (**self).set_metadata(path, metadata)
    }

    fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        (**self).create_dir(path, mode)
    }

    fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        (**self).remove_file(path)
    }

    fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        (**self).remove_dir(path)
    }

    fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        (**self).remove_dir_all(path)
    }

    fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        (**self).rename(src, dest)
    }

    fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        (**self).copy(src, dest)
    }

    fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        (**self).symlink(path, target)
    }

    fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<ReadStream> {
        (**self).open(path, opts)
    }

    fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        (**self).create(path, opts)
    }

    fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<WriteStream> {
        (**self).append(path, opts)
    }

    fn read_file(
        &self,
        path: &Path,
        opts: &ReadOptions,
        dest: &mut (dyn Write + Send),
    ) -> RemoteResult<u64> {
        (**self).read_file(path, opts, dest)
    }

    fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        (**self).write_file(path, opts, src)
    }

    fn append_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn Read + Send),
    ) -> RemoteResult<u64> {
        (**self).append_file(path, opts, src)
    }

    fn exec(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        (**self).exec(cmd)
    }
}
