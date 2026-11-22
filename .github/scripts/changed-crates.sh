#!/usr/bin/env bash
# Maps changed repository paths to the workspace crates they affect.
#
# Reads changed paths from stdin (one per line; empty input is fine) and prints
# "<package>=true|false" for every workspace member, sorted by name.
#
# A member is marked when:
#   - a changed path is a workspace-wide input (see the list below), or
#   - a changed path lies under the member's directory, or
#   - the member has a path dependency (any kind) on a marked member.
# Setting CHANGED_ALL=1 marks every member.
#
# Usage: changed-crates.sh METADATA_JSON < changed-paths
#   METADATA_JSON is the output of `cargo metadata --no-deps --format-version 1`.
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "usage: changed-crates.sh METADATA_JSON < changed-paths" >&2
  exit 2
fi

metadata=$1
all=false
if [ "${CHANGED_ALL:-}" = "1" ]; then
  all=true
fi

# The logic lives in jq so it runs unchanged on bash 3.2 (macOS) and bash 5.
jq -R -n -r --argjson all "$all" --slurpfile meta "$metadata" '
  [inputs | select(length > 0)] as $changed
  | $meta[0].workspace_root as $root
  | [$meta[0].packages[] | {
      name: .name,
      dir: (.manifest_path | sub("^" + $root + "/"; "") | sub("/Cargo[.]toml$"; "")),
      deps: [.dependencies[] | select(.path != null) | .name]
    }] as $members
  | ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", "Justfile"] as $workspace_files
  | ($changed | any(. as $p
      | ($workspace_files | index($p)) != null
        or ($p | startswith("just/"))
        or ($p | startswith(".github/scripts/"))
        or $p == ".github/workflows/ci.yml"
        or $p == ".github/workflows/crate-ci.yml"
    )) as $workspace_wide
  | (if $all or $workspace_wide then [$members[].name]
     else [$members[]
        | select(. as $m | $changed | any(. as $p
            | $p == $m.dir or ($p | startswith($m.dir + "/"))))
        | .name]
     end) as $seed
  | (reduce range(0; $members | length) as $_ ($seed;
      . as $marked
      | . + [$members[] | select(any(.deps[]; . as $d | $marked | index($d) != null)) | .name]
      | unique)) as $marked
  | $members | sort_by(.name)[]
  | . as $m
  | "\($m.name)=\(if ($marked | index($m.name)) != null then "true" else "false" end)"
'
