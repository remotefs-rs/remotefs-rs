//! ## Kube MultiPod FS
//!
//! The `KubeMultiPodFs` client is a client that allows you to interact with multiple pods in a Kubernetes cluster.

mod path;

use std::path::{Path, PathBuf};

use futures_io::AsyncRead;
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;
use kube::{Api, Client, Config};
use remotefs::File;
use remotefs::fs::{
    AsyncReadStream, AsyncRemoteFs, AsyncWriteStream, Capabilities, ExecOutput, FileType, Metadata,
    ReadOptions, RemoteError, RemoteErrorType, RemoteResult, SetMetadata, UnixPex, WriteOptions,
};

use self::path::KubePath;
use crate::KubeContainerFs;
use crate::utils::path as path_utils;

/// Blocking adapter over [`KubeMultiPodFs`], implementing [`remotefs::RemoteFs`].
#[cfg(feature = "tokio")]
pub type BlockingKubeMultiPodFs = remotefs::adapters::blocking::BlockOn<KubeMultiPodFs>;

/// An [`AsyncRemoteFs`] client exposing every pod and container in a
/// namespace as one abstract file system.
///
/// Paths have the form `/pod-name/container-name/path/to/file`. `/`,
/// `/pod-name`, and `/pod-name/container-name` are virtual directories: they
/// can be listed and stat-ed but never modified. Underneath, every
/// in-container operation is delegated to a [`KubeContainerFs`] addressed by
/// the path.
///
/// # Examples
///
/// ```rust,no_run
/// use std::path::Path;
///
/// use remotefs::AsyncRemoteFs;
/// use remotefs_kube::KubeMultiPodFs;
///
/// # async fn run() -> remotefs::RemoteResult<()> {
/// let mut client = KubeMultiPodFs::new();
/// client.connect().await?;
/// let pods = client.list_dir(Path::new("/")).await?;
/// let files = client.list_dir(Path::new("/my-pod/alpine/tmp")).await?;
/// client.disconnect().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct KubeMultiPodFs {
    config: Option<Config>,
    pods: Option<Api<Pod>>,
}

impl KubeMultiPodFs {
    /// Create a client over the default namespace.
    ///
    /// If [`KubeMultiPodFs::config`] is not called before
    /// [`connect`](AsyncRemoteFs::connect), the client falls back to the
    /// default kubeconfig (or the in-cluster configuration, when running
    /// inside a pod).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use remotefs_kube::KubeMultiPodFs;
    ///
    /// let client = KubeMultiPodFs::new();
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the Kubernetes client configuration to use on
    /// [`connect`](AsyncRemoteFs::connect), instead of the default kubeconfig.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs_kube::{Config, KubeMultiPodFs};
    ///
    /// let config = Config::new("https://127.0.0.1:8443".parse().unwrap());
    /// let client = KubeMultiPodFs::new().config(config);
    /// ```
    #[must_use]
    pub fn config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Wrap the client for blocking callers using the given runtime handle.
    ///
    /// The returned value implements [`remotefs::RemoteFs`] and can be stored
    /// as `Box<dyn RemoteFs>`. It must not be used from inside an async
    /// context, as documented by [`tokio::runtime::Handle::block_on`].
    ///
    /// # Panics
    ///
    /// Panics if `handle` belongs to a current-thread runtime, which cannot
    /// drive the blocked operation from a non-async caller.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use remotefs::RemoteFs;
    /// use remotefs_kube::KubeMultiPodFs;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let runtime = tokio::runtime::Runtime::new()?;
    /// let mut client: Box<dyn RemoteFs> =
    ///     Box::new(KubeMultiPodFs::new().into_blocking(runtime.handle().clone()));
    /// client.connect()?;
    /// client.disconnect()?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tokio")]
    #[must_use]
    pub fn into_blocking(self, handle: tokio::runtime::Handle) -> BlockingKubeMultiPodFs {
        assert_ne!(
            handle.runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread,
            "into_blocking requires a multi-thread Tokio runtime"
        );
        remotefs::adapters::blocking::BlockOn::new(self, handle)
    }

    // -- private

    fn pods(&self) -> RemoteResult<&Api<Pod>> {
        self.pods
            .as_ref()
            .ok_or_else(|| RemoteError::new(RemoteErrorType::NotConnected))
    }

    /// Parse `path`, failing before the connection check on invalid paths.
    fn resolve(&self, path: &Path) -> RemoteResult<KubePath> {
        KubePath::parse(path)
    }

