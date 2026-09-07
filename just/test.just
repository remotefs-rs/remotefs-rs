# Run all unit, integration, and documentation tests.
[group('test')]
test args="":
  cargo test --lib {{ args }}
  cargo test --doc {{ args }}

# Generate an LCOV coverage report.
[group('test')]
coverage output="lcov.info" args="":
  cargo llvm-cov --lib --lcov --output-path {{ output }} {{ args }}
