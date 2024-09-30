//! ## Mock
//!
//! Contains mock for test units

// -- logger

#[allow(dead_code)]
pub fn logger() {
    let _ = env_logger::builder().is_test(true).try_init();
}
