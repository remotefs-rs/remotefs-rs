use std::sync::Arc;

use clap::Args;
use remotefs_kube::{Config, KubeMultiPodFs};

/// Mount a Kube multipod filesystem
#[derive(Args, Debug)]
pub struct KubeArgs {
    /// namespace
    #[arg(long, default_value = "default")]
    namespace: String,
    /// kubernetes cluster URL
    #[arg(long)]
    cluster_url: String,
}

impl From<KubeArgs> for KubeMultiPodFs {
    fn from(args: KubeArgs) -> Self {
        let mut config = Config::new(args.cluster_url.parse().expect("Invalid cluster URL"));
        config.default_namespace = args.namespace;

        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("Unable to create tokio runtime"),
        );

        KubeMultiPodFs::new(&rt).config(config)
    }
}
