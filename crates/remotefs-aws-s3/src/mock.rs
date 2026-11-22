//! ## Mock
//!
//! Contains mock support for test units.

#[cfg(any(feature = "with-containers", feature = "with-s3-ci"))]
pub mod container;

/// Initialize the test logger once.
pub fn logger() {
    use std::sync::Once;

    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .format_line_number(true)
            .try_init();
    });
}
