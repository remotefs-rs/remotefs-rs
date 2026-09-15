#[macro_use]
extern crate log;

use std::path::Path;

use argh::FromArgs;
use remotefs::AsyncRemoteFs;
use remotefs_smb::{SmbCredentials, SmbFs, SmbOptions};

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    let mut client = SmbFs::try_new(
        SmbCredentials::default()
            .server(args.server)
            .share(args.share)
            .username(args.username)
            .password(password)
            .workgroup(args.workgroup),
        SmbOptions::default(),
    )?;

    info!("connecting to server...");
    client.connect().await?;
    info!("client connected");

    let root = Path::new("/");
    info!("listing files at {}", root.display());
    for file in client.list_dir(root).await? {
        println!("{}", file.name());
    }

    info!("disconnecting client...");
    client.disconnect().await?;
    info!("client disconnected");

    Ok(())
}
