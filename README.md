# remotefs

<p align="center">
  <img src="assets/logo.png" alt="logo" width="256" height="256" />
</p>

<p align="center">~ The Omni Filetransfer Client Library (and more!) ~</p>

<p align="center">Developed by <a href="https://veeso.github.io/" target="_blank">@veeso</a></p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-teal.svg" alt="License-MIT" /></a>
  <a href="https://github.com/veeso/remotefs-rs/stargazers"><img src="https://img.shields.io/github/stars/veeso/remotefs-rs.svg?style=plain" alt="Repo stars" /></a>
  <a href="https://ko-fi.com/veeso"><img src="https://img.shields.io/badge/donate-ko--fi-red" alt="Ko-fi" /></a>
</p>

<p align="center">
  <a href="https://github.com/veeso/remotefs-rs/actions/workflows/ci.yml"><img src="https://github.com/veeso/remotefs-rs/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://coveralls.io/github/veeso/remotefs-rs"><img src="https://coveralls.io/repos/github/veeso/remotefs-rs/badge.svg" alt="Coveralls" /></a>
</p>

## Crates

This repository holds every remotefs crate. Each crate lives under `crates/`, has
its own README, and is released with the same version number as the others.

| Crate             | Description                                                   | Links                                                                                                                                 |
| ----------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| `remotefs`        | A client library to work with all file transfer protocols.    | [crates.io](https://crates.io/crates/remotefs) · [docs.rs](https://docs.rs/remotefs) · [source](crates/remotefs)                      |
| `remotefs-aws-s3` | AWS S3 client library.                                        | [crates.io](https://crates.io/crates/remotefs-aws-s3) · [docs.rs](https://docs.rs/remotefs-aws-s3) · [source](crates/remotefs-aws-s3) |
| `remotefs-ftp`    | FTP client library.                                           | [crates.io](https://crates.io/crates/remotefs-ftp) · [docs.rs](https://docs.rs/remotefs-ftp) · [source](crates/remotefs-ftp)          |
| `remotefs-fuse`   | FUSE driver that mounts any remotefs backend.                 | [crates.io](https://crates.io/crates/remotefs-fuse) · [docs.rs](https://docs.rs/remotefs-fuse) · [source](crates/remotefs-fuse)       |
| `remotefs-gcs`    | Google Cloud Storage client library.                          | [crates.io](https://crates.io/crates/remotefs-gcs) · [docs.rs](https://docs.rs/remotefs-gcs) · [source](crates/remotefs-gcs)          |
| `remotefs-kube`   | Kubernetes pod file system client library.                    | [crates.io](https://crates.io/crates/remotefs-kube) · [docs.rs](https://docs.rs/remotefs-kube) · [source](crates/remotefs-kube)       |
| `remotefs-memory` | In-memory client, for testing and simulation.                 | [crates.io](https://crates.io/crates/remotefs-memory) · [docs.rs](https://docs.rs/remotefs-memory) · [source](crates/remotefs-memory) |
| `remotefs-smb`    | SMB client library.                                           | [crates.io](https://crates.io/crates/remotefs-smb) · [docs.rs](https://docs.rs/remotefs-smb) · [source](crates/remotefs-smb)          |
| `remotefs-ssh`    | SSH client library, with SCP and SFTP.                        | [crates.io](https://crates.io/crates/remotefs-ssh) · [docs.rs](https://docs.rs/remotefs-ssh) · [source](crates/remotefs-ssh)          |
| `remotefs-webdav` | WebDAV client library.                                        | [crates.io](https://crates.io/crates/remotefs-webdav) · [docs.rs](https://docs.rs/remotefs-webdav) · [source](crates/remotefs-webdav) |
| `fusibile`        | Command line program that mounts remote file systems locally. | [crates.io](https://crates.io/crates/fusibile) · [docs.rs](https://docs.rs/fusibile) · [source](crates/fusibile)                      |

## Workspace layout

- `crates/`: one folder per crate.
- `dist/`: release scripts, install scripts, and the install page.
- `docs/`: the [release guide](docs/release.md).
- `just/`: task recipes. The `Justfile` imports them.
- `.github/`: workflows and the helper scripts they call.

## Development

The toolchain is pinned in `rust-toolchain.toml`. Run `just` to list every recipe.

```sh
just build            # build every crate
just test             # unit and doc tests, one crate at a time
just test_containers  # every test suite, including the ones that need Docker
just check            # the full local quality gate
```

Some crates need system libraries. Each one is listed below.

| Crate                       | System libraries                                               |
| --------------------------- | -------------------------------------------------------------- |
| `remotefs-fuse`, `fusibile` | FUSE: `libfuse3` on Linux, macFUSE on macOS, Dokany on Windows |
| `remotefs-smb`              | `libsmbclient` for the `pavao` client on UNIX                  |
| `remotefs-ssh`              | OpenSSL for the default libssh2 backend                        |
| most backends               | Docker, for the tests in `test_containers`                     |

On a machine without FUSE, set `SKIP_CRATES` to leave those crates out of
`just test`, for example `SKIP_CRATES="fusibile remotefs-fuse" just test`.

## Install fusibile

On Linux and macOS:

```sh
curl -sSLf https://veeso.github.io/remotefs-rs/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://veeso.github.io/remotefs-rs/install.ps1 | iex
```

## Release

Every crate shares one version. The [release guide](docs/release.md) describes how
a release is cut and what has to be configured before the first one.

## License

remotefs is released under the [MIT license](LICENSE).
