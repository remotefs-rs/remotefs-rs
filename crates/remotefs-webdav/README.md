# Rust-template

[![license-mit](https://img.shields.io/badge/License-MIT-teal.svg)](https://opensource.org/license/mit/)
[![build-test](https://github.com/veeso-dev/rust-template/actions/workflows/build-test.yml/badge.svg)](https://github.com/veeso-dev/rust-template/actions/workflows/build-test.yml)
[![downloads](https://img.shields.io/crates/d/rust-template.svg)](https://crates.io/crates/rust-template)
[![latest version](https://img.shields.io/crates/v/rust-template.svg)](https://crates.io/crates/rust-template)
[![Conventional Commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white)](https://conventionalcommits.org)

---

- [Rust-template](#rust-template)
  - [About rust-template](#about-rust-template)
  - [Get started](#get-started)
    - [Setup env](#setup-env)
    - [Run with Cargo make](#run-with-cargo-make)
  - [rust-template API](#rust-template-api)
    - [Check](#check)
  - [Contributing and issues](#contributing-and-issues)
  - [Changelog](#changelog)
  - [License](#license)

---

## About rust-template

rust-template is a Rust web service which comes integrated with ClamAV. The service provides an API endpoint to scan files with ClamAV.

---

## Get started

### Setup env

```sh
cp .env.test .env
vim .env
```

```env
WEB_PORT=3001
```

### Run with Cargo make

```sh
cargo make -p production run
```

## rust-template API

### Check

Check web service status:

```txt
GET /check
```

Response: Empty (200)

---

## Contributing and issues

Contributions, bug reports, new features and questions are welcome! 😉
If you have any question or concern, or you want to suggest a new feature, or you want just want to improve pavao, feel free to open an issue or a PR.

Please follow [our contributing guidelines](CONTRIBUTING.md)

---

## Changelog

View rust-template's changelog [HERE](CHANGELOG.md)

---

## License

rust-template is licensed under the MIT license.

You can read the entire license [HERE](LICENSE)
