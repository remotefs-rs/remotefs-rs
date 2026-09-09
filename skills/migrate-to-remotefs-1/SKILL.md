---
name: migrate-to-remotefs-1
description: Migrate a remotefs backend from the 0.3 contract to remotefs 1.
---

# Migrate a backend to remotefs 1

Read the packaged [`MIGRATION.md`](../../MIGRATION.md) and the [canonical remotefs migration guide](https://github.com/remotefs-rs/remotefs-rs/blob/main/MIGRATION.md)
before changing code. Inspect the target crate's `AGENTS.md`, `CLAUDE.md`,
`CONTRIBUTING.md`, Justfile, CI, test-container recipes, and changelog. Do not
assume this repository is the target crate or that downstream crates have
already migrated.

## Workflow

1. Inventory the 0.3 trait implementation, cwd state, path resolution,
   protocol locks, stream finalizers, callbacks, error mappings, and feature
   flags.
2. Detect native async support from the protocol API and existing `Runtime` or
   `block_on` usage. Implement the native `AsyncRemoteFs` contract when the
   protocol is async; use `BlockOn` or `Unblock` only when an adapter is the
   correct boundary.
3. Execute the backend checklist in `MIGRATION.md` in order. Preserve typed
   error sources, enforce required size hints before opening, honor offsets and
   lengths, advertise capabilities, and move finalization into owned streams
   with explicit `finish` plus best-effort logging cleanup.
   Remove working-directory state and relative-path resolution. Pass explicit
   absolute paths to every filesystem operation and validate them with
   `remotefs::path::ensure_absolute`. Its remote POSIX, Windows drive, and UNC
   root checks are independent of the client platform; `Path::is_absolute`
   checks the client platform's syntax and is not a substitute. The crate
   provides no working-directory wrapper or relative-path resolver.
4. Apply the protocol-specific notes: SCP requires a size, SFTP and kube use
   the bound, GCS uses a hint; memory removes unsafe downcasts and retains
   observable callback behavior; SSH moves Drop finalization into explicit
   finish while retaining a best-effort fallback; libssh protects its SFTP
   handle; memory callbacks are `Box<dyn Fn() -> u32 + Send + Sync>`.
5. Keep the crate's own unit, integration, and test-container suites green.
   Run its documented feature matrix and platform checks. Add or update tests
   for absolute paths, ranges, capabilities, finish errors, cancellation, and
   protocol single-transfer rules where applicable.
6. Bump the backend crate's major version according to its release policy and
   update its changelog. Do not prescribe a common numerical backend version.
   Do not silently skip unavailable integration suites; report the missing
   dependency, service, or platform check.

For FUSE consumers, connect before placing a client in `Arc`, remove a global
mutex where sharing is safe, use request range offsets, finish in flush or
release without double-finishing an owned stream, check capabilities, and map
errors through `io::Error::from`. `&self` permits sharing but does not promise
parallel transfers on protocols with one data channel.

Install this skill by copying or symlinking this directory into the agent's
skill directory; do not modify the current user's setup as part of a backend
migration. The relative packaged-guide link is for the repository copy; the
canonical link remains valid after installation elsewhere.
