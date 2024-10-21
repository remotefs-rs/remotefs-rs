use thiserror::Error;

pub type DriverResult<T> = Result<T, DriverError>;

#[derive(Debug, Error)]
pub enum DriverError {}
