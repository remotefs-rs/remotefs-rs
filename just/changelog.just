# Preview the unreleased commits of fusibile's next release, without the header.
[group('changelog')]
fusibile_release_notes version:
  git-cliff --config cliff.toml --unreleased --tag "v{{ version }}" --tag-pattern "^(v|fusibile-v)[0-9]" --include-path "crates/fusibile/**" --strip header

# Regenerate every crate's CHANGELOG.md from its own history. With a version, the
# unreleased commits become that crate's release, tagged v<version>.
[group('changelog')]
changelog_all version="":
  #!/usr/bin/env bash
  set -eo pipefail
  set -f
  cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | while read -r crate; do
    case "$crate" in
      remotefs) pattern='^(v|remotefs-v)[0-9]'; prefix=remotefs; include="--include-path crates/remotefs/** --include-path src/**" ;;
      remotefs-fuse) pattern='^(v|fusibile-v|remotefs-fuse-v)[0-9]'; prefix=remotefs-fuse; include="--include-path crates/remotefs-fuse/**" ;;
      fusibile) pattern='^(v|fusibile-v)[0-9]'; prefix=fusibile; include="--include-path crates/fusibile/**" ;;
      *) pattern="^(v|${crate}-v)[0-9]"; prefix="$crate"; include="--include-path crates/${crate}/**" ;;
    esac
    if [ -n "{{ version }}" ]; then
      git-cliff --config cliff.toml --tag-pattern "$pattern" $include --tag "v{{ version }}" --output "crates/${crate}/CHANGELOG.md"
    else
      git-cliff --config cliff.toml --tag-pattern "$pattern" $include --output "crates/${crate}/CHANGELOG.md"
    fi
    dprint fmt "crates/${crate}/CHANGELOG.md" > /dev/null
  done
