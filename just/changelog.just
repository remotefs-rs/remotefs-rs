# Preview unreleased Conventional Commits for a version.
[group('changelog')]
changelog_preview version:
  git-cliff --config cliff.toml --unreleased --tag "v{{ version }}" --strip all

# Generate CHANGELOG.md through the requested version.
[group('changelog')]
changelog version:
  git-cliff --config cliff.toml --tag "v{{ version }}" --output CHANGELOG.md
  just fmt CHANGELOG.md
