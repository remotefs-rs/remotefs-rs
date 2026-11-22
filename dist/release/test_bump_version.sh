#!/usr/bin/env bash
# Tests for bump_version.sh. Builds a throwaway fixture tree, bumps it, asserts.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BUMP="$SCRIPT_DIR/bump_version.sh"
FAILURES=0

fail() {
    echo "FAIL: $*" >&2
    FAILURES=$((FAILURES + 1))
}

assert_contains() {
    local file="$1" needle="$2"
    if ! grep -qF -- "$needle" "$file"; then
        fail "expected '$needle' in $file, got:"
        cat "$file" >&2
    fi
}

assert_absent() {
    local file="$1" needle="$2"
    if grep -qF -- "$needle" "$file"; then
        fail "did not expect '$needle' in $file"
    fi
}

make_fixture() {
    local root="$1"
    mkdir -p "$root/crates/remotefs-ssh"
    cat > "$root/Cargo.toml" <<'FIXTURE'
[workspace]
members = ["crates/remotefs", "crates/remotefs-ssh"]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
remotefs = { version = "1.0.0", path = "crates/remotefs" }
remotefs-ssh = { version = "1.0.0", path = "crates/remotefs-ssh", default-features = false }
tokio = "1"
FIXTURE
    cat > "$root/README.md" <<'FIXTURE'
```toml
remotefs = "1.0"
```
FIXTURE
    cat > "$root/crates/remotefs-ssh/README.md" <<'FIXTURE'
```toml
remotefs-ssh = "1.0"
remotefs = { version = "1.0", features = ["async"] }
```
FIXTURE
}

# -- valid bump --------------------------------------------------------------
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
make_fixture "$ROOT"
"$BUMP" 1.2.3 "$ROOT" > /dev/null

assert_contains "$ROOT/Cargo.toml" 'version = "1.2.3"'
assert_contains "$ROOT/Cargo.toml" 'remotefs = { version = "1.2.3", path = "crates/remotefs" }'
assert_contains "$ROOT/Cargo.toml" 'remotefs-ssh = { version = "1.2.3", path = "crates/remotefs-ssh", default-features = false }'
assert_contains "$ROOT/README.md"  'remotefs = "1.2"'
assert_contains "$ROOT/crates/remotefs-ssh/README.md" 'remotefs-ssh = "1.2"'
assert_contains "$ROOT/crates/remotefs-ssh/README.md" 'remotefs = { version = "1.2", features = ["async"] }'
# third-party versions must be untouched
assert_contains "$ROOT/Cargo.toml" 'tokio = "1"'
assert_absent   "$ROOT/Cargo.toml" 'version = "0.1.0"'
assert_absent   "$ROOT/Cargo.toml" 'version = "1.0.0"'

# -- idempotency -------------------------------------------------------------
"$BUMP" 1.2.3 "$ROOT" > /dev/null
assert_contains "$ROOT/Cargo.toml" 'version = "1.2.3"'
assert_contains "$ROOT/crates/remotefs-ssh/README.md" 'remotefs-ssh = "1.2"'

# -- invalid versions are rejected -------------------------------------------
for bad in "v1.2.3" "1.2" "1.2.3-rc1" "01.2.3" "" "1.2.3.4"; do
    if "$BUMP" "$bad" "$ROOT" > /dev/null 2>&1; then
        fail "expected rejection of version '$bad'"
    fi
done

if [ "$FAILURES" -eq 0 ]; then
    echo "OK: all bump_version tests passed"
else
    echo "$FAILURES test(s) failed" >&2
    exit 1
fi
