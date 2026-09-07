//! ## Mock
//!
//! Contains mock for test units

use crate::RemoteFs;

// -- mock
pub struct MockRemoteFs;

impl RemoteFs for MockRemoteFs {
    #[allow(unused)]
    fn connect(&mut self) -> crate::RemoteResult<crate::fs::Welcome> {
        Ok(crate::fs::Welcome::default())
    }

    #[allow(unused)]
    fn disconnect(&mut self) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn is_connected(&mut self) -> bool {
        true
    }

    #[allow(unused)]
    fn pwd(&mut self) -> crate::RemoteResult<std::path::PathBuf> {
        Ok(std::path::PathBuf::from("/"))
    }

    #[allow(unused)]
    fn change_dir(&mut self, dir: &std::path::Path) -> crate::RemoteResult<std::path::PathBuf> {
        Ok(dir.to_path_buf())
    }

    #[allow(unused)]
    fn list_dir(&mut self, path: &std::path::Path) -> crate::RemoteResult<Vec<crate::File>> {
        Ok(vec![])
    }

    #[allow(unused)]
    fn stat(&mut self, path: &std::path::Path) -> crate::RemoteResult<crate::File> {
        Ok(crate::File {
            path: std::path::PathBuf::from("/foo"),
            metadata: crate::fs::Metadata::default(),
        })
    }

    #[allow(unused)]
    fn setstat(
        &mut self,
        path: &std::path::Path,
        metadata: crate::fs::Metadata,
    ) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn exists(&mut self, path: &std::path::Path) -> crate::RemoteResult<bool> {
        Ok(true)
    }

    #[allow(unused)]
    fn remove_file(&mut self, path: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn remove_dir(&mut self, path: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn create_dir(
        &mut self,
        path: &std::path::Path,
        mode: crate::fs::UnixPex,
    ) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn symlink(
        &mut self,
        path: &std::path::Path,
        target: &std::path::Path,
    ) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn copy(&mut self, src: &std::path::Path, dest: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn mov(&mut self, src: &std::path::Path, dest: &std::path::Path) -> crate::RemoteResult<()> {
        Ok(())
    }

    #[allow(unused)]
    fn exec(&mut self, cmd: &str) -> crate::RemoteResult<(u32, String)> {
        Ok((0, String::default()))
    }

    #[allow(unused)]
    fn append(
        &mut self,
        path: &std::path::Path,
        metadata: &crate::fs::Metadata,
    ) -> crate::RemoteResult<crate::fs::WriteStream> {
        Err(crate::RemoteError::new(
            crate::RemoteErrorType::UnsupportedFeature,
        ))
    }

    #[allow(unused)]
    fn create(
        &mut self,
        path: &std::path::Path,
        metadata: &crate::fs::Metadata,
    ) -> crate::RemoteResult<crate::fs::WriteStream> {
        Err(crate::RemoteError::new(
            crate::RemoteErrorType::UnsupportedFeature,
        ))
    }

    #[allow(unused)]
    fn open(&mut self, path: &std::path::Path) -> crate::RemoteResult<crate::fs::ReadStream> {
        Err(crate::RemoteError::new(
            crate::RemoteErrorType::UnsupportedFeature,
        ))
    }
}
