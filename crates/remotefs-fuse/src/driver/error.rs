use thiserror::Error;

pub type DriverResult<T> = Result<T, DriverError>;

#[derive(Debug, Error)]
pub enum DriverError {
    #[cfg(target_family = "unix")]
    #[error("Inode DB error: {0}")]
    Inode(#[from] super::unix::InodeDbError),
}
