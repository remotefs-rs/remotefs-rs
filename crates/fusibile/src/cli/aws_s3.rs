use argh::FromArgs;
use remotefs_aws_s3::AwsS3Fs;

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "aws-s3")]
/// Mount an AWS S3 bucket
pub struct AwsS3Args {
    /// the name of the bucket to mount
    #[argh(option)]
    bucket: String,
    /// the region of the bucket
    #[argh(option)]
    region: Option<String>,
    /// custom endpoint
    #[argh(option)]
    endpoint: Option<String>,
    /// aws profile
    #[argh(option)]
    profile: Option<String>,
    /// access key
    #[argh(option)]
    access_key: Option<String>,
    /// secret key
    #[argh(option)]
    secret_access_key: Option<String>,
    /// security token
    #[argh(option)]
    security_token: Option<String>,
    /// new path style
    #[argh(switch)]
    new_path_style: bool,
}

impl From<AwsS3Args> for AwsS3Fs {
    fn from(args: AwsS3Args) -> Self {
        let mut fs = AwsS3Fs::new(args.bucket).new_path_style(args.new_path_style);
        if let Some(region) = args.region {
            fs = fs.region(region);
        }
        if let Some(endpoint) = args.endpoint {
            fs = fs.endpoint(endpoint);
        }
        if let Some(profile) = args.profile {
            fs = fs.profile(profile);
        }
        if let Some(access_key) = args.access_key {
            fs = fs.access_key(access_key);
        }
        if let Some(secret_access_key) = args.secret_access_key {
            fs = fs.secret_access_key(secret_access_key);
        }
        if let Some(security_token) = args.security_token {
            fs = fs.security_token(security_token);
        }

        fs
    }
}
