# Changelog

- [Changelog](#changelog)
  - [0.1.1](#011)
  - [0.1.0](#010)

---

## 0.1.1

Released on 09/12/2021

- Allow to build `RemoteFs` as trait object
- ❗ Breaking changes:
  - Changed signature of `open_file` to accept a `Box<dyn Write + Send>` instead of `impl Write + Send`

## 0.1.0

Released on 08/12/2021

- First release
