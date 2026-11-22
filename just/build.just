# Build every target in debug mode.
[group('build')]
build args="":
  cargo build --all-targets {{ args }}

# Build every target in release mode.
[group('build')]
release args="":
  cargo build --all-targets --release {{ args }}

# Remove Cargo build artifacts.
[group('build')]
[confirm("Are you sure you want to clean the build artifacts?")]
clean:
  cargo clean

# Build a static musl Linux release binary of fusibile in a pinned Alpine container.
[group('build')]
build_musl target:
  dist/release/build_musl.sh {{ target }}

# Build a release binary of fusibile for a target triple.
[group('build')]
build_release target features="":
  cargo build --locked --release --target {{ target }} -p fusibile {{ features }}
