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
    RsaKey {
        private_key: PathBuf,
        certificate: Option<PathBuf>,
    },
    Password(String),
}

/// Local private-key signer for russh's hash-selecting certificate API.
struct PrivateKeySigner {
    private_key: Arc<russh::keys::PrivateKey>,
}

#[derive(Debug)]
struct PrivateKeySignerError(String);

impl std::fmt::Display for PrivateKeySignerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PrivateKeySignerError {}

impl From<russh::SendError> for PrivateKeySignerError {
    fn from(err: russh::SendError) -> Self {
        Self(err.to_string())
    }
}

impl russh::Signer for PrivateKeySigner {
    type Error = PrivateKeySignerError;

    fn auth_sign(
        &mut self,
        _key: &russh::keys::agent::AgentIdentity,
        hash_alg: Option<russh::keys::HashAlg>,
        to_sign: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, Self::Error>> + Send {
        std::future::ready(sign_with_private_key(
            self.private_key.as_ref(),
            hash_alg,
            to_sign,
        ))
    }
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
    let pubkey_authentication = ssh_config.params.pubkey_authentication.unwrap_or(true);

    // Authentication order mirrors the libssh2/libssh backends: SSH agent first,
    // then key, then password.
    if pubkey_authentication && let Some(agent_identity) = opts.ssh_agent_identity.as_ref() {
        match auth_with_agent(
            session,
            runtime,
            username,
            agent_identity,
            ssh_config.params.pubkey_accepted_algorithms.algorithms(),
        ) {
            Ok(()) => {
                info!("Authenticated with ssh agent");
                return Ok(());
            }
            Err(err) => {
                error!("Could not authenticate with ssh agent: {err}");
            }
        }
    }

    // Collect authentication methods in priority order: RSA key, then password
    let mut methods = vec![];

    if pubkey_authentication {
        if let Some(rsa_key) = opts.key_storage.as_ref().and_then(|x| {
            x.resolve(ssh_config.host.as_str(), username.as_str())
                .or(x.resolve(ssh_config.resolved_host.as_str(), username.as_str()))
        }) {
            methods.push(Authentication::RsaKey {
                private_key: rsa_key.clone(),
                certificate: ssh_config.params.certificate_file.clone(),
            });
        }

        // Add identity files from config
        if let Some(identity_files) = ssh_config.params.identity_file.as_ref() {
            for identity_file in identity_files {
                methods.push(Authentication::RsaKey {
                    private_key: identity_file.clone(),
                    certificate: ssh_config.params.certificate_file.clone(),
                });
            }
        }
    }

    if let Some(password) = opts.password.as_ref() {
        methods.push(Authentication::Password(password.clone()));
    }

    let mut last_err = None;
    for auth_method in methods {
        match auth_method {
            Authentication::RsaKey {
                private_key: key_path,
                certificate,
            } => {
                match auth_with_rsa_key(
                    session,
                    runtime,
                    username,
                    &key_path,
                    certificate.as_deref(),
                    opts.password.as_deref(),
                    ssh_config.params.pubkey_accepted_algorithms.algorithms(),
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

/// Return the configured signature hashes accepted for a public key algorithm.
fn accepted_hash_algorithms(
    key_algorithm: &russh::keys::Algorithm,
    accepted_algorithms: &[String],
    certificate: bool,
) -> Vec<Option<russh::keys::HashAlg>> {
    if matches!(key_algorithm, russh::keys::Algorithm::Rsa { .. }) {
        accepted_algorithms
            .iter()
            .filter_map(|algorithm| match (certificate, algorithm.as_str()) {
                (false, "rsa-sha2-512") => Some(Some(russh::keys::HashAlg::Sha512)),
                (false, "rsa-sha2-256") => Some(Some(russh::keys::HashAlg::Sha256)),
                // russh 0.63 always advertises a certificate's embedded
                // `ssh-rsa-cert` name and cannot advertise its SHA-2 variants.
                (false, "ssh-rsa") | (true, "ssh-rsa-cert-v01@openssh.com") => Some(None),
                _ => None,
            })
            .collect()
    } else {
        let key_algorithm = if certificate {
            key_algorithm.to_certificate_type()
        } else {
            key_algorithm.as_ref().to_string()
        };
        if !accepted_algorithms.contains(&key_algorithm) {
            return Vec::new();
        }
        vec![None]
    }
}

/// Append an SSH signature using the selected RSA hash when applicable.
fn sign_with_private_key(
    private_key: &russh::keys::PrivateKey,
    hash_alg: Option<russh::keys::HashAlg>,
    mut to_sign: Vec<u8>,
) -> Result<Vec<u8>, PrivateKeySignerError> {
    use russh::keys::signature::Signer as _;
    use russh::keys::ssh_encoding::Encode as _;

    let signature = match private_key.key_data() {
        russh::keys::ssh_key::private::KeypairData::Rsa(keypair) => {
            (keypair, hash_alg).try_sign(&to_sign)
        }
        _ => private_key.try_sign(&to_sign),
    }
    .map_err(|err| PrivateKeySignerError(err.to_string()))?;
    let mut encoded_signature = Vec::new();
    signature
        .encode(&mut encoded_signature)
        .map_err(|err| PrivateKeySignerError(err.to_string()))?;
    encoded_signature
        .encode(&mut to_sign)
        .map_err(|err| PrivateKeySignerError(err.to_string()))?;
    Ok(to_sign)
}

/// Authenticate with an RSA private key file.
fn auth_with_rsa_key<T>(
    session: &mut Handle<T>,
    runtime: &Runtime,
    username: &str,
    key_path: &Path,
    certificate_path: Option<&Path>,
    passphrase: Option<&str>,
    accepted_algorithms: &[String],
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

    if let Some(certificate_path) = certificate_path {
        let certificate =
            russh::keys::load_openssh_certificate(certificate_path).map_err(|err| {
                RemoteError::new_ex(
                    RemoteErrorType::AuthenticationFailed,
                    format!(
                        "Could not load certificate at '{}': {err}",
                        certificate_path.display()
                    ),
                )
            })?;
        let hash_algs =
            accepted_hash_algorithms(&certificate.algorithm(), accepted_algorithms, true);
        if hash_algs.is_empty() {
            return Err(RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                format!(
                    "certificate algorithm {} is not accepted by SSH config",
                    certificate.algorithm().to_certificate_type()
                ),
            ));
        }

        let mut signer = PrivateKeySigner {
            private_key: private_key.clone(),
        };
        let mut last_failure = None;
        for hash_alg in hash_algs {
            let auth_result = runtime
                .block_on(async {
                    session
                        .authenticate_certificate_with(
                            username,
                            certificate.clone(),
                            hash_alg,
                            &mut signer,
                        )
                        .await
                })
                .map_err(|err| RemoteError::new_ex(RemoteErrorType::AuthenticationFailed, err))?;
            match auth_result {
                russh::client::AuthResult::Success => return Ok(()),
                russh::client::AuthResult::Failure {
                    remaining_methods, ..
                } => {
                    let pubkey_still_offered =
                        remaining_methods.contains(&russh::MethodKind::PublicKey);
                    last_failure = Some(remaining_methods);
                    if !pubkey_still_offered {
                        break;
                    }
                }
            }
        }
        return Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!(
                "certificate authentication failed for key at '{}' (remaining methods: {last_failure:?})",
                key_path.display()
            ),
        ));
    }

    let key_algorithm = private_key.algorithm();
    let hash_algs = accepted_hash_algorithms(&key_algorithm, accepted_algorithms, false);
    if hash_algs.is_empty() {
        return Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            format!("public key algorithm {key_algorithm} is not accepted by SSH config"),
        ));
    }

    let mut last_failure = None;
    for hash_alg in hash_algs {
        let key_with_hash = russh::keys::PrivateKeyWithHashAlg::new(private_key.clone(), hash_alg);

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

/// Authenticate with the SSH agent, letting it sign the challenges.
///
/// The agent socket is resolved from the `SSH_AUTH_SOCK` environment variable.
#[cfg(unix)]
fn auth_with_agent<T>(
    session: &mut Handle<T>,
    runtime: &Runtime,
    username: &str,
    identity: &crate::SshAgentIdentity,
    accepted_algorithms: &[String],
) -> RemoteResult<()>
where
    T: Handler,
{
    use russh::keys::agent::client::AgentClient;

    debug!("Authenticating with username '{username}' via ssh agent");

    runtime.block_on(async {
        let mut agent = AgentClient::connect_env().await.map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                format!("could not connect to ssh agent: {err}"),
            )
        })?;

        let identities = agent.request_identities().await.map_err(|err| {
            RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                format!("could not list ssh agent identities: {err}"),
            )
        })?;

        let mut last_err = None;
        for agent_identity in identities {
            let pubkey = agent_identity.public_key().into_owned();
            let (blob, key_algorithm, certificate) = match &agent_identity {
                russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                    (key.to_bytes().unwrap_or_default(), key.algorithm(), false)
                }
                russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => (
                    certificate.to_bytes().unwrap_or_default(),
                    certificate.algorithm(),
                    true,
                ),
            };
            if !identity.pubkey_matches(&blob) {
                continue;
            }
            debug!(
                "Trying to authenticate with ssh agent identity: {}",
                pubkey.fingerprint(russh::keys::HashAlg::Sha256)
            );

            let hash_algs =
                accepted_hash_algorithms(&key_algorithm, accepted_algorithms, certificate);

            for hash_alg in hash_algs {
                let auth_result = match &agent_identity {
                    russh::keys::agent::AgentIdentity::PublicKey { key, .. } => {
                        session
                            .authenticate_publickey_with(
                                username,
                                key.clone(),
                                hash_alg,
                                &mut agent,
                            )
                            .await
                    }
                    russh::keys::agent::AgentIdentity::Certificate { certificate, .. } => {
                        session
                            .authenticate_certificate_with(
                                username,
                                certificate.clone(),
                                hash_alg,
                                &mut agent,
                            )
                            .await
                    }
                };
                match auth_result {
                    Ok(russh::client::AuthResult::Success) => return Ok(()),
                    Ok(russh::client::AuthResult::Failure {
                        remaining_methods, ..
                    }) => {
                        debug!(
                            "ssh agent auth with hash {hash_alg:?} failed; remaining methods: {remaining_methods:?}"
                        );
                        let pubkey_still_offered =
                            remaining_methods.contains(&russh::MethodKind::PublicKey);
                        last_err = Some(RemoteError::new_ex(
                            RemoteErrorType::AuthenticationFailed,
                            "ssh agent authentication failed",
                        ));
                        if !pubkey_still_offered {
                            break;
                        }
                    }
                    Err(err) => {
                        debug!("ssh agent auth signing error: {err}");
                        last_err = Some(RemoteError::new_ex(
                            RemoteErrorType::AuthenticationFailed,
                            format!("ssh agent signing failed: {err}"),
                        ));
                        break;
                    }
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                "ssh agent provided no usable identity",
            )
        }))
    })
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::mock::ssh as ssh_mock;

    #[test]
    fn should_distinguish_plain_and_certificate_agent_algorithms() {
        let rsa = russh::keys::Algorithm::Rsa { hash: None };
        let ed25519 = russh::keys::Algorithm::Ed25519;
        let plain = vec!["rsa-sha2-256".to_string()];
        let certificate = vec!["rsa-sha2-256-cert-v01@openssh.com".to_string()];
        let legacy_certificate = vec!["ssh-rsa-cert-v01@openssh.com".to_string()];
        let ed25519_plain = vec!["ssh-ed25519".to_string()];
        let ed25519_certificate = vec!["ssh-ed25519-cert-v01@openssh.com".to_string()];

        assert!(accepted_hash_algorithms(&rsa, &plain, true).is_empty());
        assert!(accepted_hash_algorithms(&rsa, &certificate, false).is_empty());
        assert!(accepted_hash_algorithms(&rsa, &certificate, true).is_empty());
        assert_eq!(
            accepted_hash_algorithms(&rsa, &legacy_certificate, true),
            vec![None]
        );
        assert!(accepted_hash_algorithms(&ed25519, &ed25519_plain, true).is_empty());
        assert!(accepted_hash_algorithms(&ed25519, &ed25519_certificate, false).is_empty());
        assert_eq!(
            accepted_hash_algorithms(&ed25519, &ed25519_certificate, true),
            vec![None]
        );
    }

    #[test]
    fn should_sign_legacy_rsa_certificate_with_ssh_rsa() {
        let key_file = ssh_mock::create_key_file();
        let private_key =
            russh::keys::load_secret_key(key_file.path(), None).expect("failed to load RSA key");
        let message = b"certificate authentication payload".to_vec();

        let signed = sign_with_private_key(&private_key, None, message.clone())
            .expect("failed to sign certificate authentication payload");
        let algorithm_length_offset = message.len() + 4;
        let algorithm_offset = algorithm_length_offset + 4;
        let algorithm_length = u32::from_be_bytes(
            signed[algorithm_length_offset..algorithm_offset]
                .try_into()
                .expect("signature algorithm length must be four bytes"),
        ) as usize;

        assert_eq!(
            &signed[algorithm_offset..algorithm_offset + algorithm_length],
            b"ssh-rsa"
        );
    }
}

/// The SSH agent is only reachable over a Unix socket; on other platforms this is a no-op
/// that simply reports the agent as unavailable so the remaining methods are tried.
#[cfg(not(unix))]
fn auth_with_agent<T>(
    _session: &mut Handle<T>,
    _runtime: &Runtime,
    _username: &str,
    _identity: &crate::SshAgentIdentity,
    _accepted_algorithms: &[String],
) -> RemoteResult<()>
where
    T: Handler,
{
    Err(RemoteError::new_ex(
        RemoteErrorType::AuthenticationFailed,
        "ssh agent authentication is not supported on this platform for the russh backend",
    ))
}
