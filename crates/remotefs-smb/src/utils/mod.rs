//! ## Utils
//!
//! `utils` is the module which provides utilities of different kind

pub mod path;
#[cfg(all(target_family = "unix", feature = "pavao"))]
pub mod smb;
