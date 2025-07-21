//! ## Commons
//!
//! SSH2 common methods

use std::io::Read;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use ssh2::{MethodType as SshMethodType, Session};

use super::SshOpts;
use super::config::Config;
use crate::SshAgentIdentity;

/// Authentication method
#[derive(Debug, Clone, PartialEq, Eq)]
enum Authentication {
    RsaKey(PathBuf),
    Password(String),
}

// -- connect

/// Establish connection with remote server and in case of success, return the generated [`Session`]
pub fn connect(opts: &SshOpts) -> RemoteResult<Session> {
    // parse configuration
    let ssh_config = Config::try_from(opts)?;
    // Resolve host
    debug!("Connecting to '{}'", ssh_config.address);
    // setup tcp stream
    let socket_addresses: Vec<SocketAddr> = match ssh_config.address.to_socket_addrs() {
        Ok(s) => s.collect(),
        Err(err) => {
            return Err(RemoteError::new_ex(
                RemoteErrorType::BadAddress,
                err.to_string(),
            ));
        }
    };
    let mut stream = None;
    for _ in 0..ssh_config.connection_attempts {
        for socket_addr in socket_addresses.iter() {
            trace!(
                "Trying to connect to socket address '{}' (timeout: {}s)",
                socket_addr,
                ssh_config.connection_timeout.as_secs()
            );
            if let Ok(tcp_stream) = tcp_connect(socket_addr, ssh_config.connection_timeout) {
                debug!("Connection established with address {}", socket_addr);
                stream = Some(tcp_stream);
                break;
            }
        }
        // break from attempts cycle if some
        if stream.is_some() {
            break;
        }
    }
    // If stream is None, return connection timeout
    let stream = match stream {
        Some(s) => s,
        None => {
            error!("No suitable socket address found; connection timeout");
            return Err(RemoteError::new_ex(
                RemoteErrorType::ConnectionError,
                "connection timeout",
            ));
        }
    };
    // Create session
    let mut session = match Session::new() {
        Ok(s) => s,
        Err(err) => {
            error!("Could not create session: {}", err);
            return Err(RemoteError::new_ex(RemoteErrorType::ConnectionError, err));
        }
    };
    // Set TCP stream
    session.set_tcp_stream(stream);
    // configure algos
    set_algo_prefs(&mut session, opts, &ssh_config)?;
    // Open connection and initialize handshake
    if let Err(err) = session.handshake() {
        error!("SSH handshake failed: {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // if use_ssh_agent is enabled, try to authenticate with ssh agent
    if let Some(ssh_agent_config) = &opts.ssh_agent_identity {
        match session_auth_with_agent(&mut session, &ssh_config.username, ssh_agent_config) {
            Ok(_) => {
                info!("Authenticated with ssh agent");
                return Ok(session);
            }
            Err(err) => {
                error!("Could not authenticate with ssh agent: {}", err);
            }
        }
    }

    // Authenticate with password or key
    if !session.authenticated() {
        let mut methods = vec![];
        // first try with ssh agent
        if let Some(rsa_key) = opts.key_storage.as_ref().and_then(|x| {
            x.resolve(ssh_config.host.as_str(), ssh_config.username.as_str())
                .or(x.resolve(
                    ssh_config.resolved_host.as_str(),
                    ssh_config.username.as_str(),
                ))
        }) {
            methods.push(Authentication::RsaKey(rsa_key.clone()));
        }
        // then try with password
        if let Some(password) = opts.password.as_ref() {
            methods.push(Authentication::Password(password.clone()));
        }

        // try with methods
        let mut last_err = None;
        for auth_method in methods {
            match session_auth(&mut session, opts, &ssh_config, auth_method) {
                Ok(_) => {
                    info!("Authenticated successfully");
                    return Ok(session);
                }
                Err(err) => {
                    error!("Authentication failed: {err}",);
                    last_err = Some(err);
                }
            }
        }

        return Err(match last_err {
            Some(err) => err,
            None => RemoteError::new_ex(
                RemoteErrorType::AuthenticationFailed,
                "no authentication method provided",
            ),
        });
    }
    // Return session
    Ok(session)
}

/// connect to socket address with provided timeout.
/// If timeout is zero, don't set timeout
fn tcp_connect(address: &SocketAddr, timeout: Duration) -> std::io::Result<TcpStream> {
    if timeout.is_zero() {
        TcpStream::connect(address)
    } else {
        TcpStream::connect_timeout(address, timeout)
    }
}

/// Configure algorithm preferences into session
fn set_algo_prefs(session: &mut Session, opts: &SshOpts, config: &Config) -> RemoteResult<()> {
    // Configure preferences from config
    let params = &config.params;
    trace!("Configuring algorithm preferences...");
    if let Some(compress) = params.compression {
        trace!("compression: {}", compress);
        session.set_compress(compress);
    }

    // kex
    let algos = params.kex_algorithms.algorithms().join(",");
    trace!("Configuring KEX algorithms: {}", algos);
    if let Err(err) = session.method_pref(SshMethodType::Kex, algos.as_str()) {
        error!("Could not set KEX algorithms: {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // HostKey
    let algos = params.host_key_algorithms.algorithms().join(",");
    trace!("Configuring HostKey algorithms: {}", algos);
    if let Err(err) = session.method_pref(SshMethodType::HostKey, algos.as_str()) {
        error!("Could not set host key algorithms: {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // ciphers
    let algos = params.ciphers.algorithms().join(",");
    trace!("Configuring Crypt algorithms: {}", algos);
    if let Err(err) = session.method_pref(SshMethodType::CryptCs, algos.as_str()) {
        error!("Could not set crypt algorithms (client-server): {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }
    if let Err(err) = session.method_pref(SshMethodType::CryptSc, algos.as_str()) {
        error!("Could not set crypt algorithms (server-client): {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // MAC
    let algos = params.mac.algorithms().join(",");
    trace!("Configuring MAC algorithms: {}", algos);
    if let Err(err) = session.method_pref(SshMethodType::MacCs, algos.as_str()) {
        error!("Could not set MAC algorithms (client-server): {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }
    if let Err(err) = session.method_pref(SshMethodType::MacSc, algos.as_str()) {
        error!("Could not set MAC algorithms (server-client): {}", err);
        return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
    }

    // -- configure algos from opts
    for method in opts.methods.iter() {
        let algos = method.prefs();
        trace!("Configuring {:?} algorithm: {}", method.method_type, algos);
        if let Err(err) = session.method_pref(method.method_type.into(), algos.as_str()) {
            error!("Could not set {:?} algorithms: {}", method.method_type, err);
            return Err(RemoteError::new_ex(RemoteErrorType::ProtocolError, err));
        }
    }
    Ok(())
}

/// Authenticate on session with ssh agent
fn session_auth_with_agent(
    session: &mut Session,
    username: &str,
    ssh_agent_config: &SshAgentIdentity,
) -> RemoteResult<()> {
    let mut agent = session
        .agent()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;

    agent
        .connect()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;

    agent
        .list_identities()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?;

    let mut connection_result = Err(RemoteError::new(RemoteErrorType::AuthenticationFailed));

    for identity in agent
        .identities()
        .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err))?
    {
        if ssh_agent_config.pubkey_matches(identity.blob()) {
            debug!("Trying to authenticate with ssh agent with key: {identity:?}");
        } else {
            continue;
        }
        match agent.userauth(username, &identity) {
            Ok(()) => {
                connection_result = Ok(());
                debug!("Authenticated with ssh agent with key: {identity:?}");
                break;
            }
            Err(err) => {
                debug!("SSH agent auth failed: {err}");
                connection_result = Err(RemoteError::new_ex(
                    RemoteErrorType::AuthenticationFailed,
                    err,
                ));
            }
        }
    }

    if let Err(err) = agent.disconnect() {
        warn!("Could not disconnect from ssh agent: {err}");
    }

    connection_result
}

/// Authenticate on session with private key
fn session_auth_with_rsakey(
    session: &mut Session,
    username: &str,
    private_key: &Path,
    password: Option<&str>,
    identity_file: Option<&[PathBuf]>,
) -> RemoteResult<()> {
    debug!("Authenticating with username '{}' and RSA key", username);
    let mut keys = vec![private_key];
    if let Some(identity_file) = identity_file {
        let other_keys: Vec<&Path> = identity_file.iter().map(|x| x.as_path()).collect();
        keys.extend(other_keys);
    }
    // iterate over keys
    for key in keys.into_iter() {
        trace!("Trying to authenticate with RSA key at '{}'", key.display());
        match session.userauth_pubkey_file(username, None, key, password) {
            Ok(_) => {
                debug!("Authenticated with key at '{}'", key.display());
                return Ok(());
            }
            Err(err) => {
                error!("Authentication failed: {}", err);
            }
        }
    }
    Err(RemoteError::new_ex(
        RemoteErrorType::AuthenticationFailed,
        "could not find any suitable RSA key to authenticate with",
    ))
}

/// Authenticate on session with the provided [`Authentication`] method.
fn session_auth(
    session: &mut Session,
    opts: &SshOpts,
    ssh_config: &Config,
    authentication: Authentication,
) -> RemoteResult<()> {
    match authentication {
        Authentication::RsaKey(private_key) => session_auth_with_rsakey(
            session,
            &ssh_config.username,
            private_key.as_path(),
            opts.password.as_deref(),
            ssh_config.params.identity_file.as_deref(),
        ),
        Authentication::Password(password) => {
            session_auth_with_password(session, &ssh_config.username, &password)
        }
    }
}

/// Authenticate on session with username and password
fn session_auth_with_password(
    session: &mut Session,
    username: &str,
    password: &str,
) -> RemoteResult<()> {
    // Username / password
    debug!("Authenticating with username '{}' and password", username);
    if let Err(err) = session.userauth_password(username, password) {
        error!("Authentication failed: {}", err);
        Err(RemoteError::new_ex(
            RemoteErrorType::AuthenticationFailed,
            err,
        ))
    } else {
        Ok(())
    }
}

// -- shell commands

/// Perform shell command in current SSH session
pub fn perform_shell_cmd<S: AsRef<str>>(session: &mut Session, cmd: S) -> RemoteResult<String> {
    // Create channel
    trace!("Running command: {}", cmd.as_ref());
    let mut channel = match session.channel_session() {
        Ok(ch) => ch,
        Err(err) => {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                format!("Could not open channel: {err}"),
            ));
        }
    };
    // Execute command
    if let Err(err) = channel.exec(cmd.as_ref()) {
        return Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not execute command \"{}\": {}", cmd.as_ref(), err),
        ));
    }
    // Read output
    let mut output: String = String::new();
    match channel.read_to_string(&mut output) {
        Ok(_) => {
            // Wait close
            let _ = channel.wait_close();
            trace!("Command output: {}", output);
            Ok(output)
        }
        Err(err) => Err(RemoteError::new_ex(
            RemoteErrorType::ProtocolError,
            format!("Could not read output: {err}"),
        )),
    }
}

/// Perform shell command at specified path and return exit code and output
pub fn perform_shell_cmd_at_with_rc<S: AsRef<str>>(
    session: &mut Session,
    cmd: S,
    p: &Path,
) -> RemoteResult<(u32, String)> {
    perform_shell_cmd_with_rc(session, format!("cd \"{}\"; {}", p.display(), cmd.as_ref()))
}

/// Perform shell command and collect return code and output
pub fn perform_shell_cmd_with_rc<S: AsRef<str>>(
    session: &mut Session,
    cmd: S,
) -> RemoteResult<(u32, String)> {
    let output = perform_shell_cmd(session, format!("{}; echo $?", cmd.as_ref()))?;
    if let Some(index) = output.trim().rfind('\n') {
        trace!("Read from stdout: '{}'", output);
        let actual_output = (output[0..index + 1]).to_string();
        trace!("Actual output '{}'", actual_output);
        trace!("Parsing return code '{}'", output[index..].trim());
        let rc = match u32::from_str(output[index..].trim()).ok() {
            Some(val) => val,
            None => {
                return Err(RemoteError::new_ex(
                    RemoteErrorType::ProtocolError,
                    "Failed to get command exit code",
                ));
            }
        };
        debug!(r#"Command output: "{}"; exit code: {}"#, actual_output, rc);
        Ok((rc, actual_output))
    } else {
        match u32::from_str(output.trim()).ok() {
            Some(val) => Ok((val, String::new())),
            None => Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                "Failed to get command exit code",
            )),
        }
    }
}

#[cfg(test)]
mod test {

    use ssh2_config::ParseRule;

    use super::*;
    use crate::mock::ssh as ssh_mock;

    #[test]

    fn should_connect_to_ssh_server_auth_user_password() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .password("password");

        if let Err(err) = connect(&opts) {
            panic!("Could not connect to server: {}", err);
        }
        let session = connect(&opts).unwrap();
        assert!(session.authenticated());

        drop(container);
    }

    #[test]

    fn should_connect_to_ssh_server_auth_key() {
        use crate::ssh::container::OpensshServer;

        let container = OpensshServer::start();
        let port = container.port();

        crate::mock::logger();
        let config_file = ssh_mock::create_ssh_config(port);
        let opts = SshOpts::new("sftp")
            .config_file(config_file.path(), ParseRule::ALLOW_UNKNOWN_FIELDS)
            .key_storage(Box::new(ssh_mock::MockSshKeyStorage::default()));
        let session = connect(&opts).unwrap();
        assert!(session.authenticated());
    }

    #[test]

    fn should_perform_shell_command_on_server() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password");
        let mut session = connect(&opts).unwrap();
        assert!(session.authenticated());
        // run commands
        assert!(perform_shell_cmd(&mut session, "pwd").is_ok());
    }

    #[test]

    fn should_perform_shell_command_on_server_and_return_exit_code() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("password");
        let mut session = connect(&opts).unwrap();
        assert!(session.authenticated());
        // run commands
        assert_eq!(
            perform_shell_cmd_at_with_rc(&mut session, "pwd", Path::new("/tmp"))
                .ok()
                .unwrap(),
            (0, String::from("/tmp\n"))
        );
        assert_eq!(
            perform_shell_cmd_at_with_rc(&mut session, "pippopluto", Path::new("/tmp"))
                .ok()
                .unwrap()
                .0,
            127
        );
    }

    #[test]

    fn should_fail_authentication() {
        crate::mock::logger();
        let container = crate::ssh::container::OpensshServer::start();
        let port = container.port();

        let opts = SshOpts::new("127.0.0.1")
            .port(port)
            .username("sftp")
            .password("ippopotamo");
        assert!(connect(&opts).is_err());
    }

    #[test]
    fn test_filetransfer_sftp_bad_server() {
        crate::mock::logger();
        let opts = SshOpts::new("myverybad.verybad.server")
            .port(10022)
            .username("sftp")
            .password("ippopotamo");
        assert!(connect(&opts).is_err());
    }
}
