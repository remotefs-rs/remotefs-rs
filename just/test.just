# Run all unit, integration, and documentation tests.
[group('test')]
test args="":
  cargo test --lib {{ args }}
  cargo test --doc {{ args }}

# Test every supported feature combination.
[group('test')]
test_features args="":
  just test "--no-default-features {{ args }}"
  just test "{{ args }}"
  just test "--no-default-features --features async {{ args }}"
  just test "--features async {{ args }}"
  just test "--no-default-features --features tokio {{ args }}"
  just test "--features tokio {{ args }}"
  just test "--all-features {{ args }}"

# Generate an LCOV coverage report.
[group('test')]
coverage output="lcov.info" args="":
  cargo llvm-cov --lib --lcov --output-path {{ output }} {{ args }}
