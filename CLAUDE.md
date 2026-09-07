# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with
code in this repository.

`AGENTS.md` is a symlink to this file and holds the same agent contract.

## Commands

Every task runs through a [`just`](https://just.systems) recipe. Do not bypass a
recipe with an ad hoc `cargo` or tool command. If a recurring task has no
recipe, add one under `just/` before using it. Run `just` to list all recipes.

```sh
just build                 # cargo build --all-targets
just release               # release build
just test                  # cargo test --lib, then --doc
just coverage              # cargo llvm-cov, writes lcov.info
just fmt                   # dprint fmt (Markdown, Rust, TOML, YAML)
just fmt_check             # dprint check
just lint "-- -D warnings" # alias of just clippy
just doc                   # cargo doc --no-deps with RUSTDOCFLAGS="-D warnings"
just deny                  # cargo deny check
just scan_secrets          # trufflehog filesystem
just check                 # the full local quality gate
just setup_githooks        # point core.hooksPath at .githooks
just changelog_preview 0.4.0
just changelog 0.4.0
just publish "--dry-run --allow-dirty"
```

`just check` is the required gate before declaring work done. It chains
`fmt_check`, Clippy with warnings denied, `doc`, `deny`, and `test`.

Recipes build with default features (`find`). CI additionally lints with
`--no-default-features` and with `--all-features`, because `no-log` compiles
out every `log` macro and can turn otherwise-live bindings into dead code.
There is no container, network, or platform dependency: the whole suite is
plain unit tests plus doctests and runs everywhere.

If a required tool is missing, say so. Never claim a check passed or silently
swap in a weaker command.

## Architecture

remotefs is the root crate of the [remotefs](https://github.com/remotefs-rs)
family. It defines the protocol-agnostic contract that every backend
(`remotefs-ssh`, `remotefs-ftp`, `remotefs-aws-s3`, `remotefs-smb`,
`remotefs-kube`, `remotefs-webdav`) implements, and that consumers such as
`remotefs-fuse` and termscp program against. It ships no client of its own; it
is a library-only crate (`src/lib.rs`, crate name `remotefs`) with no binaries
and no examples.

- **`RemoteFs` is the whole API surface.** `src/fs/sync.rs` declares the trait:
  required methods are the ones only a protocol can answer (`connect`,
  `list_dir`, `stat`, `open`, `create`, ...), while `remove_dir_all`,
  `append_file`, `create_file`, `open_file`, `on_written`, `on_read`, and
  `find` ship default implementations built on top of the required ones. A
  backend overrides a defaulted method only when the protocol offers a faster
  path. Because the trait must stay object-safe (`Box<dyn RemoteFs>` is tested
  in `src/fs/sync.rs`), never add generic methods to it.
- **Types are shared, not per-backend.** `src/fs/file/` holds `File`,
  `Metadata`, `FileType`, `UnixPex`, and `UnixPexClass`; `src/fs/errors.rs`
  holds `RemoteError`/`RemoteErrorType`/`RemoteResult`; `src/fs/welcome.rs`
  holds `Welcome`. Every downstream crate re-exports these, so any change here
  is a breaking change for the entire family — treat additions to
  `RemoteErrorType` and `Metadata` as semver-major.
- **Streams erase the backend.** `src/fs/stream.rs` wraps caller-supplied
  readers and writers in `ReadStream`/`WriteStream`. Each holds either a plain
  `Read`/`Write` or a `Read + Seek`/`Write + Seek` box; `seekable()` reports
  which, and seeking a non-seekable stream returns `ErrorKind::Unsupported`.
  All boxed trait objects are `Send` so a backend can move a transfer to
  another thread.
- **Command layer.** `Justfile` is a thin importer. Each recipe group lives in
  its own file under `just/` (`build`, `test`, `code_check`, `changelog`,
  `publish`) and carries a `[group(...)]` attribute so `just --list` stays
  organized. Recipes take an `args=""` passthrough rather than hard-coding
  flags.
- **Formatting is dprint, not cargo fmt.** `dprint.json` owns Markdown, TOML,
  and YAML, and delegates `.rs` files to nightly rustfmt through its exec
  plugin (`--edition 2024`, matching this crate's `package.edition`).
  `rustfmt.toml` uses nightly-only options (`imports_granularity`,
  `group_imports`), which is why nightly is required. Always format with
  `just fmt`.
- **Release path.** Commits follow Conventional Commits and `cliff.toml` turns
  them into `CHANGELOG.md`. Publishing goes through `just publish`
  (`cargo publish --locked`); version bumps live in `Cargo.toml`.
- **Supply-chain policy.** `deny.toml` is strict: license allowlist,
  `yanked = "deny"`, `unmaintained = "all"`, wildcard versions denied, and
  crates.io as the only allowed source. It runs with `all-features = true`.

## Conventions

- Toolchain is pinned to Rust 1.98.0 (`rust-toolchain.toml`). `package.edition`
  in `Cargo.toml` is 2024 and `package.rust-version` is 1.89.0; do not bump
  either as part of unrelated changes.
- Public library items need canonical rustdoc, including a runnable example.
  `just test` runs doctests, and `just doc` denies warnings.
- Keep `Cargo.toml` dependency and feature entries alphabetically sorted, with
  bare minimal versions.
- Conventional Commits, imperative and lower-case. No agent attribution,
  session links, or agent `Co-Authored-By` lines.
- Do not stage planning state. `docs/superpowers/`, `.superpowers/`, and
  `.claude/plans/` are gitignored and dprint-excluded.
- After editing a Markdown file that contains a table, run
  `fmt-md-tables -i <file>`.
- After any change under `.github/workflows/`, run `zizmor .github/workflows`
  until it exits clean. Pin actions to a full commit SHA with the matching tag
  in a trailing comment, declare least-privilege permissions, and set
  `persist-credentials: false` on checkout.
