//! Authentication logic for the russh backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use russh::client::{Handle, Handler};
use tokio::runtime::Runtime;

use crate::SshOpts;
use crate::ssh::config::Config;

/// Authentication method for russh backend.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Authentication {
    RsaKey(PathBuf),
    Password(String),
}

/// Authenticate a russh session using the configured methods.
pub(super) fn authenticate<T>(
    session: &mut Handle<T>,
    runtime: &Runtime,
    opts: &SshOpts,
    ssh_config: &Config,
) -> RemoteResult<()>
where
    T: Handler,
{
    let username = &ssh_config.username;

    // Collect authentication methods in priority order: RSA key, then password
    let mut methods = vec![];

    if let Some(rsa_key) = opts.key_storage.as_ref().and_then(|x| {
        x.resolve(ssh_config.host.as_str(), username.as_str())
            .or(x.resolve(ssh_config.resolved_host.as_str(), username.as_str()))
    }) {
        methods.push(Authentication::RsaKey(rsa_key.clone()));
    }

    // Add identity files from config
    if let Some(identity_files) = ssh_config.params.identity_file.as_ref() {
        for identity_file in identity_files {
            methods.push(Authentication::RsaKey(identity_file.clone()));
        }
    }

    if let Some(password) = opts.password.as_ref() {
        methods.push(Authentication::Password(password.clone()));
    }

    let mut last_err = None;
    for auth_method in methods {
        match auth_method {
            Authentication::RsaKey(key_path) => {
                match auth_with_rsa_key(
                    session,
                    runtime,
                    username,
                    &key_path,
                    opts.password.as_deref(),
                ) {
                    Ok(()) => {
                        info!("Authenticated with key at '{}'", key_path.display());
                        return Ok(());
                    }
                    Err(err) => {
                        error!(
                            "Authentication with key at '{}' failed: {err}",
                            key_path.display()
                        );
                        last_err = Some(err);
                    }
                }
            }
            Authentication::Password(password) => {
                match auth_with_password(session, runtime, username, &password) {
                    Ok(()) => {
                        info!("Authenticated with password");
                        return Ok(());
                    }
                    Err(err) => {
                        error!("Password authentication failed: {err}");
                        last_err = Some(err);
                    }
                }
            }
        }
    }

    Err(last_err.unwrap_or_else(|| {
        RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            "no authentication method provided",
        )
    }))
}

/// Authenticate with an RSA private key file.
fn auth_with_rsa_key<T>(
    session: &mut Handle<T>,
    runtime: &Runtime,
    username: &str,
    key_path: &Path,
    passphrase: Option<&str>,
) -> RemoteResult<()>
where
    T: Handler,
{
    debug!(
        "Authenticating with username '{username}' and key at '{}'",
        key_path.display()
    );

    let private_key = russh::keys::load_secret_key(key_path, passphrase).map_err(|err| {
        RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!(
                "Could not load private key at '{}': {err}",
                key_path.display()
            ),
        )
    })?;
    let private_key = Arc::new(private_key);

    // For RSA keys, `None` maps to the legacy `ssh-rsa` (SHA-1) signature, which modern
    // OpenSSH servers reject by default. Try the modern `rsa-sha2-512` / `rsa-sha2-256`
    // signature algorithms first, falling back to SHA-1 for legacy servers.
    // For non-RSA keys the hash is ignored, so a single `None` attempt is enough.
    let hash_algs: &[Option<russh::keys::HashAlg>] = if private_key.algorithm().is_rsa() {
        &[
            Some(russh::keys::HashAlg::Sha512),
            Some(russh::keys::HashAlg::Sha256),
            None,
        ]
    } else {
        &[None]
    };

    let mut last_failure = None;
    for hash_alg in hash_algs {
        let key_with_hash =
            russh::keys::PrivateKeyWithHashAlg::new(private_key.clone(), *hash_alg);

        let auth_result = runtime
            .block_on(async {
                session
                    .authenticate_publickey(username, key_with_hash)
                    .await
            })
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, err))?;

        match auth_result {
            russh::client::AuthResult::Success => return Ok(()),
            russh::client::AuthResult::Failure {
                remaining_methods, ..
            } => {
                debug!(
                    "public key authentication with hash {hash_alg:?} failed for key at '{}'; remaining methods: {remaining_methods:?}",
                    key_path.display()
                );
                // If the server no longer offers public key auth, stop retrying hashes.
                let pubkey_still_offered =
                    remaining_methods.contains(&russh::MethodKind::PublicKey);
                last_failure = Some(remaining_methods);
                if !pubkey_still_offered {
                    break;
                }
            }
        }
    }

    Err(RemoteError::new_ex(
        RemoteErrorType::AuthenticationFailed,
        format!(
            "public key authentication failed for key at '{}' (remaining methods: {last_failure:?})",
            key_path.display()
        ),
    ))
}

/// Authenticate with username and password.
fn auth_with_password<T>(
    session: &mut Handle<T>,
    runtime: &Runtime,
    username: &str,
    password: &str,
) -> RemoteResult<()>
where
    T: Handler,
{
    debug!("Authenticating with username '{username}' and password");

    let auth_result = runtime
        .block_on(async { session.authenticate_password(username, password).await })
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, err))?;

    match auth_result {
        russh::client::AuthResult::Success => Ok(()),
        russh::client::AuthResult::Failure { .. } => Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            "password authentication failed",
        )),
    }
}
