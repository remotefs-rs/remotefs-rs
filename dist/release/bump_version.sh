#!/usr/bin/env bash
# Bump the workspace version across every tracked location.
# Usage: bump_version.sh <version> [root]
#
# Rewrites, in the workspace root:
#   - the shared [workspace.package] version,
#   - the version of every internal path dependency in [workspace.dependencies]
#     (entries whose path starts with crates/),
# and, in README.md and every crates/*/README.md, the MAJOR.MINOR dependency
# snippets of every workspace member (members are read from the root manifest).
# Third-party entries are never touched.
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

# the READMEs pin only MAJOR.MINOR, the way a dependency requirement is written
MINOR_REQ="${VERSION%.*}"

# values reach perl through the environment, so no quoting layers are involved.
# In-place substitution, portable across GNU and BSD/macOS perl.
export BUMP_VERSION="$VERSION" BUMP_MINOR="$MINOR_REQ"

# root Cargo.toml — the [workspace.package] version. Line-anchored, so the
# `version = ` keys nested inside [workspace.dependencies] entries are not touched.
perl -0777 -pi -e 's/^version = "[0-9][0-9A-Za-z.\-]*"/version = "$ENV{BUMP_VERSION}"/m' "$ROOT/Cargo.toml"

# root Cargo.toml — the version of each internal path dependency. Only entries
# whose path lives under crates/ match, so third-party versions stay as they are.
perl -0777 -pi -e 's/^([A-Za-z0-9_-]+ = \{ version = )"[0-9][0-9A-Za-z.\-]*"(, path = "crates\/)/$1"$ENV{BUMP_VERSION}"$2/mg' "$ROOT/Cargo.toml"

# workspace member names, taken from the root `members` array (basename of each path)
members=$(perl -0777 -ne 'if (/^members\s*=\s*\[(.*?)\]/ms) { my $list = $1; while ($list =~ /"([^"]+)"/g) { print((split m{/}, $1)[-1], "\n") } }' "$ROOT/Cargo.toml")

# README snippets — `<member> = "MAJOR.MINOR"` and `<member> = { version = "MAJOR.MINOR"`.
# The lookbehind keeps `remotefs` from matching inside `remotefs-ssh`.
for readme in "$ROOT/README.md" "$ROOT"/crates/*/README.md; do
    [ -f "$readme" ] || continue
    while IFS= read -r member; do
        [ -n "$member" ] || continue
        MEMBER="$member" perl -0777 -pi -e '
            s/(?<![\w-])\Q$ENV{MEMBER}\E = "[0-9][0-9A-Za-z.\-]*"/$ENV{MEMBER} = "$ENV{BUMP_MINOR}"/mg;
            s/(?<![\w-])\Q$ENV{MEMBER}\E = \{ version = "[0-9][0-9A-Za-z.\-]*"/$ENV{MEMBER} = { version = "$ENV{BUMP_MINOR}"/mg;
        ' "$readme"
    done <<< "$members"
done

echo "Bumped to $VERSION"
