//! ## Kube MultiPod FS
//!
//! The `KubeMultiPodFs` client is a client that allows you to interact with multiple pods in a Kubernetes cluster.

mod path;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client, Config};
use remotefs::fs::{
    FileType, Metadata, ReadStream, RemoteError, RemoteErrorType, RemoteFs, RemoteResult, UnixPex,
    Welcome, WriteStream,
};
use remotefs::File;
use tokio::runtime::Runtime;

use self::path::KubePath;
use crate::KubeContainerFs;

/// Kube MultiPod FS
///
/// The `KubeMultiPodFs` client is a client that allows you to interact with multiple pods in a Kubernetes cluster.
///
/// Underneath it uses the `KubeContainerFs` client to interact with the pods, but it changes the current pod and
/// the container name under the hood, to simulate a multi-pod filesystem.
///
/// Path are relative to the current pod and container and have the following format:
///
/// /pod-name/container-name/path/to/file
pub struct KubeMultiPodFs {
    kube: KubeContainerFs,
    runtime: Arc<Runtime>,
}

impl KubeMultiPodFs {
    /// Create a new `KubeMultiPodFs` client
    pub fn new(runtime: &Arc<Runtime>) -> Self {
        Self {
            kube: KubeContainerFs::new("", "", runtime),
            runtime: runtime.clone(),
        }
    }

    /// Set configuration
    pub fn config(mut self, config: Config) -> Self {
        self.kube = self.kube.config(config);
        self
    }

    /// Get the current pod name
    fn pod_name(&self) -> Option<&str> {
        if self.kube.pod_name.is_empty() {
            None
        } else {
            Some(&self.kube.pod_name)
        }
    }

    /// Returns the current container name
    fn container_name(&self) -> Option<&str> {
        // if there is no pod, there is no container
        if self.kube.pod_name.is_empty() {
            return None;
        }
        if self.kube.container.is_empty() {
            None
        } else {
            Some(&self.kube.container)
        }
    }

    /// Get the kube path from a path
    fn kube_path(&self, path: &Path) -> KubePath {
        KubePath::from_path(self.pod_name(), self.container_name(), path)
    }

    /// Dispatch operations based on the path
    ///
    /// The `on_root` closure is called when the path is `/`
    /// The `on_pod` closure is called when the path is `/pod-name`
    /// The `on_container` closure is called when the path is `/pod-name/container-name` or `/pod-name/container-name/path/to/file`
    ///
    /// In any case, the current pod and container are set accordingly.
    fn path_dispatch<T, FR, FP, FC, FPP>(
        &mut self,
        path: KubePath,
        on_root: FR,
        on_pod: FP,
        on_container: FC,
        on_path: FPP,
    ) -> T
    where
        FR: FnOnce(&mut Self) -> T,
        FP: FnOnce(&mut Self, &str) -> T,
        FC: FnOnce(&mut Self, &str) -> T,
        FPP: FnOnce(&mut Self, &Path) -> T,
    {
        if path.pod.is_none() {
            return on_root(self);
        }
        if path.container.is_none() {
            return on_pod(self, path.pod.as_deref().unwrap());
        }

        // temporary set pod and container
        if let Some(p) = path.path {
            let prev_pod = self.kube.pod_name.clone();
            let prev_container = self.kube.container.clone();
            self.kube.pod_name = path.pod.unwrap();
            self.kube.container = path.container.unwrap();
            let res = on_path(self, &p);

            // restore pod and container
            self.kube.pod_name = prev_pod;
            self.kube.container = prev_container;

            res
        } else {
            on_container(self, path.container.as_deref().unwrap())
        }
    }

    /// Files coming from the container client has the absolute path relative to the container fs.
    ///
    /// The absolute path must be changed to `/pod-name/container-name/path/to/file`
    fn fix_absolute_path(&self, mut f: File) -> File {
        if self.pod_name().is_none() || self.container_name().is_none() {
            return f;
        }

        let mut p = PathBuf::from("/");
        p.push(self.pod_name().unwrap());
        p.push(self.container_name().unwrap());

        let relative_path = f.path.strip_prefix("/").unwrap_or(f.path.as_path());
        p.push(relative_path);

        f.path = p;
        f
    }