    /// A connected container client for `pod`/`container`.
    fn container_fs(&self, pod: &str, container: &str) -> RemoteResult<KubeContainerFs> {
        Ok(KubeContainerFs::attached(
            self.pods()?.clone(),
            pod,
            container,
        ))
    }

    fn virtual_path_error() -> RemoteError {
        RemoteError::with_message(
            RemoteErrorType::PermissionDenied,
            "pods and containers are virtual directories",
        )
    }

    /// Resolve `path` to a container client and an in-container path, or
    /// fail because the path names a virtual directory.
    fn container_target(&self, path: &Path) -> RemoteResult<(KubeContainerFs, PathBuf)> {
        let kube_path = self.resolve(path)?;
        match (kube_path.pod, kube_path.container, kube_path.path) {
            (Some(pod), Some(container), Some(inner)) => {
                Ok((self.container_fs(&pod, &container)?, inner))
            }
            _ => {
                self.pods()?;
                Err(Self::virtual_path_error())
            }
        }
    }

    /// Resolve two paths that must live in the same container.
    fn same_container_targets(
        &self,
        a: &Path,
        b: &Path,
    ) -> RemoteResult<(KubeContainerFs, PathBuf, PathBuf)> {
        let a = self.resolve(a)?;
        let b = self.resolve(b)?;
        match (a.pod, a.container, a.path, b.pod, b.container, b.path) {
            (
                Some(pod_a),
                Some(container_a),
                Some(inner_a),
                Some(pod_b),
                Some(container_b),
                Some(inner_b),
            ) => {
                if pod_a != pod_b || container_a != container_b {
                    self.pods()?;
                    return Err(RemoteError::with_message(
                        RemoteErrorType::UnsupportedFeature,
                        "cross-container operations are not supported",
                    ));
                }
                Ok((self.container_fs(&pod_a, &container_a)?, inner_a, inner_b))
            }
            _ => {
                self.pods()?;
                Err(Self::virtual_path_error())
            }
        }
    }

    /// Prefix a file coming from a container client with `/pod/container`.
    fn prefix_path(pod: &str, container: &str, mut file: File) -> File {
        let mut p = path_utils::join(Path::new("/"), pod);
        p = path_utils::join(&p, container);
        let relative = file.path.to_string_lossy().to_string();
        let relative = relative.trim_start_matches('/');
        file.path = if relative.is_empty() {
            p
        } else {
            path_utils::join(&p, relative)
        };
        file
    }

    fn virtual_dir(path: PathBuf) -> File {
        File::new(path, Metadata::default().file_type(FileType::Directory))
    }

    async fn list_pods(&self) -> RemoteResult<Vec<File>> {
        let api = self.pods()?;
        let pods = api
            .list(&ListParams::default())
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))?;
        Ok(pods
            .into_iter()
            .map(|pod| {
                Self::virtual_dir(path_utils::join(
                    Path::new("/"),
                    &pod.metadata.name.unwrap_or_default(),
                ))
            })
            .collect())
    }

    async fn get_pod(&self, pod_name: &str) -> RemoteResult<Pod> {
        self.pods()?
            .get(pod_name)
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::NoSuchFileOrDirectory, err))
    }

    async fn list_containers(&self, pod_name: &str) -> RemoteResult<Vec<File>> {
        let pod = self.get_pod(pod_name).await?;
        let pod_spec = pod.spec.ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::NoSuchFileOrDirectory, "Pod spec not found")
        })?;
        let pod_path = path_utils::join(Path::new("/"), pod_name);
        Ok(pod_spec
            .containers
            .into_iter()
            .map(|container| {
                debug!("found container {name}", name = container.name);
                Self::virtual_dir(path_utils::join(&pod_path, &container.name))
            })
            .collect())
    }

    async fn stat_pod(&self, pod: &str) -> RemoteResult<File> {
        self.get_pod(pod).await?;
        Ok(Self::virtual_dir(path_utils::join(Path::new("/"), pod)))
    }

    async fn stat_container(&self, pod: &str, container: &str) -> RemoteResult<File> {
        self.list_containers(pod)
            .await?
            .into_iter()
            .find(|f| f.name() == container)
            .ok_or_else(|| {
                RemoteError::with_message(
                    RemoteErrorType::NoSuchFileOrDirectory,
                    format!("Container {container} not found"),
                )
            })
    }

    async fn exists_pod(&self, pod: &str) -> RemoteResult<bool> {
        Ok(self.pods()?.get(pod).await.is_ok())
    }

    async fn exists_container(&self, pod: &str, container: &str) -> RemoteResult<bool> {
        let pod = match self.pods()?.get(pod).await {
            Ok(pod) => pod,
            Err(_) => return Ok(false),
        };
        Ok(pod
            .spec
            .map(|spec| spec.containers.iter().any(|c| c.name == container))
            .unwrap_or(false))
    }
}

