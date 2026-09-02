#[macro_use]
extern crate log;

use std::sync::Arc;

use argh::FromArgs;
use remotefs::RemoteFs;
use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};
use tokio::runtime::Runtime;

#[derive(FromArgs)]
#[argh(description = "
where positional can be: [smb://address[:port]]

Lists the share root with the Rust-native SMB client.

Please, report issues to <https://github.com/remotefs-rs/remotefs-rs-smb>
Please, consider supporting the author <https://ko-fi.com/veeso>")]
struct Args {
    #[argh(option, short = 'P', description = "specify password")]
    password: Option<String>,
    #[argh(option, short = 'u', description = "specify username")]
    username: String,
    #[argh(
        option,
        short = 'w',
        default = r#""WORKGROUP".to_string()"#,
        description = "specify workgroup"
    )]
    workgroup: String,
    #[argh(option, short = 's', description = "specify share")]
    share: String,
    #[argh(positional, description = "smb://address[:port]")]
    server: String,
}

fn main() -> anyhow::Result<()> {
    assert!(env_logger::builder().try_init().is_ok());
    let args: Args = argh::from_env();
    let password = match &args.password {
        Some(password) => password.clone(),
        None => rpassword::prompt_password("Password: ")?,
    };

    info!(
        "initializing client with server {} and share {}, with username {} and workgroup {}",
        args.server, args.share, args.username, args.workgroup
    );
    let runtime = Arc::new(Runtime::new()?);
    let mut client = SmbFs::try_new(
        SmbCredentials::default()
            .server(args.server)
            .share(args.share)
            .username(args.username)
            .password(password)
            .workgroup(args.workgroup),
        SmbOptions::default(),
        &runtime,
    )?;

    info!("connecting to server...");
    client.connect()?;
    info!("client connected");

    let wrkdir = client.pwd()?;
    info!("listing files at {}", wrkdir.display());
    for file in client.list_dir(&wrkdir)? {
        println!("{}", file.name());
    }

    info!("disconnecting client...");
    client.disconnect()?;
    info!("client disconnected");

    Ok(())
}
