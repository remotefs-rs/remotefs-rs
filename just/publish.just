# Package and publish one crate to crates.io.
[group('publish')]
publish crate args="":
  cargo publish --locked -p {{ crate }} {{ args }}

# Publish every workspace crate to crates.io. Cargo publishes in dependency order
# and waits for each crate to reach the index before the crates that depend on it.
[group('publish')]
publish_all args="":
  cargo publish --workspace --locked {{ args }}

# List files included in the packaged crate.
[group('publish')]
package_list args="":
  cargo package --list --exclude-lockfile {{ args }}
