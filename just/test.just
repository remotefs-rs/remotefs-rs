# Run unit and documentation tests for every workspace crate, one crate at a time.
#
# Crates run in separate cargo invocations because feature unification across the
# whole workspace would enable two rustls crypto providers at once (ring from
# remotefs-kube, aws-lc-rs from remotefs-gcs), and rustls refuses to pick one.
# Set SKIP_CRATES to a space separated list of crate names to leave them out, for
# example on platforms without FUSE.
[group('test')]
test args="":
  #!/usr/bin/env bash
  set -euo pipefail
  skip=" ${SKIP_CRATES:-} "
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | .name as $name
        | (if any(.targets[]; .kind | index("lib")) then "\($name) lib" else empty end),
          (if any(.targets[]; .kind | index("bin")) then "\($name) bin" else empty end)' \
    | while read -r crate kind; do
        case "$skip" in *" $crate "*) continue ;; esac
        case "$kind" in
          lib)
            cargo test -p "$crate" --lib {{ args }}
            cargo test -p "$crate" --doc {{ args }}
            ;;
          bin)
            cargo test -p "$crate" --bins {{ args }}
            ;;
        esac
      done

# Run unit and documentation tests for one crate (see `test`).
[group('test')]
test_crate crate args="":
  cargo test -p {{ crate }} --lib {{ args }}
  cargo test -p {{ crate }} --doc {{ args }}

# Test every crate with its default features (see `test`).
[group('test')]
test_features args="":
  just test "{{ args }}"

# Generate an LCOV coverage report, one crate at a time (see `test`).
[group('test')]
coverage output="lcov.info" args="":
  #!/usr/bin/env bash
  set -euo pipefail
  skip=" ${SKIP_CRATES:-} "
  cargo llvm-cov clean --workspace
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(any(.targets[]; .kind | index("lib"))) | .name' \
    | while read -r crate; do
        case "$skip" in *" $crate "*) continue ;; esac
        cargo llvm-cov --no-report --lib -p "$crate" {{ args }}
      done
  cargo llvm-cov report --lcov --output-path {{ output }}

# Run the unit tests that start Docker containers (remotefs-ftp and remotefs-ssh).
# They are ignored by default and need a running Docker daemon.
[group('test')]
test_containers args="":
  cargo test -p remotefs-ftp -p remotefs-ssh --lib --features with-containers {{ args }}
