#[macro_use]
extern crate log;

use argh::FromArgs;

use remotefs::RemoteFs;
#[cfg(target_family = "windows")]
use remotefs_smb::{SmbCredentials, SmbFs};
#[cfg(target_family = "unix")]
use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};

#[derive(FromArgs)]
#[argh(description = "
where positional can be: [smb://address[:port]]

Please, report issues to <https://github.com/veeso/remotefs-rs-smb>
Please, consider supporting the author <https://ko-fi.com/veeso>")]
struct Args {
    #[argh(option, short = 'P', description = "specify password")]
    password: Option<String>,
    #[cfg(target_family = "windows")]
    #[argh(option, short = 'u', description = "specify username")]
    username: Option<String>,
    #[cfg(target_family = "unix")]
    #[argh(option, short = 'u', description = "specify username")]
    username: String,
    #[cfg(target_family = "unix")]
    #[argh(
        option,
        short = 'w',
        default = r#""WORKGROUP".to_string()"#,
        description = "specify workgroup"
    )]
    workgroup: String,
    #[argh(option, short = 's', description = "specify share")]
    share: String,
    #[argh(
        positional,
        description = "smb://address[:port] on UNIX and \\\\server\\share on Windows"
    )]
    server: String,
}

fn main() -> anyhow::Result<()> {
    assert!(env_logger::builder().try_init().is_ok());
    let args: Args = argh::from_env();
    #[cfg(target_family = "unix")]
    let password = match &args.password {
        Some(p) => p.clone(),
        None => read_secret_from_tty("Password: ").ok().unwrap(),
    };

    #[cfg(target_family = "unix")]
    let mut client = init_client(args, password)?;
    #[cfg(target_family = "windows")]
    let mut client = init_client(args);

    info!("connecting to server...");
    client.connect()?;
    info!("client connected");

    let wrkdir = client.pwd()?;
    info!("listing files at {}", wrkdir.display());
    let files = client.list_dir(&wrkdir)?;

    for file in files {
        println!("{}", file.name());
    }

    info!("disconnecting client...");
    client.disconnect()?;
    info!("client disconnected");

    Ok(())
}

#[cfg(target_family = "windows")]
fn init_client(args: Args) -> SmbFs {
    info!(
        "initializing client with server {} and share {}",
        args.server, args.share
    );
    let mut credentials = SmbCredentials::new(args.server, args.share);
    if let Some(username) = args.username {
        credentials = credentials.username(username);
    }
    if let Some(password) = args.password {
        credentials = credentials.password(password);
    }
    SmbFs::new(credentials)
}

#[cfg(target_family = "unix")]
fn init_client(args: Args, password: String) -> anyhow::Result<SmbFs> {
    info!(
        "initializing client with server {} and share {}, with username {} and workgroup {}",
        args.server, args.share, args.username, args.workgroup
    );
    let client = SmbFs::try_new(
        SmbCredentials::default()
            .server(args.server)
            .share(args.share)
            .username(args.username)
            .password(password)
            .workgroup(args.workgroup),
        SmbOptions::default()
            .one_share_per_server(true)
            .case_sensitive(false),
    )?;

    Ok(client)
}

#[cfg(target_family = "unix")]
/// Read a secret from tty with customisable prompt
fn read_secret_from_tty(prompt: &str) -> std::io::Result<String> {
    rpassword::prompt_password(prompt)
}
