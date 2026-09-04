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
    mkdir -p "$root"
    cat > "$root/Cargo.toml" <<'FIXTURE'
[workspace]
members = ["crates/fusibile", "crates/remotefs-fuse"]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
remotefs = "0.3"
remotefs-fuse = { version = "0.1", path = "crates/remotefs-fuse" }
remotefs-smb = { version = "0.5", default-features = false }
FIXTURE
    cat > "$root/README.md" <<'FIXTURE'
```toml
remotefs-fuse = "0.1"
```
FIXTURE
}

# -- valid bump --------------------------------------------------------------
ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT
make_fixture "$ROOT"
"$BUMP" 1.2.3 "$ROOT" > /dev/null

assert_contains "$ROOT/Cargo.toml" 'version = "1.2.3"'
assert_contains "$ROOT/Cargo.toml" 'remotefs-fuse = { version = "1.2.3", path = "crates/remotefs-fuse" }'
assert_contains "$ROOT/README.md"  'remotefs-fuse = "1.2"'
# unrelated dependency versions must be untouched
assert_contains "$ROOT/Cargo.toml" 'remotefs = "0.3"'
assert_contains "$ROOT/Cargo.toml" 'remotefs-smb = { version = "0.5", default-features = false }'
assert_absent   "$ROOT/Cargo.toml" 'version = "0.1.0"'

# -- idempotency -------------------------------------------------------------
"$BUMP" 1.2.3 "$ROOT" > /dev/null
assert_contains "$ROOT/Cargo.toml" 'version = "1.2.3"'

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
