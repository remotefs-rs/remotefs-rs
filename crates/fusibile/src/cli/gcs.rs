//! Google cloud storage (GCS) CLI commands.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::Args;
use remotefs_gcs::credentials::service_account;
use remotefs_gcs::{GoogleCloudStorageCredentials, GoogleCloudStorageFs};

/// Google Cloud Storage's default JSON API endpoint.
const DEFAULT_GCS_ENDPOINT: &str = "https://storage.googleapis.com";

/// Mount a Google Cloud Storage bucket
#[derive(Args, Debug)]
pub struct GcsArgs {
    /// the name of the bucket to mount
    #[arg(long)]
    bucket: String,
    /// the Google Cloud Storage endpoint URL.
    #[arg(long, default_value = DEFAULT_GCS_ENDPOINT)]
    endpoint: String,
    /// optional path to a service-account JSON file.
    #[arg(long)]
    service_account_key: Option<PathBuf>,
}

impl TryFrom<GcsArgs> for GoogleCloudStorageFs {
    type Error = anyhow::Error;

    fn try_from(args: GcsArgs) -> Result<Self, Self::Error> {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("Unable to create tokio runtime"),
        );

        let fs = match args.service_account_key {
            None => GoogleCloudStorageFs::new(args.bucket, &rt),
            Some(path) => {
                let raw = std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "Unable to read GCS service-account file '{path}'",
                        path = path.display()
                    )
                })?;
                let key = serde_json::from_str(&raw).with_context(|| {
                    format!(
                        "Invalid GCS service-account JSON in '{path}'",
                        path = path.display()
                    )
                })?;
                let credentials = {
                    let _guard = rt.enter();
                    service_account::Builder::new(key).build()
                }
                .with_context(|| {
                    format!(
                        "Invalid GCS service-account credentials in '{path}'",
                        path = path.display()
                    )
                })?;
                GoogleCloudStorageFs::with_credentials(
                    args.bucket,
                    GoogleCloudStorageCredentials::custom(credentials),
                    &rt,
                )
            }
        };

        Ok(fs.endpoint(args.endpoint))
    }
}
