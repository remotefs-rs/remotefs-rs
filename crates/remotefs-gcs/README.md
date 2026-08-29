# Rust template

[![CI](https://github.com/veeso/rust-template/actions/workflows/ci.yml/badge.svg)](https://github.com/veeso/rust-template/actions/workflows/ci.yml)
[![TruffleHog](https://github.com/veeso/rust-template/actions/workflows/trufflehog.yml/badge.svg)](https://github.com/veeso/rust-template/actions/workflows/trufflehog.yml)
[![zizmor](https://github.com/veeso/rust-template/actions/workflows/zizmor.yml/badge.svg)](https://github.com/veeso/rust-template/actions/workflows/zizmor.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Conventional Commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white)](https://www.conventionalcommits.org)

A reusable Rust 1.98.0 project template with a complete local and continuous
integration toolchain.

The starter package builds as both a library and a binary. Keep both targets or
remove one to match the project you are creating.

## Use this template

Create a repository from
[veeso/rust-template](https://github.com/veeso/rust-template), then review this
checklist:

1. Replace `rust-template` and `rust_template` in `Cargo.toml` and `src/`.
2. Update the author, repository, homepage, description, keywords, and
   categories in `Cargo.toml`.
3. Replace repository links and badges in this README.
4. Choose whether to keep the library target, binary target, or both.
5. Review the license and dependency policy in `LICENSE` and `deny.toml`.
6. Configure crates.io trusted publishing for `.github/workflows/publish.yml`.
7. Run `just fmt`, `just check`, and `just setup_githooks`.

## Choose the package shape

Both targets compile by default:

- Keep `src/lib.rs` and `src/main.rs` for a library with a companion binary.
- Delete `src/main.rs` and the `[[bin]]` section for a library-only crate.
- Delete `src/lib.rs` and the `[lib]` section for a binary-only crate, then
  replace the library call in `src/main.rs` with the application entry point.

## Install the tools

The pinned Rust toolchain is installed automatically by rustup. Local recipes
also use these tools:

- [just](https://just.systems) for task execution.
- [dprint](https://dprint.dev) 0.56.1 and nightly rustfmt for formatting.
- [cargo-deny](https://embarkstudios.github.io/cargo-deny/) for dependency
  policy.
- [TruffleHog](https://github.com/trufflesecurity/trufflehog) for secret
  scanning.
- [git-cliff](https://git-cliff.org) for changelog generation.
- [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov) for coverage.
- [zizmor](https://docs.zizmor.sh) for GitHub Actions auditing.
- [shellcheck](https://www.shellcheck.net) for hook validation.

On macOS with Homebrew, install the packaged tools with:

```sh
brew install cargo-deny dprint git-cliff just shellcheck trufflehog zizmor
cargo install cargo-llvm-cov
rustup toolchain install nightly --profile minimal --component rustfmt
```

## Run common tasks

Run `just` to list every recipe.

```sh
just build
just release
just test
just coverage
just fmt
just fmt_check
just lint "-- -D warnings"
just doc
just deny
just scan_secrets
just check
```

`just check` is the local quality gate. It verifies formatting, runs Clippy
with warnings denied, builds documentation with warnings denied, checks the
dependency policy, and runs the tests.

## Enable the Git hooks

Install the tracked pre-commit hook with:

```sh
just setup_githooks
```

The hook scans staged files for secrets, checks formatting, runs Clippy, and
checks dependencies with cargo-deny.

## Update the changelog

Commits follow the
[Conventional Commits](https://www.conventionalcommits.org) specification.
Preview or generate release notes with:

```sh
just changelog_preview 0.1.0
just changelog 0.1.0
```

Review and commit `CHANGELOG.md` before publishing.

## Run continuous integration

GitHub Actions provides:

- Cross-platform builds, tests, and Clippy checks.
- Formatting, documentation, and dependency-policy checks.
- TruffleHog secret scanning.
- zizmor workflow-security auditing.
- Manual crates.io dry runs and trusted publishing.

Every external action is pinned to a verified release commit. Checkout steps do
not persist credentials, and each workflow declares explicit permissions.

## Publish the crate

Generate and review the changelog section for the package version, then open
the `Publish` workflow in GitHub Actions. The workflow defaults to a dry run.
For a live release, disable `dry_run` after configuring crates.io trusted
publishing for this repository and workflow. After crates.io accepts the
package, the workflow creates the matching `v<version>` Git tag.

For a local package verification without uploading:

```sh
just publish "--dry-run --allow-dirty"
```

## License

This project is licensed under the [MIT License](LICENSE).
