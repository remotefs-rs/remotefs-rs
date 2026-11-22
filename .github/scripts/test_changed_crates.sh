#!/usr/bin/env bash
# Tests for changed-crates.sh against a fixed cargo-metadata fixture.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SCRIPT="$SCRIPT_DIR/changed-crates.sh"
FIXTURE="$SCRIPT_DIR/fixtures/metadata.json"
MEMBER_COUNT=5

FAILURES=0

fail() {
  echo "FAIL: $1" >&2
  FAILURES=$((FAILURES + 1))
}

# run_case NAME EXPECTED_TRUE_LIST [PATH...]
# Pipes PATHs (one per line) into changed-crates.sh and compares the set of
# members printed as "=true" with EXPECTED_TRUE_LIST (space separated, may be
# empty). Every case must also print one line per member.
run_case() {
  local name=$1 expected=$2
  shift 2

  local actual lines actual_true expected_sorted
  if [ $# -gt 0 ]; then
    actual=$(printf '%s\n' "$@" | "$SCRIPT" "$FIXTURE")
  else
    actual=$("$SCRIPT" "$FIXTURE" < /dev/null)
  fi

  lines=$(printf '%s\n' "$actual" | grep -cE '=(true|false)$' || true)
  if [ "$lines" -ne "$MEMBER_COUNT" ]; then
    fail "$name: expected $MEMBER_COUNT member lines, got $lines"
    return
  fi

  actual_true=$(printf '%s\n' "$actual" | sed -n 's/=true$//p' | sort | tr '\n' ' ' | sed 's/ $//')
  expected_sorted=$(printf '%s\n' $expected | sort | tr '\n' ' ' | sed 's/ $//')
  if [ "$actual_true" != "$expected_sorted" ]; then
    fail "$name: expected [$expected_sorted], got [$actual_true]"
  fi
}

run_case "root docs only" "" README.md .github/ISSUE_TEMPLATE/bug_report.md
run_case "single leaf crate" "remotefs-ssh fusibile" crates/remotefs-ssh/src/lib.rs
run_case "core crate" "remotefs remotefs-fuse remotefs-memory remotefs-ssh fusibile" crates/remotefs/src/lib.rs
run_case "dev dependency propagates" "remotefs-memory remotefs-fuse fusibile" crates/remotefs-memory/README.md
run_case "workspace file" "remotefs remotefs-fuse remotefs-memory remotefs-ssh fusibile" Cargo.lock
run_case "prefix is not a match" "" crates/remotefs-sshx/a.rs
CHANGED_ALL=1 run_case "all flag" "remotefs remotefs-fuse remotefs-memory remotefs-ssh fusibile"
run_case "empty input" ""

if [ "$FAILURES" -ne 0 ]; then
  echo "$FAILURES test(s) failed" >&2
  exit 1
fi
echo "test_changed_crates: all cases passed"
