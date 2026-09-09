# Migrating to remotefs 1

## Overview

remotefs 1 replaces the 0.3 trait with a shared, absolute-path contract. The
blocking [`RemoteFs`](https://docs.rs/remotefs/1/remotefs/trait.RemoteFs.html)
and optional
[`AsyncRemoteFs`](https://docs.rs/remotefs/1/remotefs/trait.AsyncRemoteFs.html)
traits now use shared receivers for operations, owned streams for transfers,
and explicit transfer completion.

The backend remains responsible for protocol behavior. Every filesystem path
must be absolute; working-directory state and relative-path resolution are
removed. Recursive search takes an explicit absolute root, and runtime
adaptation is available through adapters. Start by updating the root dependency:

```toml
remotefs = "1"
```

This document is shipped in the package and is also maintained in the
[canonical remotefs repository](https://github.com/remotefs-rs/remotefs-rs/blob/main/MIGRATION.md).
The [migration skill](skills/migrate-to-remotefs-1/SKILL.md) gives an agent the
same backend-oriented checklist.

## API changes

| remotefs 0.3                         | remotefs 1                                                 |
| ------------------------------------ | ---------------------------------------------------------- |
| `&mut self` operations               | `&self` operations; lifecycle keeps mutable receivers      |
| `connect() -> RemoteResult<Welcome>` | `connect() -> RemoteResult<()>`; `Welcome` is removed      |
| `pwd` / `change_dir`                 | removed; pass absolute paths to every operation            |
| `find` / `iter_search`               | `find` / `find_async` free functions with an explicit root |
| `setstat`                            | `set_metadata(&SetMetadata)`                               |
| `mov`                                | `rename`                                                   |
| `open_file` / `create_file`          | `read_file` / `write_file`                                 |
| `on_read` / `on_written`             | consuming stream `finish`                                  |
| Owned boxed one-shot I/O             | borrowed `&mut (dyn Read/Write + Send)`                    |
| Async boxed one-shot I/O             | borrowed futures I/O forms with `Send + Unpin`             |
| `&Metadata` transfer arguments       | `ReadOptions` / `WriteOptions`                             |
| Create-directory mode                | `Option<UnixPex>`                                          |
| `(u32, String)` exec result          | `ExecOutput::new` and readable fields                      |
| `DirectoryAlreadyExists`             | `AlreadyExists`                                            |
| `PexError`                           | `PermissionDenied`                                         |
| `SslError`                           | `ConnectionError` with its source                          |
| `RemoteError.kind`                   | `RemoteError::kind()`                                      |
| `RemoteError.msg`                    | source/display text                                        |
| `RemoteError::new_ex`                | `with_source` / `with_message`                             |
| `RemoteError: Clone + Eq + Hash`     | compare `kind()`; errors retain typed sources              |
| `Metadata::size: u64`                | `Metadata::size: Option<u64>`                              |
| File struct literals                 | `File::new`                                                |
| Exhaustive public enum matches       | wildcard arms for future variants                          |
| `ReadAndSeek` / `WriteAndSeek`       | `RemoteRead` / `RemoteWrite` plus `ReadStream::new`        |
| Old stream `From` constructors       | backend implementations passed to `ReadStream::new`        |
| Runtime constructor parameters       | native async clients or `BlockOn` / `Unblock` adapters     |

### Connection results

Both traits report successful connection and authentication with `Ok(())`:

```rust
// RemoteFs
fn connect(&mut self) -> RemoteResult<()>;

// AsyncRemoteFs
async fn connect(&mut self) -> RemoteResult<()>;
```

Remove `Welcome` imports, reexports, constructors, and banner builders. Replace
successful `Ok(Welcome::default())` or banner-bearing results with `Ok(())`,
preserving connection and authentication errors. Update forwarding implementations,
adapters, and mocks to return the same unit result.

Consumers should call `client.connect()?` or `client.connect().await?` without
binding a greeting. If they displayed `welcome.banner`, use a backend-specific
banner accessor when available, or remove banner display. The core traits provide
no generic banner replacement.

### Absolute paths

The backend receives absolute paths:

```rust
use std::path::Path;

use remotefs::{RemoteFs, RemoteResult};

fn report(client: &dyn RemoteFs) -> RemoteResult<remotefs::File> {
    client.stat(Path::new("/srv/data/report.txt"))
}
```

`path::ensure_absolute` validates remote roots independently of the client
operating system. It accepts POSIX paths beginning with `/`, Windows drive paths
beginning with an ASCII letter, `:`, and `/` or `\`, and UNC paths beginning
with `\\` followed by nonempty server and share components separated by `/` or
`\`. Empty, relative, drive-relative (`C:file`), single-backslash-rooted
(`\file`), incomplete UNC, and Windows device-namespace paths (`\\?\` or
`\\.\`) return `InvalidPath`.

Validation returns the original path unchanged, including non-UTF-8 content and
`.` or `..` components. Backends enforce their own protocol-specific path rules.
The crate provides no working-directory wrapper or relative-path resolver.

### Consumer example

One-shot transfers borrow the caller's I/O object and return the byte count:

```rust
use std::io::Cursor;
use std::path::Path;

use remotefs::fs::WriteOptions;
use remotefs::{RemoteFs, RemoteResult};

fn upload(fs: &dyn RemoteFs, path: &Path, bytes: &[u8]) -> RemoteResult<u64> {
    let opts = WriteOptions::default().size_hint(bytes.len() as u64);
    let mut input = Cursor::new(bytes);
    fs.write_file(path, &opts, &mut input)
}
```

A size hint is mandatory only for protocols that require one. Read offsets are
honored natively when possible and otherwise skipped once; `length` limits the
returned bytes. Calling `finish` is the protocol completion contract. Closing
or dropping a stream alone does not complete a remote transfer.

Transfers are cancellable by dropping the future or stream. Backends must
release the data socket before taking a control-channel lock. FTP permits only
one active data transfer; a second open must return `ProtocolError` rather
than deadlocking or silently sharing the socket.

## Backend checklist

Update a backend in this order:

1. Remove backend cwd state and path resolution. Validate that every required
   backend operation receives an absolute path.
2. Change safe operations to `&self`. Wrap mutable protocol handles behind the
   backend's synchronization primitive for the blocking trait.
3. Implement `capabilities()` accurately, including range, seek, stream,
   append, copy, symlink, metadata, POSIX mode, and exec support.
4. Replace borrowed or callback finalizers with owned `RemoteRead` and
   `RemoteWrite` streams. Implement explicit `finish`; keep a best-effort
   logging `Drop` fallback for abandoned protocol resources.
5. Enforce required size hints before opening a stream. SCP requires a size;
   SFTP and kube use the bound; GCS uses a hint. Honor read offsets and
   lengths, including zero and beyond-EOF ranges.
6. Preserve typed `RemoteError` causes. Map permission, connection, and
   protocol failures to the new kinds without string-only conversion.
7. Implement the appropriate native trait. Use a runtime-neutral
   `AsyncRemoteFs` implementation for native async clients; use
   `remotefs::adapters::blocking::BlockOn` or
   `remotefs::adapters::r#async::Unblock` only when the protocol does not have
   the corresponding native implementation.
8. Replace memory callbacks with `Box<dyn Fn() -> u32 + Send + Sync>` and
   remove unsafe downcasts. Protect the libssh SFTP handle when sharing it.

The expected native implementations are:

| Native async clients               | Native blocking clients     |
| ---------------------------------- | --------------------------- |
| s3, gcs, kube, smb-rust, ssh-russh | ssh2, libssh, pavao, memory |
| webdav/reqwest, ftp/suppaftp async |                             |

Keep each backend's own test-container suite green. Do not assume that a
protocol's ability to create an object implies support for streaming,
seeking, ranges, or concurrent transfers.

## Consumer checklist

Consumers should:

- connect before placing a client in an `Arc`;
- pass explicit absolute paths to every filesystem operation;
- call `find` or `find_async` with an explicit absolute root;
- use range offsets for partial reads and check `capabilities()` before
  requiring native behavior;
- call `finish` exactly once from flush/release paths and avoid double-finishing
  the same owned stream;
- map failures through `io::Error::from` where an operating-system error is
  required; and
- remove a global mutex when the client and protocol permit safe shared use.

`&self` allows a client to be shared; it is not a promise of parallel
transfers. FTP and other protocols that permit one data channel still need
consumer-level serialization.

For FUSE, connect before sharing the client, use the request's range offset,
finish streams from flush/release, and make the release path tolerate an
abandoned transfer. Do not finish the same owned stream in both flush and
release.

## Async migration

Enable `async` for the runtime-neutral trait and futures I/O types:

```toml
remotefs = { version = "1", features = ["async"] }
```

`AsyncRemoteFs` mirrors the blocking contract. Its streams implement the
`futures-io` traits and are `Send`; they expose async `seek` and consuming
`finish`. A Tokio application can use native async clients directly, adapt a
blocking client with `remotefs::adapters::r#async::Unblock`, or adapt an async
client for a blocking consumer with
`remotefs::adapters::blocking::BlockOn`.

`BlockOn` must not be called from an async execution context because
`Handle::block_on` would block the executor thread. Use `spawn_blocking` for
that direction. Each `BlockOn` one-shot transfer uses one scoped worker thread
for borrowed source or destination I/O. That worker exits before the call returns;
an in-progress blocking I/O call must finish first. `Unblock` moves blocking
control calls and stream operations to Tokio's blocking pool and keeps the
caller's async I/O borrowed only for the duration of each one-shot operation.

`Unblock` streamed writes acknowledge bytes accepted into a bounded local buffer.
Flush or finish waits for the worker and reports failures. Dropping a stream may
leave accepted bytes written remotely without finalizing the transfer.

Dropping an async transfer future cancels the local pump and closes the caller's
pipe endpoint. A running blocking worker may continue until its backend operation
and cleanup return; cancellation cannot interrupt arbitrary blocking I/O.
Successful one-shot uploads stop polling the source when the backend completes,
including overrides that consume a declared length without waiting for EOF.
Downloads drain buffered bytes and flush the destination before returning success.
A backend must still release its protocol data socket before acquiring a control
lock. A finish error is returned even when the copy itself succeeds; when both
fail, the returned error retains both causes.

## Transfer lifecycle

Use the lifecycle that matches the operation:

```rust
let mut stream = fs.open(path, &read_options)?;
std::io::copy(&mut stream, &mut destination)?;
stream.finish()?;
```

For writes, flush before `finish` when the protocol requires it. A stream can
be seekable only when `seekable()` is true; otherwise seeking returns
`ErrorKind::Unsupported`. A dropped stream is an abandoned transfer, not a
successful transfer. Implement logging and best-effort cleanup in the backend
when the protocol makes that possible, but do not treat `Drop` as completion.

## Agent skill

The packaged
[migrate-to-remotefs-1 skill](skills/migrate-to-remotefs-1/SKILL.md) can be
copied or symlinked into an agent's skill directory without changing the
current user's setup:

```sh
mkdir -p "$CODEX_HOME/skills/migrate-to-remotefs-1"
cp skills/migrate-to-remotefs-1/SKILL.md "$CODEX_HOME/skills/migrate-to-remotefs-1/"
```

Or create a local symlink:

```sh
ln -s "$PWD/skills/migrate-to-remotefs-1" "$CODEX_HOME/skills/migrate-to-remotefs-1"
```

When installed elsewhere, the skill points to this packaged document and the
[canonical migration guide](https://github.com/remotefs-rs/remotefs-rs/blob/main/MIGRATION.md),
so it does not depend on a relative path into this repository.
