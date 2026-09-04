#!/usr/bin/env bash
# Bump the workspace version across every tracked location.
# Usage: bump_version.sh <version> [root]
#
# Text substitution only — Cargo.lock is refreshed separately with
# `cargo update --workspace`, so this script stays testable without a registry.
set -euo pipefail

VERSION="${1:?usage: bump_version.sh <version> [root]}"
if [[ ! "$VERSION" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
    echo "invalid release version: $VERSION (expected MAJOR.MINOR.PATCH)" >&2
    exit 2
fi
ROOT="${2:-$(git rev-parse --show-toplevel)}"

# the README pins only MAJOR.MINOR, the way a dependency requirement is written
MINOR_REQ="${VERSION%.*}"

# in-place substitution that works on both GNU and BSD/macOS
sedi() { perl -0777 -pi -e "$1" "$2"; }

# root Cargo.toml — the [workspace.package] version. Line-anchored, so the
# `version = ` keys nested inside [workspace.dependencies] entries are not touched.
sedi "s/^version = \"[0-9][0-9A-Za-z.\\-]*\"/version = \"$VERSION\"/m" "$ROOT/Cargo.toml"

# root Cargo.toml — the path dependency on the library crate. Without this the
# published fusibile would depend on a stale remotefs-fuse requirement.
sedi "s/^remotefs-fuse = \{ version = \"[0-9][0-9A-Za-z.\\-]*\"/remotefs-fuse = { version = \"$VERSION\"/m" "$ROOT/Cargo.toml"

# README.md — the dependency snippet shown to library users
sedi "s/^remotefs-fuse = \"[0-9][0-9A-Za-z.\\-]*\"/remotefs-fuse = \"$MINOR_REQ\"/m" "$ROOT/README.md"

echo "Bumped to $VERSION"
