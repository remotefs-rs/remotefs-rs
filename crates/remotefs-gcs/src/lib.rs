//! Reusable functionality for the Rust project template.

/// Returns the starter greeting.
///
/// # Examples
///
/// ```
/// assert_eq!(rust_template::greeting(), "Hello from rust-template!");
/// ```
#[must_use]
pub const fn greeting() -> &'static str {
    "Hello from rust-template!"
}

#[cfg(test)]
mod tests {
    #[test]
    fn greeting_identifies_template() {
        assert_eq!(super::greeting(), "Hello from rust-template!");
    }
}