    /// List pods
    fn list_pods(&self) -> RemoteResult<Vec<File>> {
        let api = self.kube.pods.as_ref().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NotConnected,
                "Not connected to a Kubernetes cluster",
            )
        })?;
        let pods = self
            .runtime
            .block_on(async { api.list(&Default::default()).await })
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::ProtocolError, err))?;

        Ok(pods
            .into_iter()
            .map(|pod| File {
                path: {
                    let mut p = PathBuf::from("/");
                    p.push(pod.metadata.name.unwrap_or_default());
                    p
                },
                metadata: Metadata::default().file_type(FileType::Directory),
            })
            .collect())
    }

    /// List containers
    fn list_containers(&self, pod_name: &str) -> RemoteResult<Vec<File>> {
        let api = self.kube.pods.as_ref().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NotConnected,
                "Not connected to a Kubernetes cluster",
            )
        })?;
        let pod = self
            .runtime
            .block_on(async { api.get(pod_name).await })
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, err))?;

        let pod_spec = pod.spec.ok_or_else(|| {
            RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, "Pod spec not found")
        })?;

        Ok(pod_spec
            .containers
            .into_iter()
            .map(|container| File {
                path: {
                    let mut p = PathBuf::from("/");
                    p.push(pod_name);
                    p.push(&container.name);
                    debug!("found container {} -> {}", container.name, p.display());

                    p
                },
                metadata: Metadata::default().file_type(FileType::Directory),
            })
            .collect())
    }

    /// Stat root
    #[inline]
    fn stat_root(&self) -> RemoteResult<File> {
        Ok(File {
            path: PathBuf::from("/"),
            metadata: Metadata::default().file_type(FileType::Directory),
        })
    }

    /// Stat pod
    fn stat_pod(&self, pod: &str) -> RemoteResult<File> {
        let pods = self.list_pods()?;

        pods.into_iter().find(|f| f.name() == pod).ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NoSuchFileOrDirectory,
                format!("Pod {} not found", pod),
            )
        })
    }

    /// Stat container
    fn stat_container(&self, container: &str) -> RemoteResult<File> {
        let pod_name = self.pod_name().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NoSuchFileOrDirectory,
                "No pod to stat container",
            )
        })?;
        let containers = self.list_containers(pod_name)?;

        containers
            .into_iter()
            .find(|f| f.name() == container)
            .ok_or_else(|| {
                RemoteError::new_ex(
                    RemoteErrorType::NoSuchFileOrDirectory,
                    format!("Container {} not found", container),
                )
            })
    }

    /// Check whether pod exists
    fn exists_pod(&self, pod: &str) -> RemoteResult<bool> {
        let api = self.kube.pods.as_ref().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NotConnected,
                "Not connected to a Kubernetes cluster",
            )
        })?;

        Ok(self.runtime.block_on(async { api.get(pod).await.is_ok() }))
    }

    /// Check whether container exists
    fn exists_container(&self, container: &str) -> RemoteResult<bool> {
        let pod_name = self.pod_name().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NoSuchFileOrDirectory,
                "No pod to check container existence",
            )
        })?;

        let api = self.kube.pods.as_ref().ok_or_else(|| {
            RemoteError::new_ex(
                RemoteErrorType::NotConnected,
                "Not connected to a Kubernetes cluster",
            )
        })?;

        let pod = self
            .runtime
            .block_on(async { api.get(pod_name).await })
            .map_err(|err| RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, err))?;

        let pod_spec = pod.spec.ok_or_else(|| {
            RemoteError::new_ex(RemoteErrorType::NoSuchFileOrDirectory, "Pod spec not found")
        })?;

        Ok(pod_spec.containers.iter().any(|c| c.name == container))
    }
}

