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

# Every test suite that needs more than the default features, one per line, as
# `<crate>|<cargo arguments>|<test binary arguments>`. The `with-containers` and
# `integration-tests` features turn on the tests that need Docker, a FUSE mount, or
# a Minikube cluster. A crate may appear more than once, because each remotefs-ssh
# backend is a feature of its own. Tests that share an external server, or mount a
# FUSE filesystem (concurrent mounts fail at random with EPERM), run on one thread.
test_suites := '''
remotefs|--features tokio|
remotefs-aws-s3|--features tokio,with-containers|--test-threads=1
remotefs-ftp|--features rustls-aws-lc-rs,tokio-rustls-aws-lc-rs,with-containers|
remotefs-fuse|--features tokio,integration-tests|--test-threads=1
remotefs-gcs|--features tokio,with-containers|--test-threads=1
remotefs-kube|--features tokio,integration-tests|--test-threads=1
remotefs-memory||
remotefs-smb|--features with-containers|
remotefs-ssh|--no-default-features --features find,libssh2,with-containers|
remotefs-ssh|--no-default-features --features find,libssh,with-containers|
remotefs-ssh|--no-default-features --features find,russh,with-containers|
remotefs-webdav|--features tokio,with-containers|
fusibile||
'''

# Run every suite of `test_suites` with the given cargo command, such as `cargo test`,
# and list the suites that failed at the end. Crates run in separate cargo invocations
# for the reason given at `test`. Also fails when a test is ignored because it needs
# Docker: that means a suite lost its `with-containers` feature and would pass without
# testing anything.
[private]
suites runner args="":
  #!/usr/bin/env bash
  set -euo pipefail
  skip=" ${SKIP_CRATES:-} "
  log=$(mktemp)
  trap 'rm -f "$log"' EXIT
  failed=()
  while IFS='|' read -r crate cargo_args test_args; do
    [ -n "$crate" ] || continue
    case "$skip" in *" $crate "*) continue ;; esac
    echo "==> $crate $cargo_args"
    # shellcheck disable=SC2086
    if ! {{ runner }} --tests -p "$crate" $cargo_args {{ args }} -- $test_args </dev/null 2>&1 | tee -a "$log"; then
      failed+=("$crate $cargo_args")
    fi
  done <<'SUITES'
  {{ test_suites }}
  SUITES
  if [ "${#failed[@]}" -gt 0 ]; then
    printf 'error: suite failed: %s\n' "${failed[@]}" >&2
    exit 1
  fi
  if grep -q 'ignored, needs Docker' "$log"; then
    echo "error: some tests were ignored because they need Docker" >&2
    exit 1
  fi

# Run the unit and integration tests of every suite, including the ones that start
# Docker containers, a FUSE mount, or a Minikube cluster. Needs `containers_up`, a
# running Docker daemon, and MINIKUBE_IP for remotefs-kube. Leave crates out with
# SKIP_CRATES.
[group('test')]
test_containers args="":
  {{ just_executable() }} suites "cargo test" "{{ args }}"

# Start the SMB and WebDAV servers that their `with-containers` tests connect to.
[group('test')]
containers_up:
  docker compose -p remotefs-smb -f crates/remotefs-smb/tests/docker-compose.yml up -d
  docker compose -p remotefs-webdav -f crates/remotefs-webdav/tests/docker-compose.yml up -d

# Stop and remove the servers started by `containers_up`.
[group('test')]
containers_down:
  docker compose -p remotefs-smb -f crates/remotefs-smb/tests/docker-compose.yml down --volumes
  docker compose -p remotefs-webdav -f crates/remotefs-webdav/tests/docker-compose.yml down

# Generate an LCOV coverage report from every suite of `test_containers`, so it needs
# the same environment.
[group('test')]
coverage output="lcov.info" args="":
  cargo llvm-cov clean --workspace
  {{ just_executable() }} suites "cargo llvm-cov --no-report" "{{ args }}"
  cargo llvm-cov report --summary-only
  cargo llvm-cov report --lcov --output-path {{ output }}
