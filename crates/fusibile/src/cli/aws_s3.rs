use std::sync::Arc;

use clap::Args;
use remotefs_aws_s3::AwsS3Fs;

/// Mount an AWS S3 bucket
#[derive(Args)]
pub struct AwsS3Args {
    /// the name of the bucket to mount
    #[arg(long)]
    bucket: String,
    /// the region of the bucket
    #[arg(long)]
    region: Option<String>,
    /// custom endpoint
    #[arg(long)]
    endpoint: Option<String>,
    /// aws profile
    #[arg(long)]
    profile: Option<String>,
    /// access key
    #[arg(long)]
    access_key: Option<String>,
    /// secret key
    #[arg(long)]
    secret_access_key: Option<String>,
    /// security token
    #[arg(long)]
    security_token: Option<String>,
    /// new path style
    #[arg(long)]
    new_path_style: bool,
}

impl std::fmt::Debug for AwsS3Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsS3Args")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("profile", &self.profile)
            .field(
                "access_key",
                &self.access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "security_token",
                &self.security_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("new_path_style", &self.new_path_style)
            .finish()
    }
}

impl From<AwsS3Args> for AwsS3Fs {
    fn from(args: AwsS3Args) -> Self {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("Unable to create tokio runtime"),
        );

        let mut fs = AwsS3Fs::new(args.bucket, &rt).new_path_style(args.new_path_style);
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

#[cfg(test)]
mod tests {
    use super::AwsS3Args;

    #[test]
    fn debug_should_redact_secrets() {
        let secret_key = "super-secret-key";
        let secret_token = "super-secret-token";

        let args = AwsS3Args {
            bucket: "bucket".to_string(),
            region: None,
            endpoint: None,
            profile: None,
            access_key: Some("access-key".to_string()),
            secret_access_key: Some(secret_key.to_string()),
            security_token: Some(secret_token.to_string()),
            new_path_style: false,
        };
        let rendered = format!("{args:?}");
        assert!(!rendered.contains(secret_key));
        assert!(!rendered.contains(secret_token));
        assert!(rendered.contains("[REDACTED]"));
    }
}