impl RemoteFs for KubeMultiPodFs {
    fn connect(&mut self) -> RemoteResult<Welcome> {
        debug!("Initializing Kube connection...");
        let api = self.runtime.block_on(async {
            let client = match self.kube.config.as_ref() {
                Some(config) => Client::try_from(config.clone())
                    .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err)),
                None => Client::try_default()
                    .await
                    .map_err(|err| RemoteError::new_ex(RemoteErrorType::ConnectionError, err)),
            }?;
            let api: Api<Pod> = Api::default_namespaced(client);

            Ok(api)
        })?;

        // Set pods
        self.kube.pods = Some(api);

        Ok(Welcome::default())
    }

    fn disconnect(&mut self) -> RemoteResult<()> {
        self.kube.disconnect()
    }

    fn is_connected(&mut self) -> bool {
        if self.pod_name().is_none() {
            self.kube.pods.is_some()
        } else {
            self.kube.is_connected()
        }
    }

    fn pwd(&mut self) -> RemoteResult<PathBuf> {
        let mut p = PathBuf::from("/");

        // compose path in format /pod-name/container-name/pwd
        if let Some(pod_name) = self.pod_name() {
            p.push(pod_name);
        } else {
            return Ok(p);
        }

        if let Some(container_name) = self.container_name() {
            p.push(container_name);
        } else {
            return Ok(p);
        }

        // push as relative
        let pwd = self.kube.pwd()?;
        let pwd_as_relative = pwd.strip_prefix("/").unwrap_or(pwd.as_path());
        p.push(pwd_as_relative);

        Ok(p)
    }

    fn change_dir(&mut self, dir: &Path) -> RemoteResult<PathBuf> {
        let path = self.kube_path(dir);
        debug!("Changing directory to {path}");

        let prev_pod = self.pod_name().unwrap_or("").to_string();
        let prev_container = self.container_name().unwrap_or("").to_string();

        if let Some(pod) = path.pod {
            if self.exists_pod(&pod)? {
                self.kube.pod_name = pod.to_string();
            } else {
                return Err(RemoteError::new_ex(
                    RemoteErrorType::NoSuchFileOrDirectory,
                    format!("Pod {} does not exist", pod),
                ));
            }
        } else {
            self.kube.pod_name = "".to_string();
        }

        if let Some(container) = path.container {
            if self.exists_container(&container)? {
                self.kube.container = container.to_string();
            } else {
                // restore previous pod
                self.kube.pod_name = prev_pod;
                return Err(RemoteError::new_ex(
                    RemoteErrorType::NoSuchFileOrDirectory,
                    format!("Container {} does not exist", container),
                ));
            }
        } else {
            self.kube.container = "".to_string();
        }

        let res = if let Some(path) = path.path {
            self.kube.change_dir(&path)
        } else {
            self.kube.wrkdir = PathBuf::from("/");
            Ok(PathBuf::from("/"))
        };

        // restore previous pod and container
        if let Err(err) = res {
            self.kube.pod_name = prev_pod;
            self.kube.container = prev_container;

            return Err(err);
        }

        self.pwd()
    }

    fn list_dir(&mut self, path: &Path) -> RemoteResult<Vec<File>> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |fs| fs.list_pods(),
            |fs, pod| fs.list_containers(pod),
            |fs, _| {
                fs.kube
                    .list_dir(Path::new("/"))
                    .map(|files| files.into_iter().map(|f| fs.fix_absolute_path(f)).collect())
            },
            |fs, path| {
                fs.kube
                    .list_dir(path)
                    .map(|files| files.into_iter().map(|f| fs.fix_absolute_path(f)).collect())
            },
        )
    }

    fn stat(&mut self, path: &Path) -> RemoteResult<File> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |fs| fs.stat_root(),
            |fs, pod| fs.stat_pod(pod),
            |fs, container| {
                fs.stat_container(container)
                    .map(|f| fs.fix_absolute_path(f))
            },
            |fs, path| fs.kube.stat(path).map(|f| fs.fix_absolute_path(f)),
        )
    }

    fn setstat(&mut self, path: &Path, metadata: Metadata) -> RemoteResult<()> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| Ok(()),
            |_, _| Ok(()),
            |_, _| Ok(()),
            |fs, path| fs.kube.setstat(path, metadata),
        )
    }

    fn exists(&mut self, path: &Path) -> RemoteResult<bool> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| Ok(true),
            |fs, pod| fs.exists_pod(pod),
            |fs, container| fs.exists_container(container),
            |fs, path| fs.kube.exists(path),
        )
    }

    fn remove_file(&mut self, path: &Path) -> RemoteResult<()> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |_, _| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |_, _| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |fs, path| fs.kube.remove_file(path),
        )
    }

    fn remove_dir(&mut self, path: &Path) -> RemoteResult<()> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |_, _| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |_, _| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |fs, path| fs.kube.remove_dir(path),
        )
    }

    fn remove_dir_all(&mut self, path: &Path) -> RemoteResult<()> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |_, _| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |_, _| Err(RemoteError::new(RemoteErrorType::CouldNotRemoveFile)),
            |fs, path| fs.kube.remove_dir_all(path),
        )
    }

    fn create_dir(&mut self, path: &Path, mode: UnixPex) -> RemoteResult<()> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.create_dir(path, mode),
        )
    }

    fn symlink(&mut self, path: &Path, target: &Path) -> RemoteResult<()> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.symlink(path, target),
        )
    }

    fn copy(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let path = self.kube_path(src);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.copy(path, dest),
        )
    }

    fn mov(&mut self, src: &Path, dest: &Path) -> RemoteResult<()> {
        let path = self.kube_path(src);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.mov(path, dest),
        )
    }

    fn exec(&mut self, cmd: &str) -> RemoteResult<(u32, String)> {
        if self.pod_name().is_none() || self.container_name().is_none() {
            return Err(RemoteError::new_ex(
                RemoteErrorType::ProtocolError,
                "No pod or container to execute command on",
            ));
        }

        self.kube.exec(cmd)
    }

    fn append(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create(&mut self, _path: &Path, _metadata: &Metadata) -> RemoteResult<WriteStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn open(&mut self, _path: &Path) -> RemoteResult<ReadStream> {
        Err(RemoteError::new(RemoteErrorType::UnsupportedFeature))
    }

    fn create_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        reader: Box<dyn std::io::Read + Send>,
    ) -> RemoteResult<u64> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.create_file(path, metadata, reader),
        )
    }

    fn append_file(
        &mut self,
        path: &Path,
        metadata: &Metadata,
        reader: Box<dyn std::io::Read + Send>,
    ) -> RemoteResult<u64> {
        let path = self.kube_path(path);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.append_file(path, metadata, reader),
        )
    }

    fn open_file(&mut self, src: &Path, dest: Box<dyn std::io::Write + Send>) -> RemoteResult<u64> {
        let path = self.kube_path(src);

        self.path_dispatch(
            path,
            |_| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |_, _| {
                Err(RemoteError::new_ex(
                    RemoteErrorType::CouldNotOpenFile,
                    "This operation requires a pod and a container",
                ))
            },
            |fs, path| fs.kube.open_file(path, dest),
        )
    }
}

