# Releasing fusibile and remotefs-fuse

Both crates in this workspace share one version and are always released together under a single
`vX.Y.Z` tag. The whole process is driven by the `Release` GitHub Actions workflow
(`.github/workflows/release.yml`); there is no manual `cargo publish` step.

## One-time setup

These only need doing once per repository, not per release.

1. **GitHub Pages**: Settings, Pages, Build and deployment, Source set to `GitHub Actions`. This
   serves `install.sh` and `install.ps1` from
   <https://remotefs-rs.github.io/remotefs-rs-fuse/>.
2. **Homebrew tap**: `remotefs-rs/homebrew-fusibile` must exist and contain at least a
   `README.md` and a placeholder `Formula/fusibile.rb`. The `publish-homebrew` job overwrites
   `Formula/fusibile.rb` on every release; it does not create the repository itself.
3. **Secrets**: `RELEASE_PAT`, a GitHub token with push access to this repository and to
   `remotefs-rs/homebrew-fusibile`, plus permission to create releases. crates.io publishing uses
   trusted publishing (OIDC), so no crates.io token secret is needed.

## Triggering a release

Go to the **Actions** tab, select the **Release** workflow, and click **Run workflow**. It takes
two inputs:

- `version`: the version to release, e.g. `1.2.0` (no leading `v`).
- `dry_run`: `true` by default. Leave it on to build and verify everything without pushing or
  publishing anything; set it to `false` to actually cut the release.

Equivalent with the GitHub CLI:

```sh
gh workflow run release.yml -f version=1.2.0 -f dry_run=true
gh run watch
```

**Always run a dry run first** for anything beyond a trivial patch release, and read the logs for
all five build jobs before running for real.

## What the pipeline does

1. **prepare**: validates the version format, bumps it everywhere with
   `dist/release/bump_version.sh`, regenerates `CHANGELOG.md` and `RELEASE_NOTES.md` with
   `git-cliff`, and (unless dry run) commits and pushes `chore: release vX.Y.Z` to `main`.
2. **build**: builds `fusibile` for five targets in parallel:
   - `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`: fully static binaries with
     Samba and `libfuse3` compiled from source, built in a pinned Alpine container
     (`just build_musl`).
   - `aarch64-apple-darwin`: native build with `smb-vendored` (Samba compiled from source,
     dynamically linked against gnutls/libunistring at runtime).
   - `x86_64-apple-darwin`: cross-compiled from an `aarch64-apple-darwin` runner, **without SMB
     support** (see [Known limitations](#known-limitations) below).
   - `x86_64-pc-windows-msvc`: native build with default features (SMB uses the Win32 WNet API,
     no vendored Samba involved).

   Each target is packaged as `fusibile-vX.Y.Z-<target>.tar.gz` (`.zip` on Windows) plus a
   matching `<target>.sha256`.
3. **release**: downloads every build artifact, assembles them alongside `install.sh` and
   `install.ps1`, and (unless dry run) creates the GitHub release `vX.Y.Z` with those assets and
   the generated release notes.
4. **publish-crates**: publishes `remotefs-fuse` first, waits for it to appear in the crates.io
   index, then publishes `fusibile`. Skipped entirely on a dry run.
5. **publish-homebrew**: regenerates `Formula/fusibile.rb` in the tap repository from the build
   checksums and pushes it. Skipped entirely on a dry run.

## After a real release

1. Confirm the GitHub release has all 12 assets (5 archives, 5 checksums, `install.sh`,
   `install.ps1`).
2. Confirm both crates show the new version on crates.io.
3. Confirm the tap formula was updated: `brew install remotefs-rs/fusibile/fusibile` on a clean
   machine should pull the new version.
4. Confirm the install scripts work: `curl -sSLf https://remotefs-rs.github.io/remotefs-rs-fuse/install.sh | sh`
   on a clean Linux or macOS box, and `irm https://remotefs-rs.github.io/remotefs-rs-fuse/install.ps1 | iex`
   on Windows.

## Known limitations

- **Windows on ARM64** is not built: `dokany-rs`, the Windows driver binding, has no aarch64
  support yet.
- **`x86_64-apple-darwin` (Intel macOS) has no SMB support.** `smb-vendored`'s vendored Samba
  build fails to link on that target with undefined `libintl` symbols, whether cross-compiled or
  built natively on an Intel runner. If you need SMB on Intel macOS, build from source instead:
  `cargo install fusibile --locked --features smb`, with `libsmbclient` installed via Homebrew.

## Testing the pipeline before it exists on `main`

`workflow_dispatch` only lets you pick a `ref` to run against once the workflow file is already
known on the default branch; a workflow that only exists on a feature branch cannot be dispatched
at all. If you need to dry-run changes to `release.yml` before merging, push a minimal placeholder
(just the `on: workflow_dispatch` trigger and a no-op job) to `main` first, dispatch against your
branch as usual, then let the real content replace the placeholder when the branch merges.
