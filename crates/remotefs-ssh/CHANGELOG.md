# Changelog

All notable changes to this project are documented in this file.

## 1.2.0

Released on 2026-11-23

### Breaking changes

- Edition 2024; ssh2-config 0.4

> Edition 2024; ssh2-config 0.4

- Support for multiple SSH backends. Support for libssh.org (remotefs-rs/remotefs-rs-ssh#10)

> Removed `From<SshOpts>`; Removed `new`; use `libssh2` and `libssh` constructors instead; Renamed ssh2-vendored feature to libssh2-vendored

- fix: Benchmarks for libssh2

- feat: libssh backend

- test: Added libssh tests

- fix: Restore From for SshOpts into SftpFs/ScpFs

- fix: broken code for libssh backende

- style: lint

* migrate to remotefs 1 (remotefs-rs/remotefs-rs-ssh#26)

> migrate SftpFs and ScpFs to the remotefs 1 blocking contract and add the native asynchronous RusshSftpFs and RusshScpFs clients with BlockOn wrappers. Every path must be absolute, pwd and change_dir are removed, operations take a shared reference, connect returns unit and the server banner moves to SftpFs::banner and ScpFs::banner, transfers return owned streams that must be finished, SCP create requires a size hint, and the russh backend no longer takes a Tokio runtime.

- chore: release 1.0

* **remotefs-kube:** preserve symlink targets verbatim (#43)

> KubeMultiPodFs stores absolute targets as supplied. Pass a path inside the container, such as /etc/hosts, without the virtual pod and container prefix. SCP and Kubernetes symlink creation require Perl or ln with -T on the remote host.

### Added

- ParseRule for ssh2 config file
- ssh-agent configuration to authenticate
- remotefs 0.3
- Breaking: Edition 2024; ssh2-config 0.4
- Breaking: Support for multiple SSH backends. Support for libssh.org (remotefs-rs/remotefs-rs-ssh#10)

> - feat!: Support for multiple SSH backends. Support for libssh.org
>
> Added new feature to enable **libssh2** backend; Added a new feature to enable **libssh** backend. Renamed ssh2-vendored feature to libssh2-vendored. Removed `new`; use `libssh2` and `libssh` constructors instead. By default libssh2 feature is enabled. See README for more details

- add russh pure-Rust SSH backend

> Add a new `russh` feature providing a pure-Rust SSH backend using the
> russh, russh-keys, and russh-sftp crates. This eliminates the need for
> system C libraries (libssh2/libssh) when the russh backend is selected.
>
> The implementation includes:
>
> - SshSession and Sftp trait implementations for russh
> - SCP send/recv over russh channels with proper ACK handling
> - SFTP via russh-sftp
> - Authentication: password, public key, and SSH agent
> - Algorithm preference negotiation from SSH config
> - NoCheckServerKey default handler for server key verification
> - Benchmarks, examples, and full test coverage
>
> Closes remotefs-rs/remotefs-rs-ssh#15

- **ssh:** support connection attempts across backends
- **ssh:** support bind address across backends
- **ssh:** support bind interface across backends
- **ssh:** support TCP keepalive across backends
- **russh:** support SSH compression config
- **russh:** support host key certificates
- **ssh:** support public key authentication config
- **ssh:** support accepted public key algorithms
- **ssh:** support certificate files
- **russh:** support CA signature algorithms
- **russh:** support adding keys to agent
- **ssh:** support proxy jump connections
- **ssh:** support server alive intervals
- **ssh:** support agent forwarding
- **ssh:** support remote forwarding
- Breaking: migrate to remotefs 1 (remotefs-rs/remotefs-rs-ssh#26)

> - feat!: migrate to remotefs 1
>
> Migrate the blocking libssh2 and libssh clients to the remotefs 1 contract and add native asynchronous russh SFTP and SCP clients with blocking adapters.

- **ssh:** add home_dir option to SshOpts

> Pass the home directory to the ssh2-config parser so that `~` and relative `Include` paths resolve against it. When unset, ssh2-config falls back to `dirs::home_dir`.

- allow relative and dangling symlink targets (#41)

> symlink(path, target) no longer requires an absolute, existing target. The target is stored verbatim on every backend, so relative and dangling links can be created; only an empty target is rejected, with InvalidPath. The new remotefs::path::ensure_symlink_target helper implements that check. KubeMultiPodFs writes relative targets inside the link's container and keeps translating absolute targets within the same container.

### Changed

- **ssh:** replace chrono with jiff for file times (#42)

> The ssh and kube backends now use jiff to format the times they send to
> remote hosts and to read the times printed by remote ls. Dates users see
> do not change.
>
> A time past the latest date jiff supports used to panic. It now shows
> that latest date, and a time before the earliest supported date shows
> that date instead.
>
> chrono is still in the dependency graph through russh-util, which russh
> pulls in for wasm32 builds, so its advisory exception stays in deny.toml.

### Fixed

- Fixed relative paths resolve on Windows
- path-slash 2
- syntax
- bad code
- path must be resolved after absolutize
- docs
- tests
- resolved host from configuration wasn't used
- clippy
- resolved host from configuration wasn't used
- connect
- parse special permissions `StT` in ls output
- removed users dep
- test is sync and send
- ci
- ssh2-config 0.5
- label regex groups and add support for parsing ls output on systems that include SELinux labels, POSIX ACLs, and extended attributes (remotefs-rs/remotefs-rs-ssh#9)
- Fixed issue with SSH authentication

> if the key is resolved and it fails to authenticate, if a password is provided, try to authenticate with the password before returning an error

- capped read
- **sftp:** eliminate shell command dependency from SFTP client

> Replace shell commands (pwd, rm -rf, cp -rf) with pure SFTP protocol
> operations so the SFTP client no longer requires shell access on the
> remote server.
>
> - Use sftp.realpath(".") instead of cmd("pwd") for working directory
> - Implement recursive remove_dir_all via readdir/unlink/rmdir
> - Implement recursive copy via readdir/mkdir/open_read/open_write
> - Fix libssh realpath to use canonicalize (SSH_FXP_REALPATH) instead
>   of read_link (SSH_FXP_READLINK)
> - Fix libssh symlink resolution in readdir to use read_link directly

- **scp:** use stat epoch for mtime instead of TZ-ambiguous ls

> ls -l prints times in the server's local TZ without an offset, so
> parse_lstime treats the naive datetime as UTC. Downstream clients that
> render via DateTime<Local> then show UTC wall-clock instead of local.
> Only ScpFs is affected; SftpFs uses the protocol's epoch directly.
>
> Probe stat flavor once per session (GNU via stat --version, else BSD
> via stat -f %m /, else Unsupported) and cache on the session. list_dir
> overrides mtimes with one batched stat -c '%Y %n' / -f '%m %N' call
> keyed by basename; stat overrides with stat -c %Y / -f %m. Falls back
> to the ls parser on hosts exposing neither flavor.
>
> Closes termscp#416.

- bump russh 0.6.1
- **russh:** close SFTP handles with awaited shutdown to stop handle leak

> russh-sftp's `File::drop` closes handles via `close_nowait`, which frees
> the handle server-side but never decrements the client's open-handle
> counter. Reads (one handle per chunk) and writes (one handle per upload)
> relied on `Drop`, so the counter climbed monotonically until the
> negotiated limit was reached, failing later operations with
> `Limit exceeded: Handle limit reached`.
>
> Both paths now close handles via an awaited `shutdown`, which decrements
> the counter. The reader also moves open+seek into the spawned task.

- **russh:** authenticate RSA keys with rsa-sha2-256/512

> russh maps a `None` hash algorithm to the legacy `ssh-rsa` (SHA-1)
> signature, which OpenSSH 8.8+ rejects by default. RSA public key
> authentication therefore failed against modern servers with
> "public key authentication failed", while the libssh2/libssh backends
> negotiate rsa-sha2-256/512 transparently (termscp#422).
>
> RSA keys now attempt rsa-sha2-512, then rsa-sha2-256, then legacy
> ssh-rsa, stopping early if the server stops offering publickey auth.
> Non-RSA keys are unaffected (russh ignores the hash for them).
>
> The test container was bumped from OpenSSH 8.6 (which still accepts
> SHA-1, masking the bug) to 10.2 so the suite exercises modern signature
> negotiation. The unrealistic single-algo KexAlgorithms pin in the test
> ssh config was modernized accordingly. Added a regression test for key
> auth via the ssh config IdentityFile directive.

- **russh:** wire SSH agent authentication

> The russh backend never read `SshOpts::ssh_agent_identity`, so it
> silently ignored the SSH agent and fell straight through to key/password
> auth. Users whose keys live only in an ssh-agent (common on macOS) could
> not authenticate, unlike the libssh2/libssh backends (termscp#422).
>
> russh now authenticates via the agent first (agent -> key -> password),
> mirroring the other backends: it connects to the agent over
> `SSH_AUTH_SOCK`, enumerates identities, filters by `SshAgentIdentity`,
> and signs challenges through the agent. RSA identities request
> rsa-sha2-512/256 before legacy ssh-rsa, same as direct key auth. Agent
> auth is Unix-only; other platforms report it unavailable and fall
> through to the remaining methods.
>
> Added a testcontainers regression test that spawns an ssh-agent, loads
> the mock key, and authenticates through the agent alone.

- **libssh:** order symlink arguments correctly

> The libssh backend passed the link path and target to libssh-rs's
> symlink(target, dest) in the wrong order. OpenSSH's older sftp-server
> reversed the SSH_FXP_SYMLINK arguments, which cancelled the mistake;
> the 10.2 server now used by the test suite no longer does, so symlinks
> were created at the wrong path. Order the arguments correctly.

- **russh:** match the check_server_key signature and drop unused russh-keys

> Update Handler::check_server_key to accept russh 0.63's
> PublicKeyOrCertificate instead of the removed PublicKey parameter type.
> Also drop the unused russh-keys dependency, which pulled a
> vulnerable russh-cryptovec release (RUSTSEC-2026-0153); the crate
> already gets everything it needs from russh::keys.

- resolve clippy warnings under the all-features lint gate

> Build Metadata with struct-update syntax instead of a default plus
> field reassignment, drop needless borrows in the example list_dir
> calls, and pass BenchmarkCtx::new directly instead of wrapping it in
> a closure. just lint "-- -D warnings" was failing on all of these.

- **libssh:** reverse argument for `sftp::symlink` call. Path and target were reversed.
- **libssh:** preserve explicit port over ssh config
- **ssh:** honor connect timeout across backends
- **libssh2:** honor configured identity files
- **ssh:** use resolved endpoint for libssh
- **ssh:** harden timeouts and forwarded channels

> Treat zero connection timeouts as disabled across libssh and russh, including ProxyJump handshakes. Bound forwarded channels to prevent resource exhaustion and document the supported SSH configuration options.

- **ssh:** use forward slashes in remote entry paths on windows

> Since the move to remotefs 1, remote entry paths were built with the
> platform path join. On Windows that inserted a backslash, so listing /srv
> returned entries named srv\files that could not be opened. SCP listings,
> batched stat, SFTP directory listings on every backend and SFTP recursive
> copy now build remote paths with a helper that normalizes the directory
> with path-slash and appends the entry name after a forward slash.
>
> closes remotefs-rs/remotefs-rs-ssh#28

- **ssh:** accept backslash separators in remote paths

> Since 1.1.1 listed entries use forward slashes, but paths given to a
> client were still sent to the server as is. On Windows, a path built with
> Path::join such as /home/deck/.steam/steam\userdata reached the server with
> a literal backslash, so list_dir and every other operation failed on SCP
> and on SFTP over russh and libssh. Every client now treats a backslash in
> the paths it is given as a separator, on every platform as the SMB client
> does, and sends it as a forward slash. Remote names containing a literal
> backslash can no longer be reached.
>
> closes remotefs-rs/remotefs-rs-ssh#30

- Breaking: **remotefs-kube:** preserve symlink targets verbatim (#43)

> Apply the reviewed fixes for relative, absolute, and dangling targets across the symlink-capable clients.

### Performance

- buffer entire file in libssh SFTP reads to reduce round-trips

> Replace the streaming SftpFileReader (one SFTP round-trip per read()
> call) with a buffered approach that reads the entire file into memory
> using a 256 KiB buffer before returning. This pre-fetches the file data
> in open_read() rather than deferring reads to the caller's smaller
> 64 KiB buffer loop, reducing total round-trips.
>
> Unlike the russh backend which uses true concurrent pipelining (multiple
> file handles reading different chunks via tokio::spawn), this approach
> is still sequential due to libssh-rs serializing all operations through
> a session-level mutex. True pipelining would require the AIO FFI
> (sftp_aio_begin_read/sftp_aio_wait_read) which libssh-rs does not
> expose at the safe API level.

- **russh:** streaming PipelinedSftpReader with bounded pre-fetch

> Replace the fully-buffered pipelined_sftp_read with a streaming
> PipelinedSftpReader struct that fetches 16 MiB batches (4 concurrent
> 4 MiB chunks) and pre-fetches up to 2 batches ahead. This caps memory
> at ~48 MiB regardless of file size while keeping pipelined throughput.
>
> Adds a 20 MiB byte-order test to verify data arrives sequentially.

- **russh:** use `try_update` instead of `fetch_update`

> also bumps MSRV to 1.95.0

### Build

- ssh2-config 0.6.0
- ssh2-config 0.7
- 0.7.2
- bump russh, testcontainers, and rust-version

> Move russh to 0.63, testcontainers to 0.28, and the published MSRV to
> 1.89.0; relax the path-slash pin to a bare minimal version.

- bump dependencies

> bump `russh` to `3`, bump `ssh2-config` to `0.9`

- **release:** bump every workspace crate version
- unify crates in a cargo workspace at version 1.2.0

> Every member crate shares version 1.2.0 and inherits its package metadata
> from the workspace root. Every third-party dependency version now lives in
> the root `[workspace.dependencies]` table, and members reference it with
> `workspace = true`, keeping only their own features and optional flags.
>
> The lockfile is committed so the workspace resolves the same versions
> everywhere. Cargo.lock is no longer ignored, and the toolchain is pinned to
> 1.100.0.

- **msrv:** bump msrv to 1.98
- inherit the workspace lints in every crate

> Eight crates lacked a [lints] table, so the workspace lint rules were
> never applied to them and cargo warned about the missing inheritance.
>
> Enabling them surfaced the findings they had been hiding:
>
> - remove imports already provided by the prelude or a glob import
> - implement Debug for the public clients and SSH options; secrets such
>   as passwords and key storage are left out of the output
> - use expect_err in a remotefs-smb test now that its Ok type is Debug

### Style

- Lint
- reformat markdown and rust sources with dprint

> Run just fmt over the repository. No behavioural change.