#[remotefs::async_trait]
impl AsyncRemoteFs for KubeMultiPodFs {
    async fn connect(&mut self) -> RemoteResult<()> {
        if self.pods.is_some() {
            return Err(RemoteError::new(RemoteErrorType::AlreadyConnected));
        }
        debug!("Initializing Kube connection...");
        let client = match self.config.as_ref() {
            Some(config) => Client::try_from(config.clone()),
            None => Client::try_default().await,
        }
        .map_err(|err| RemoteError::with_source(RemoteErrorType::ConnectionError, err))?;
        let api: Api<Pod> = Api::default_namespaced(client);
        api.list(&ListParams::default().limit(1))
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::ConnectionError, err))?;
        self.pods = Some(api);
        info!("Connection established");
        Ok(())
    }

    async fn disconnect(&mut self) -> RemoteResult<()> {
        if self.pods.take().is_none() {
            return Err(RemoteError::new(RemoteErrorType::NotConnected));
        }
        info!("Disconnected from remote");
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.pods.is_some()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::STREAM_READ
            | Capabilities::STREAM_WRITE
            | Capabilities::APPEND
            | Capabilities::RANGE_READ
            | Capabilities::COPY
            | Capabilities::SYMLINK
            | Capabilities::SET_METADATA
            | Capabilities::POSIX_MODE
    }

    async fn list_dir(&self, path: &Path) -> RemoteResult<Vec<File>> {
        let kube_path = self.resolve(path)?;
        match (kube_path.pod, kube_path.container, kube_path.path) {
            (None, _, _) => self.list_pods().await,
            (Some(pod), None, _) => self.list_containers(&pod).await,
            (Some(pod), Some(container), inner) => {
                let fs = self.container_fs(&pod, &container)?;
                let inner = inner.unwrap_or_else(|| PathBuf::from("/"));
                let files = fs.list_dir(&inner).await?;
                Ok(files
                    .into_iter()
                    .map(|f| Self::prefix_path(&pod, &container, f))
                    .collect())
            }
        }
    }

    async fn stat(&self, path: &Path) -> RemoteResult<File> {
        let kube_path = self.resolve(path)?;
        match (kube_path.pod, kube_path.container, kube_path.path) {
            (None, _, _) => {
                self.pods()?;
                Ok(Self::virtual_dir(PathBuf::from("/")))
            }
            (Some(pod), None, _) => self.stat_pod(&pod).await,
            (Some(pod), Some(container), None) => self.stat_container(&pod, &container).await,
            (Some(pod), Some(container), Some(inner)) => {
                let fs = self.container_fs(&pod, &container)?;
                fs.stat(&inner)
                    .await
                    .map(|f| Self::prefix_path(&pod, &container, f))
            }
        }
    }

    async fn exists(&self, path: &Path) -> RemoteResult<bool> {
        let kube_path = self.resolve(path)?;
        match (kube_path.pod, kube_path.container, kube_path.path) {
            (None, _, _) => {
                self.pods()?;
                Ok(true)
            }
            (Some(pod), None, _) => self.exists_pod(&pod).await,
            (Some(pod), Some(container), None) => self.exists_container(&pod, &container).await,
            (Some(pod), Some(container), Some(inner)) => {
                self.container_fs(&pod, &container)?.exists(&inner).await
            }
        }
    }

    async fn set_metadata(&self, path: &Path, metadata: &SetMetadata) -> RemoteResult<()> {
        let (fs, inner) = self.container_target(path)?;
        fs.set_metadata(&inner, metadata).await
    }

    async fn create_dir(&self, path: &Path, mode: Option<UnixPex>) -> RemoteResult<()> {
        let (fs, inner) = self.container_target(path)?;
        fs.create_dir(&inner, mode).await
    }

    async fn remove_file(&self, path: &Path) -> RemoteResult<()> {
        let (fs, inner) = self.container_target(path)?;
        fs.remove_file(&inner).await
    }

    async fn remove_dir(&self, path: &Path) -> RemoteResult<()> {
        let (fs, inner) = self.container_target(path)?;
        fs.remove_dir(&inner).await
    }

    async fn remove_dir_all(&self, path: &Path) -> RemoteResult<()> {
        let (fs, inner) = self.container_target(path)?;
        fs.remove_dir_all(&inner).await
    }

    async fn rename(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let (fs, src, dest) = self.same_container_targets(src, dest)?;
        fs.rename(&src, &dest).await
    }

    async fn copy(&self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let (fs, src, dest) = self.same_container_targets(src, dest)?;
        fs.copy(&src, &dest).await
    }

    async fn symlink(&self, path: &Path, target: &Path) -> RemoteResult<()> {
        let (fs, path, target) = self.same_container_targets(path, target)?;
        fs.symlink(&path, &target).await
    }

    async fn open(&self, path: &Path, opts: &ReadOptions) -> RemoteResult<AsyncReadStream> {
        let (fs, inner) = self.container_target(path)?;
        fs.open(&inner, opts).await
    }

    async fn create(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let (fs, inner) = self.container_target(path)?;
        fs.create(&inner, opts).await
    }

    async fn append(&self, path: &Path, opts: &WriteOptions) -> RemoteResult<AsyncWriteStream> {
        let (fs, inner) = self.container_target(path)?;
        fs.append(&inner, opts).await
    }

    async fn write_file(
        &self,
        path: &Path,
        opts: &WriteOptions,
        src: &mut (dyn AsyncRead + Send + Unpin),
    ) -> RemoteResult<u64> {
        let (fs, inner) = self.container_target(path)?;
        fs.write_file(&inner, opts, src).await
    }

    /// Commands need a pod and a container; the multi-pod client has no
    /// current container, so `exec` is unsupported.
    async fn exec(&self, _cmd: &str) -> RemoteResult<ExecOutput> {
        Err(RemoteError::with_message(
            RemoteErrorType::UnsupportedFeature,
            "exec requires a KubeContainerFs",
        ))
    }
}

