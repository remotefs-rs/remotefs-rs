use std::sync::Arc;

use argh::FromArgs;
use remotefs_kube::{Config, KubeMultiPodFs};

#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "ftp")]
/// Mount a Kube multipod filesystem
pub struct KubeArgs {
    /// namespace
    #[argh(option, default = "String::from(\"default\")")]
    namespace: String,
    /// kubernetes cluster URL
    #[argh(option)]
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
