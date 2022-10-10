//! ## Mock
//!
//! Contains mock for test units



// -- logger

pub fn logger() {
    let _ = env_logger::builder().is_test(true).try_init();
}