#[cfg(test)]
mod test {

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn should_init_multipod_fs() {
        let client = KubeMultiPodFs::new();
        assert!(client.config.is_none());
        assert!(!client.is_connected());
        assert!(!client.capabilities().contains(Capabilities::EXEC));
        assert!(client.capabilities().contains(Capabilities::RANGE_READ));
    }

    #[tokio::test]
    async fn should_reject_relative_paths_before_connection_check() {
        let client = KubeMultiPodFs::new();
        assert_eq!(
            client
                .stat(Path::new("pod/container"))
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
        assert_eq!(
            client
                .create_dir(Path::new("pod/container/dir"), None)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::InvalidPath
        );
    }

    #[tokio::test]
    async fn should_fail_as_not_connected_or_virtual() {
        let client = KubeMultiPodFs::new();
        assert_eq!(
            client.list_dir(Path::new("/")).await.unwrap_err().kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client
                .create_dir(Path::new("/pod/container/dir"), None)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client
                .create_dir(Path::new("/pod/container"), None)
                .await
                .unwrap_err()
                .kind(),
            RemoteErrorType::NotConnected
        );
        assert_eq!(
            client.exec("echo").await.unwrap_err().kind(),
            RemoteErrorType::UnsupportedFeature
        );
    }

    #[test]
    fn should_prefix_container_paths() {
        let file = File::new(PathBuf::from("/tmp/a.txt"), Metadata::default());
        let file = KubeMultiPodFs::prefix_path("pod", "alpine", file);
        assert_eq!(file.path.to_string_lossy(), "/pod/alpine/tmp/a.txt");
        let root = File::new(PathBuf::from("/"), Metadata::default());
        let root = KubeMultiPodFs::prefix_path("pod", "alpine", root);
        assert_eq!(root.path.to_string_lossy(), "/pod/alpine");
    }

    #[test]
    fn test_should_be_send_sync_and_object_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<KubeMultiPodFs>();
        let _: Box<dyn AsyncRemoteFs> = Box::new(KubeMultiPodFs::new());
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn blocking_wrapper_is_a_remote_fs_trait_object() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client: Box<dyn remotefs::RemoteFs> =
            Box::new(KubeMultiPodFs::new().into_blocking(runtime.handle().clone()));
        assert!(!client.is_connected());
    }

    #[cfg(feature = "integration-tests")]
    mod integration {

        use futures::io::Cursor;
        use pretty_assertions::assert_eq;
        use serial_test::serial;

        use super::*;

