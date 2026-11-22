# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

`AGENTS.md` is a symlink to this file and holds the same agent contract.

## Commands

Every task runs through a [`just`](https://just.systems) recipe. Do not bypass a
recipe with an ad hoc `cargo` or tool command. If a recurring task has no
recipe, add one under `just/` before using it. Run `just` to list all recipes.

```sh
just build                  # cargo build --all-targets for the workspace
just build "--workspace --exclude fusibile --exclude remotefs-fuse"
just release                # release build
just test                   # unit and doc tests, one crate at a time
just test_features          # the same, with default features only
just test_containers        # Docker-backed tests of remotefs-ftp and remotefs-ssh
just coverage               # llvm-cov per crate, writes lcov.info
just fmt                    # dprint fmt (Markdown, Rust, TOML, YAML)
just fmt_check              # dprint check
just clippy "-- -D warnings" # Clippy for the workspace, warnings denied
just doc                    # cargo doc --no-deps with RUSTDOCFLAGS="-D warnings"
just deny                   # cargo deny check
just dependency_tree        # inspect the resolved dependency graph
just zizmor                 # audit GitHub Actions workflows
just fmt_md_tables FILE     # align a Markdown table in place
just package_list "-p remotefs"  # files included in one crate's package
just scan_secrets           # trufflehog filesystem
just check                  # the full local quality gate
just setup_githooks         # point core.hooksPath at .githooks
just changelog_all          # regenerate every crate's CHANGELOG.md
just changelog_all 1.2.0    # ... with the unreleased commits as 1.2.0
just fusibile_release_notes 1.2.0
just bump_version 1.2.0     # set the shared version and refresh Cargo.lock
just test_scripts           # release, change detection, and install script tests
just build_musl x86_64-unknown-linux-musl
just build_release x86_64-apple-darwin
just publish crate "--dry-run --allow-dirty"
just publish_all "--dry-run --allow-dirty"
```

`just check` is the required gate before declaring work done. It chains
`fmt_check`, Clippy with warnings denied, `doc`, `deny`, `test_scripts`, and
`test`.

Tests run one crate at a time, because workspace-wide feature unification
enables both `ring` and `aws-lc-rs` rustls providers and rustls refuses to choose.
Set `SKIP_CRATES` to a space separated list of crates to leave out, for example
`SKIP_CRATES="fusibile remotefs-fuse" just test` on a machine without FUSE.
`fusibile` and `remotefs-fuse` need FUSE to build: libfuse3 on Linux, macFUSE on
macOS, and Dokany on Windows, which the Windows CI job does not install.

Unit tests that start a container are marked `ignore` unless the
`with-containers` feature is enabled. They need a running Docker daemon, so only
the Linux CI job and `just test_containers` run them.

CI runs the full workspace on Linux, the FUSE crates excluded on macOS and
Windows, and `remotefs-ssh` excluded on Windows, where OpenSSL is not installed.

If a required tool is missing, say so. Never claim a check passed or silently
swap in a weaker command.

## Workspace layout

The repository is one Cargo workspace. Every crate is a member under `crates/`
and has its own README:

| Crate             | Path                     | Purpose                                      |
| ----------------- | ------------------------ | -------------------------------------------- |
| `remotefs`        | `crates/remotefs`        | protocol-agnostic contract and shared types  |
| `remotefs-aws-s3` | `crates/remotefs-aws-s3` | AWS S3 backend                               |
| `remotefs-ftp`    | `crates/remotefs-ftp`    | FTP and FTPS backend                         |
| `remotefs-fuse`   | `crates/remotefs-fuse`   | FUSE and Dokany driver                       |
| `remotefs-gcs`    | `crates/remotefs-gcs`    | Google Cloud Storage backend                 |
| `remotefs-kube`   | `crates/remotefs-kube`   | Kubernetes pod backend                       |
| `remotefs-memory` | `crates/remotefs-memory` | in-memory backend for tests and simulation   |
| `remotefs-smb`    | `crates/remotefs-smb`    | SMB backend (pavao on UNIX, WNet on Windows) |
| `remotefs-ssh`    | `crates/remotefs-ssh`    | SSH backend (libssh2, libssh, or russh)      |
| `remotefs-webdav` | `crates/remotefs-webdav` | WebDAV backend                               |
| `fusibile`        | `crates/fusibile`        | command line program, released as a binary   |

`remotefs` is the root of the family. The backends implement its traits, and the
FUSE driver and `fusibile` program against them.

## Architecture

remotefs is the protocol-agnostic contract that every backend implements, and
that consumers such as `remotefs-fuse` and termscp program against. The core
crate ships no client of its own; it is a library-only crate
(`crates/remotefs/src/lib.rs`, crate name `remotefs`) with no binaries and no
examples.

- **The filesystem contracts are absolute-path APIs.**
  `crates/remotefs/src/fs/sync.rs` declares `RemoteFs` and
  `crates/remotefs/src/fs/async.rs` declares `AsyncRemoteFs` when the `async`
  feature is enabled. Required methods answer protocol questions; recursive
  removal and one-shot transfers have default implementations. Backends may
  override defaults when the protocol offers a faster path. Both traits must
  remain object-safe, so never add generic methods to them.
- **Types are shared, not per-backend.** `crates/remotefs/src/fs/file/` holds
  `File`, `Metadata`, `FileType`, `UnixPex`, and `UnixPexClass`;
  `crates/remotefs/src/fs/errors.rs` holds `RemoteError`, `RemoteErrorType`, and
  `RemoteResult`, and `crates/remotefs/src/fs/options.rs` holds transfer options
  and command output. Downstream crates re-export these types, so coordinate
  public contract changes across the workspace.
- **Streams own transfer state.** `crates/remotefs/src/fs/stream.rs` defines
  `RemoteRead` and `RemoteWrite`, which back `ReadStream` and `WriteStream`. Each
  stream may be seekable; seeking a non-seekable stream returns
  `ErrorKind::Unsupported`. `finish` consumes the stream and completes the
  protocol transfer. Dropping one abandons it, with backend-specific best-effort
  cleanup.
- **Async streams are runtime-neutral.**
  `crates/remotefs/src/fs/stream/async.rs` uses `futures-io` and exposes
  `AsyncReadStream` and `AsyncWriteStream`. The Tokio adapters live in
  `crates/remotefs/src/adapters/blocking/` and
  `crates/remotefs/src/adapters/async/`: `BlockOn` presents async clients to
  blocking consumers, while `Unblock` offloads blocking clients for async
  consumers.
- **Paths are absolute on the remote host.**
  `crates/remotefs/src/path.rs` validates POSIX, Windows drive, and UNC roots
  independently of the client platform. There is no working-directory wrapper or
  relative-path resolver. `crates/remotefs/src/find.rs` provides explicit-root
  `find` and `find_async`.
- **Command layer.** `Justfile` is a thin importer of `just/*.just`: `build`,
  `changelog`, `code_check`, `publish`, `release`, and `test`. Each group carries
  a `[group(...)]` attribute so `just --list` stays organized. Recipes take an
  `args=""` passthrough rather than hard-coding flags.
- **Formatting is dprint, not cargo fmt.** `dprint.json` owns Markdown, TOML,
  and YAML, and delegates `.rs` files to nightly rustfmt through its exec plugin
  (`--edition 2024`). `rustfmt.toml` uses nightly-only options, which is why
  nightly is required. Always format with `just fmt`.
- **Release path.** One version covers every crate, set under
  `[workspace.package]` and mirrored in `[workspace.dependencies]` by
  `dist/release/bump_version.sh`. Commits follow Conventional Commits, and
  `cliff.toml` builds each crate's `CHANGELOG.md` from the commits under its
  directory, bounded by its own tags. The Release workflow tags every crate,
  publishes the `fusibile` binaries, and runs `just publish_all`. See
  `docs/release.md`.
- **Supply-chain policy.** `deny.toml` is strict: license allowlist,
  `yanked = "deny"`, `unmaintained = "all"`, wildcard versions denied, and
  crates.io as the only allowed source. It runs with `all-features = true`.

## Conventions

- Toolchain is pinned to Rust 1.100.0 (`rust-toolchain.toml`). `package.edition`
  is 2024 for every crate, set once under `[workspace.package]`.
- `package.rust-version` stays per crate: remotefs 1.89.0, remotefs-ftp 1.89.0,
  remotefs-kube 1.89.0, remotefs-memory 1.89, remotefs-smb 1.89.0,
  remotefs-aws-s3 1.94.1, remotefs-fuse 1.94.1, fusibile 1.94.1, remotefs-webdav
  1.94.1, remotefs-ssh 1.95.0, remotefs-gcs 1.98.0. Do not bump any of them as
  part of unrelated changes.
- Every third-party dependency version lives in the root
  `[workspace.dependencies]` table. Members reference it with `workspace = true`
  and keep only their own `features` and `optional` flags. `default-features` is
  set in the root, never in a member. Path dependencies between crates use the
  same table.
- Keep `[workspace.dependencies]` alphabetically sorted, with bare minimal
  versions.
- `Cargo.lock` is committed, so the whole workspace resolves the same versions
  everywhere.
- Public library items need canonical rustdoc, including a runnable example.
  `just test` runs doctests, and `just doc` denies warnings. Intra-doc links must
  not point at items behind a feature that the default docs build does not
  enable.
- Conventional Commits, imperative and lower-case. No agent attribution,
  session links, or agent `Co-Authored-By` lines. Removing or changing a public
  API needs a breaking-change commit (`type(scope)!:` with a `BREAKING CHANGE:`
  footer) on that crate's scope.
- Do not stage planning state. `docs/superpowers/`, `.superpowers/`, and
  `.claude/plans/` are gitignored and dprint-excluded.
- After editing a Markdown file that contains a table, run
  `fmt-md-tables -i <file>`. If that tool is not installed, `just fmt` aligns the
  table, and the file must still pass `just fmt_check`.
- After any change under `.github/workflows/`, run `just zizmor` until it exits
  clean. Pin actions to a full commit SHA with the matching tag in a trailing
  comment, declare least-privilege permissions, and set `persist-credentials:
  false` on checkout unless the job pushes. Pin `setup-just` with `just-version`.
