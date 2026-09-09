# Package and publish the crate to crates.io.
[group('publish')]
publish args="":
  cargo publish --locked {{ args }}

# List files included in the packaged crate.
[group('publish')]
package_list args="":
  cargo package --list --exclude-lockfile {{ args }}