        async fn write(client: &KubeMultiPodFs, path: &Path, data: &str) -> u64 {
            let mut reader = Cursor::new(data.as_bytes().to_vec());
            client
                .write_file(
                    path,
                    &WriteOptions::default().size_hint(data.len() as u64),
                    &mut reader,
                )
                .await
                .expect("write failed")
        }

        async fn read(client: &KubeMultiPodFs, path: &Path, opts: &ReadOptions) -> Vec<u8> {
            let mut dest = Cursor::new(Vec::new());
            client
                .read_file(path, opts, &mut dest)
                .await
                .expect("read failed");
            dest.into_inner()
        }

        /// `/pod` for a `/pod/alpine/tmp/...` tempdir.
        fn pod_path(tempdir: &Path) -> PathBuf {
            let pod = tempdir.iter().nth(1).unwrap().to_string_lossy();
            PathBuf::from(format!("/{pod}"))
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_list_pods_containers_and_files() {
            let (pods, client, tempdir) = setup_client().await;
            let listed = client.list_dir(Path::new("/")).await.unwrap();
            assert!(listed.len() >= 2);
            assert!(listed.iter().all(File::is_dir));
            let pod_path = pod_path(&tempdir);
            let containers = client.list_dir(&pod_path).await.unwrap();
            assert_eq!(containers.len(), 1);
            assert_eq!(containers[0].name(), "alpine");
            assert_eq!(containers[0].path, pod_path.join("alpine"));
            let root = client.list_dir(&pod_path.join("alpine")).await.unwrap();
            assert!(root.iter().any(|file| file.name() == "tmp"));
            assert!(
                root.iter()
                    .all(|file| file.path.starts_with(pod_path.join("alpine")))
            );
            let path = tempdir.join("a.txt");
            write(&client, &path, "test data\n").await;
            let files = client.list_dir(&tempdir).await.unwrap();
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].path, path);
            assert_eq!(files[0].metadata.size, Some(10));
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_stat_and_exists_at_every_level() {
            let (pods, client, tempdir) = setup_client().await;
            let pod_path = pod_path(&tempdir);
            assert!(client.stat(Path::new("/")).await.unwrap().is_dir());
            assert!(client.stat(&pod_path).await.unwrap().is_dir());
            assert_eq!(
                client.stat(&pod_path.join("alpine")).await.unwrap().name(),
                "alpine"
            );
            assert!(client.exists(Path::new("/")).await.unwrap());
            assert!(client.exists(&pod_path).await.unwrap());
            assert!(client.exists(&pod_path.join("alpine")).await.unwrap());
            assert!(!client.exists(&pod_path.join("nope")).await.unwrap());
            assert!(!client.exists(Path::new("/no-such-pod")).await.unwrap());
            assert_eq!(
                client
                    .stat(Path::new("/no-such-pod"))
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::NoSuchFileOrDirectory
            );
            let path = tempdir.join("a.sh");
            write(&client, &path, "echo 5\n").await;
            let entry = client.stat(&path).await.unwrap();
            assert_eq!(entry.path(), path.as_path());
            assert_eq!(entry.metadata().size, Some(7));
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_reject_mutations_on_virtual_paths() {
            let (pods, client, tempdir) = setup_client().await;
            let pod_path = pod_path(&tempdir);
            for path in [
                Path::new("/"),
                pod_path.as_path(),
                pod_path.join("alpine").as_path(),
            ] {
                assert_eq!(
                    client.create_dir(path, None).await.unwrap_err().kind(),
                    RemoteErrorType::PermissionDenied
                );
                assert_eq!(
                    client.remove_dir_all(path).await.unwrap_err().kind(),
                    RemoteErrorType::PermissionDenied
                );
            }
            assert_eq!(
                client
                    .copy(&tempdir.join("a"), Path::new("/other-pod/alpine/tmp/a"))
                    .await
                    .unwrap_err()
                    .kind(),
                RemoteErrorType::UnsupportedFeature
            );
            finalize_client(pods, client).await;
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn should_round_trip_files() {
            let (pods, client, tempdir) = setup_client().await;
            let path = tempdir.join("a.txt");
            assert_eq!(write(&client, &path, "abcdef").await, 6);
            assert_eq!(
                read(&client, &path, &ReadOptions::default().offset(2).length(2)).await,
                b"cd"
            );
            let dest = tempdir.join("b.txt");
            assert!(client.copy(&path, &dest).await.is_ok());
            assert!(client.rename(&dest, &tempdir.join("c.txt")).await.is_ok());
            assert!(client.symlink(&tempdir.join("link"), &path).await.is_ok());
            assert!(
                client
                    .set_metadata(&path, &SetMetadata::default().mode(UnixPex::from(0o600)))
                    .await
                    .is_ok()
            );
            assert_eq!(
                client.stat(&path).await.unwrap().metadata().mode.unwrap(),
                UnixPex::from(0o600)
            );
            assert!(client.remove_file(&tempdir.join("link")).await.is_ok());
            assert!(client.remove_dir_all(&tempdir).await.is_ok());
            assert!(!client.exists(&tempdir).await.unwrap());
            finalize_client(pods, client).await;
        }

        async fn setup_client() -> (Api<Pod>, KubeMultiPodFs, PathBuf) {
            crate::log_init();
            use kube::ResourceExt as _;
            use kube::api::PostParams;
            use kube::config::AuthInfo;

            let minikube_ip = std::env::var("MINIKUBE_IP").unwrap();
            let mut auth_info = AuthInfo {
                username: Some("minikube".to_string()),
                ..Default::default()
            };
            let home = std::env::var("HOME").unwrap();
            auth_info.client_certificate =
                Some(format!("{home}/.minikube/profiles/minikube/client.crt"));
            auth_info.client_key = Some(format!("{home}/.minikube/profiles/minikube/client.key"));
            let mut config = Config::new(format!("https://{minikube_ip}:8443").parse().unwrap());
            config.accept_invalid_certs = true;
            config.auth_info = auth_info;

            let pod_names = (0..2).map(|_| generate_pod_name()).collect::<Vec<String>>();
            let client = Client::try_from(config.clone()).unwrap();
            let pods: Api<Pod> = Api::default_namespaced(client);
            for pod_name in &pod_names {
                let pod: Pod = serde_json::from_value(serde_json::json!({
                    "apiVersion": "v1",
                    "kind": "Pod",
                    "metadata": { "name": pod_name },
                    "spec": {
                        "containers": [{
                          "name": "alpine",
                          "image": "alpine:3.20",
                          "imagePullPolicy": "IfNotPresent",
                          "command": ["tail", "-f", "/dev/null"],
                        }],
                    }
                }))
                .unwrap();
                match pods.create(&PostParams::default(), &pod).await {
                    Ok(created) => assert_eq!(pod.name_any(), created.name_any()),
                    Err(kube::Error::Api(ae)) => assert_eq!(ae.code, 409),
                    Err(error) => panic!("failed to create: {error}"),
                }
                let establish = kube::runtime::wait::await_condition(
                    pods.clone(),
                    pod_name,
                    kube::runtime::conditions::is_pod_running(),
                );
                let _ = tokio::time::timeout(std::time::Duration::from_secs(30), establish)
                    .await
                    .expect("pod timeout");
            }

            let mut client = KubeMultiPodFs::new().config(config);
            client.connect().await.expect("connection failed");
            let tempdir = PathBuf::from(format!("/{}/alpine/{}", pod_names[0], generate_tempdir()));
            client
                .create_dir(&tempdir, Some(UnixPex::from(0o775)))
                .await
                .expect("failed to create tempdir");
            (pods, client, tempdir)
        }

        async fn finalize_client(pods: Api<Pod>, mut client: KubeMultiPodFs) {
            if let Err(error) = delete_test_pods(&pods).await {
                warn!("failed to clean up test pods: {error}");
            }
            assert!(client.disconnect().await.is_ok());
        }

        async fn delete_test_pods(pods: &Api<Pod>) -> kube::Result<()> {
            use kube::ResourceExt as _;
            use kube::api::DeleteParams;

            for pod in pods.list(&Default::default()).await? {
                let name = pod.name_any();
                if name.starts_with("test-") {
                    pods.delete(&name, &DeleteParams::default()).await?;
                }
            }
            Ok(())
        }

        fn generate_pod_name() -> String {
            use rand::RngExt as _;
            use rand::distr::Alphanumeric;

            let mut rng = rand::rng();
            let random_string: String = std::iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .map(char::from)
                .filter(|character| character.is_alphabetic())
                .map(|character| character.to_ascii_lowercase())
                .take(12)
                .collect();
            format!("test-{random_string}")
        }

        fn generate_tempdir() -> String {
            use rand::RngExt as _;
            use rand::distr::Alphanumeric;

            let mut rng = rand::rng();
            let name: String = std::iter::repeat(())
                .map(|()| rng.sample(Alphanumeric))
                .map(char::from)
                .take(8)
                .collect();
            format!("tmp/temp_{name}")
        }
    }
}
