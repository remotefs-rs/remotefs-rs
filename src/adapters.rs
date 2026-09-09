//! Runtime adapters for bridging Tokio and remotefs contracts.

#[cfg(feature = "tokio")]
pub mod r#async;
#[cfg(feature = "tokio")]
pub mod blocking;

#[cfg(test)]
mod tests;