#[cfg(test)]
mod test {

    #[cfg(feature = "integration-tests")]
    use std::io::Cursor;

    #[cfg(feature = "integration-tests")]
    use pretty_assertions::assert_eq;

    #[cfg(feature = "integration-tests")]
    use super::*;

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_append_to_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        // Append to file
        let file_data = "Hello, world!\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .append_file(p, &Metadata::default(), Box::new(reader))
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_change_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        let pwd = client.pwd().ok().unwrap();

        let pod = client.pod_name().unwrap().to_string();
        let container = client.container_name().unwrap().to_string();

        let mut p = PathBuf::from("/");
        p.push(&pod);
        p.push(&container);
        p.push("tmp");

        assert!(client.change_dir(&p).is_ok());
        assert!(client.change_dir(pwd.as_path()).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_change_directory_relative() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        assert!(client
            .create_dir(
                Path::new("should_change_directory_relative"),
                UnixPex::from(0o755)
            )
            .is_ok());
        assert!(client
            .change_dir(Path::new("should_change_directory_relative/"))
            .is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_change_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        assert!(client
            .change_dir(Path::new("/tmp/sdfghjuireghiuergh/useghiyuwegh"))
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_copy_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;

        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.copy(p, Path::new("b.txt")).is_ok());

        assert!(client.stat(p).is_ok());
        assert!(client.stat(Path::new("b.txt")).is_ok());

        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_copy_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.copy(p, Path::new("aaa/bbbb/ccc/b.txt")).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_create_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        let p = PathBuf::from(format!("{}/mydir", client.pwd().unwrap().display()));
        assert!(client.exists(&p).unwrap());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_create_directory_cause_already_exists() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // create directory
        assert!(client
            .create_dir(Path::new("mydir"), UnixPex::from(0o755))
            .is_ok());
        assert_eq!(
            client
                .create_dir(Path::new("mydir"), UnixPex::from(0o755))
                .err()
                .unwrap()
                .kind,
            RemoteErrorType::DirectoryAlreadyExists
        );
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_create_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // create directory
        assert!(client
            .create_dir(
                Path::new("/tmp/werfgjwerughjwurih/iwerjghiwgui"),
                UnixPex::from(0o755)
            )
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_create_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert_eq!(
            client.create_file(p, &metadata, Box::new(reader)).unwrap(),
            10
        );
        // Verify size
        assert_eq!(client.stat(p).ok().unwrap().metadata().size, 10);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_create_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("/tmp/ahsufhauiefhuiashf/hfhfhfhf");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_exec_command() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        assert_eq!(
            client.exec("echo 5").ok().unwrap(),
            (0, String::from("5\n"))
        );
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_tell_whether_file_exists() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        assert_eq!(client.exists(p).ok().unwrap(), true);
        assert_eq!(client.exists(Path::new("b.txt")).ok().unwrap(), false);

        assert_eq!(client.exists(Path::new("/tmp/ppppp")).ok().unwrap(), false);

        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_list_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let wrkdir = client.pwd().ok().unwrap();
        debug!(
            "Working directory: {}; pod {:?}; container {:?}",
            wrkdir.display(),
            client.pod_name(),
            client.container_name()
        );
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let file = client
            .list_dir(wrkdir.as_path())
            .ok()
            .unwrap()
            .get(0)
            .unwrap()
            .clone();
        assert_eq!(file.name().as_str(), "a.txt");
        let mut expected_path = wrkdir;
        expected_path.push(p);
        assert_eq!(file.path.as_path(), expected_path.as_path());
        assert_eq!(file.extension().as_deref().unwrap(), "txt");
        assert_eq!(file.metadata.size, 10);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_list_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        assert!(client.list_dir(Path::new("/tmp/auhhfh/hfhjfhf/")).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_move_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let dest = Path::new("b.txt");
        assert!(client.mov(p, dest).is_ok());
        assert_eq!(client.exists(p).ok().unwrap(), false);
        assert_eq!(client.exists(dest).ok().unwrap(), true);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_move_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let dest = Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt");
        assert!(client.mov(p, dest).is_err());
        assert!(client
            .mov(Path::new("/tmp/wuefhiwuerfh/whjhh/b.txt"), p)
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_open_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let metadata = Metadata::default().size(file_data.len() as u64);
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert_eq!(client.open_file(p, buffer).ok().unwrap(), 10);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_open_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Verify size
        let buffer: Box<dyn std::io::Write + Send> = Box::new(Vec::with_capacity(512));
        assert!(client
            .open_file(Path::new("/tmp/aashafb/hhh"), buffer)
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_print_working_directory() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        assert!(client.pwd().is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_remove_dir_all() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Create file
        let mut file_path = dir_path.clone();
        file_path.push(Path::new("a.txt"));
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir_all(dir_path.as_path()).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_remove_dir_all() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Remove dir
        assert!(client
            .remove_dir_all(Path::new("/tmp/aaaaaa/asuhi"))
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_remove_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        assert!(client.remove_dir(dir_path.as_path()).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_remove_dir() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create dir
        let mut dir_path = client.pwd().ok().unwrap();
        dir_path.push(Path::new("test/"));
        assert!(client
            .create_dir(dir_path.as_path(), UnixPex::from(0o775))
            .is_ok());
        // Create file
        let mut file_path = dir_path.clone();
        file_path.push(Path::new("a.txt"));
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client
            .create_file(file_path.as_path(), &metadata, Box::new(reader))
            .is_ok());
        // Remove dir
        assert!(client.remove_dir(dir_path.as_path()).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_remove_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.txt");
        let file_data = "test data\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());
        assert!(client.remove_file(p).is_ok());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_setstat_file() {
        use std::time::SystemTime;

        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());

        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: Some(SystemTime::UNIX_EPOCH),
                    created: None,
                    file_type: FileType::File,
                    gid: Some(1000),
                    mode: Some(UnixPex::from(0o755)),
                    modified: Some(SystemTime::UNIX_EPOCH),
                    size: 7,
                    symlink: None,
                    uid: Some(1000),
                }
            )
            .is_ok());
        let entry = client.stat(p).ok().unwrap();
        let stat = entry.metadata();
        assert_eq!(stat.accessed, None);
        assert_eq!(stat.created, None);
        assert_eq!(stat.modified, Some(SystemTime::UNIX_EPOCH));
        assert_eq!(stat.mode.unwrap(), UnixPex::from(0o755));
        assert_eq!(stat.size, 7);

        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_setstat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("bbbbb/cccc/a.sh");
        assert!(client
            .setstat(
                p,
                Metadata {
                    accessed: None,
                    created: None,
                    file_type: FileType::File,
                    gid: Some(1),
                    mode: Some(UnixPex::from(0o755)),
                    modified: None,
                    size: 7,
                    symlink: None,
                    uid: Some(1),
                }
            )
            .is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_stat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert_eq!(
            client
                .create_file(p, &metadata, Box::new(reader))
                .ok()
                .unwrap(),
            7
        );
        let entry = client.stat(p).ok().unwrap();
        assert_eq!(entry.name(), "a.sh");
        let mut expected_path = client.pwd().ok().unwrap();
        expected_path.push("a.sh");
        assert_eq!(entry.path(), expected_path.as_path());
        let meta = entry.metadata();
        assert_eq!(meta.size, 7);
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_stat_file() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        assert!(client.stat(p).is_err());
        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_make_symlink() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());

        let symlink = Path::new("b.sh");

        assert!(client.symlink(symlink, p).is_ok());
        assert!(client.remove_file(symlink).is_ok());

        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn should_not_make_symlink() {
        crate::log_init();
        let (pods, mut client) = setup_client();
        // Create file
        let p = Path::new("a.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        let mut metadata = Metadata::default();
        metadata.size = file_data.len() as u64;
        assert!(client.create_file(p, &metadata, Box::new(reader)).is_ok());

        let symlink = Path::new("b.sh");
        let file_data = "echo 5\n";
        let reader = Cursor::new(file_data.as_bytes());
        assert!(client
            .create_file(symlink, &metadata, Box::new(reader))
            .is_ok());

        assert!(client.symlink(symlink, p).is_err());
        assert!(client.remove_file(symlink).is_ok());
        assert!(client.symlink(symlink, Path::new("c.sh")).is_err());

        finalize_client(pods, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn test_should_list_pods() {
        let (api, mut client) = setup_client();

        let files = client.list_dir(Path::new("/")).unwrap();
        assert!(files.len() >= 2);

        finalize_client(api, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn test_should_list_containers() {
        let (api, mut client) = setup_client();

        let pods = client.list_dir(Path::new("/")).unwrap();
        let pod_name = pods.get(0).unwrap().name();

        let mut path = PathBuf::from("/");
        path.push(pod_name);

        let containers = client.list_dir(path.as_path()).unwrap();
        assert_eq!(containers.len(), 1);
        assert_eq!(containers.get(0).unwrap().name(), "alpine");

        finalize_client(api, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn test_should_enter_pod() {
        let (api, mut client) = setup_client();

        let pods = client.list_dir(Path::new("/")).unwrap();
        debug!("Pods: {pods:?}");
        let pod_name = pods.get(0).unwrap().name();
        debug!("Pod name: {pod_name}");

        let mut path = PathBuf::from("/");
        path.push(pod_name);
        debug!("Path: {path:?}");

        assert!(client.change_dir(path.as_path()).is_ok());
        assert_eq!(client.pwd().unwrap().as_path(), path.as_path());

        finalize_client(api, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn test_should_enter_container() {
        let (api, mut client) = setup_client();

        let pods = client.list_dir(Path::new("/")).unwrap();
        let pod_name = pods.get(0).unwrap().name();

        let mut path = PathBuf::from("/");
        path.push(pod_name);

        let containers = client.list_dir(path.as_path()).unwrap();
        let container_name = containers.get(0).unwrap().name();

        path.push(container_name);

        assert!(client.change_dir(path.as_path()).is_ok());
        assert_eq!(client.pwd().unwrap().as_path(), path.as_path());

        finalize_client(api, client);
    }

    #[test]
    #[cfg(feature = "integration-tests")]
    fn test_should_enter_root() {
        let (api, mut client) = setup_client();

        let path = PathBuf::from("/");

        assert!(client.change_dir(path.as_path()).is_ok());
        assert_eq!(client.pwd().unwrap().as_path(), path.as_path());

        finalize_client(api, client);
    }

    #[cfg(feature = "integration-tests")]
    fn setup_client() -> (Api<Pod>, KubeMultiPodFs) {
        crate::log_init();
        // setup pod with random name

        use kube::api::PostParams;
        use kube::config::AuthInfo;
        use kube::ResourceExt as _;

        let runtime = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );

        let minikube_ip = std::env::var("MINIKUBE_IP").unwrap();

        // setup pod
        debug!("setting up pod");
        // config for minikube
        let mut auth_info = AuthInfo::default();
        auth_info.username = Some("minikube".to_string());
        // get home
        let home = std::env::var("HOME").unwrap();
        auth_info.client_certificate =
            Some(format!("{home}/.minikube/profiles/minikube/client.crt"));
        auth_info.client_key = Some(format!("{home}/.minikube/profiles/minikube/client.key"));

        debug!("Auth info: {auth_info:?}");

        let config = Config {
            cluster_url: format!("https://{minikube_ip}:8443").parse().unwrap(),
            default_namespace: "default".to_string(),
            read_timeout: None,
            root_cert: None,
            connect_timeout: None,
            write_timeout: None,
            accept_invalid_certs: true,
            auth_info,
            proxy_url: None,
            tls_server_name: None,
        };

        let pod_names = (0..2)
            .into_iter()
            .map(|_| generate_pod_name())
            .collect::<Vec<String>>();

        // generate 2 pods
        let pods = runtime.block_on(async {
            let client = Client::try_from(config.clone()).unwrap();
            let pods: Api<Pod> = Api::default_namespaced(client);

            for pod_name in &pod_names {
                debug!("Pod name: {pod_name}");

                let p: Pod = serde_json::from_value(serde_json::json!({
                    "apiVersion": "v1",
                    "kind": "Pod",
                    "metadata": { "name": pod_name },
                    "spec": {
                        "containers": [{
                          "name": "alpine",
                          "image": "alpine" ,
                          "command": ["tail", "-f", "/dev/null"],
                        }],
                    }
                }))
                .unwrap();

                let pp = PostParams::default();
                match pods.create(&pp, &p).await {
                    Ok(o) => {
                        let name = o.name_any();
                        assert_eq!(p.name_any(), name);
                        info!("Created {}", name);
                    }
                    Err(kube::Error::Api(ae)) => assert_eq!(ae.code, 409), // if you skipped delete, for instance
                    Err(e) => panic!("failed to create: {e}"), // any other case is probably bad
                }

                debug!("Pod created");

                let establish = kube::runtime::wait::await_condition(
                    pods.clone(),
                    &pod_name,
                    kube::runtime::conditions::is_pod_running(),
                );

                info!("Waiting for pod to be running...");
                let _ = tokio::time::timeout(std::time::Duration::from_secs(30), establish)
                    .await
                    .expect("pod timeout");
            }

            pods
        });

        let mut client = KubeMultiPodFs::new(&runtime).config(config.clone());
        client.connect().expect("connection failed");

        let mut tempdir = PathBuf::from("/");
        tempdir.push(&pod_names[0]);
        tempdir.push("alpine");
        tempdir.push(generate_tempdir());
        println!("Tempdir: {}", tempdir.display());
        // Create wrkdir
        client
            .create_dir(tempdir.as_path(), UnixPex::from(0o775))
            .expect("failed to create tempdir");
        // Change directory
        client
            .change_dir(tempdir.as_path())
            .expect("failed to enter tempdir");
        (pods, client)
    }

    #[cfg(feature = "integration-tests")]
    fn finalize_client(_pods: Api<Pod>, mut client: KubeMultiPodFs) {
        assert!(client.disconnect().is_ok());
    }

    #[cfg(feature = "integration-tests")]
    fn generate_pod_name() -> String {
        use rand::distributions::Alphanumeric;
        use rand::{thread_rng, Rng as _};

        let mut rng = thread_rng();
        let random_string: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .filter(|c| c.is_alphabetic())
            .map(|c| c.to_ascii_lowercase())
            .take(12)
            .collect();

        format!("test-{}", random_string)
    }

    #[cfg(feature = "integration-tests")]
    fn generate_tempdir() -> String {
        use rand::distributions::Alphanumeric;
        use rand::{thread_rng, Rng};
        let mut rng = thread_rng();
        let name: String = std::iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .map(char::from)
            .take(8)
            .collect();
        format!("tmp/temp_{}", name)
    }
}
