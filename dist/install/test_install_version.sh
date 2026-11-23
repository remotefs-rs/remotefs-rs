#!/usr/bin/env bash
# Tests for latest_fusibile_version in install.sh. The installer is sourced with
# FUSIBILE_INSTALL_SOURCE_ONLY=1 so that only its functions are defined.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
INSTALLER="$SCRIPT_DIR/install.sh"
FAILURES=0

fail() {
    echo "FAIL: $*" >&2
    FAILURES=$((FAILURES + 1))
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# A releases list where the older fusibile and crate tags sit next to the release tags.
cat > "$WORK/mixed.json" <<'JSON'
[
  {
    "tag_name": "remotefs-ssh-v1.3.0"
  },
  {
    "tag_name": "v1.2.0"
  },
  {
    "tag_name": "fusibile-v1.1.0"
  },
  {
    "tag_name": "v1.0.1"
  }
]
JSON

# A releases list with no v* release at all.
cat > "$WORK/no-fusibile.json" <<'JSON'
[
  {
    "tag_name": "remotefs-ssh-v1.3.0"
  },
  {
    "tag_name": "fusibile-v1.1.0"
  }
]
JSON

run_latest() {
    (
        # shellcheck disable=SC2034 # read by the sourced installer
        FUSIBILE_INSTALL_SOURCE_ONLY=1
        # shellcheck source=/dev/null
        . "$INSTALLER"
        latest_fusibile_version "$1"
    )
}

# -- picks the newest v* tag and ignores the other tags ----------------------
if ! version="$(run_latest "$WORK/mixed.json")"; then
    fail "latest_fusibile_version failed on a mixed releases list"
elif [ "$version" != "1.2.0" ]; then
    fail "expected 1.2.0, got '$version'"
fi

# -- fails when no v* release exists -----------------------------------------
if run_latest "$WORK/no-fusibile.json" > /dev/null 2>&1; then
    fail "expected failure when no v* tag exists"
fi

if [ "$FAILURES" -eq 0 ]; then
    echo "OK: all install version tests passed"
else
    echo "$FAILURES test(s) failed" >&2
    exit 1
fi
