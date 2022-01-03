# Changelog

- [Changelog](#changelog)
  - [0.2.0](#020)
  - [0.1.1](#011)
  - [0.1.0](#010)

---

## 0.2.0

Released on ??

- Moved protocols to **extern crates**:
  - [aws-s3](https://github.com/veeso/remotefs-rs-aws-s3)
  - [ftp](https://github.com/veeso/remotefs-rs-ftp)
  - [ssh](https://github.com/veeso/remotefs-rs-ssh)
- Merged `File`, `Directory` and `Entry` into a unique struct called `File`. File types (symlink, file, directory) are now differentiated by the `file_type` attribute in `Metadata`.
- `find` method is now optional, via the `find` feature (enabled by default)
- Implemented `From` trait for `Metadata`.

## 0.1.1

Released on 09/12/2021

- Allow to build `RemoteFs` as trait object
- ❗ Breaking changes:
  - Changed signature of `open_file` to accept a `Box<dyn Write + Send>` instead of `impl Write + Send`

## 0.1.0

Released on 08/12/2021

- First release
