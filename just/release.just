# Set the shared version of every workspace crate and refresh Cargo.lock.
[group('release')]
bump_version version:
  dist/release/bump_version.sh {{ version }}
  cargo update --workspace

# Run the test suites of the release and install scripts.
[group('release')]
test_scripts:
  dist/release/test_bump_version.sh
  .github/scripts/test_changed_crates.sh
  dist/install/test_install_version.sh
