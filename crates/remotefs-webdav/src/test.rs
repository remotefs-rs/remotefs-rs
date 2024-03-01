pub fn log_init() {
    let _ = env_logger::builder().is_test(true).try_init();
}
