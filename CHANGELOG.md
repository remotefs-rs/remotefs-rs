# Changelog

- [Changelog](#changelog)
  - [0.2.0](#020)
  - [0.1.1](#011)
  - [0.1.0](#010)

---

## 0.2.0

Released on 04/01/2022

- Moved protocols to **extern crates**:
  - [aws-s3](https://github.com/veeso/remotefs-rs-aws-s3)
  - [ftp](https://github.com/veeso/remotefs-rs-ftp)
  - [ssh](https://github.com/veeso/remotefs-rs-ssh)
- Merged `File`, `Directory` and `Entry` into a unique struct called `File`. File types (symlink, file, directory) are now differentiated by the `file_type` attribute in `Metadata`.
- `find` method is now optional, via the `find` feature (enabled by default)
- Implemented `From` trait for `Metadata`.
- `create` and `append` will now return a `WriteStream` instead of a box, which will contain the inner stream which supports `Write` and may support `Seek` (according to the protocol).
- `read` will now return a `ReadStream` instead of a box, which will contain the inner stream which supports `Read` and may support `Seek` (according to the protocol).
- Metadata times (`created`, `accessed` and `modified`) are now `Option<SystemTime>` in order to provide the user to handle unset times, in case it is not supported by the remote server.
- `append_file`, `create_file` and `open_file` will now return the amount of bytes transferred between the client and the server

## 0.1.1

Released on 09/12/2021

- Allow building `RemoteFs` as a trait object
- ❗ Breaking changes:
  - Changed signature of `open_file` to accept a `Box<dyn Write + Send>` instead of `impl Write + Send`

## 0.1.0

Released on 08/12/2021

- First release
