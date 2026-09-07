# Package and publish the crate to crates.io.
[group('publish')]
publish args="":
  cargo publish --locked {{ args }}
