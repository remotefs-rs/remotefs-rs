//! Shell command runner over the pod exec stream.

use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::{AttachParams, AttachedProcess};
use remotefs::fs::ExecOutput;
use remotefs::{RemoteError, RemoteErrorType, RemoteResult};
use tokio::io::AsyncReadExt as _;

fn command_script(cmd: &str) -> String {
    format!(r#"({cmd}); printf ";$?""#)
}

/// Runs commands inside one container of one pod.
///
/// Cloning is cheap: `Api<Pod>` is a handle over a shared client.
#[derive(Clone, Debug)]
pub(crate) struct KubeExec {
    pods: Api<Pod>,
    pod_name: String,
    container: String,
}

impl KubeExec {
    pub(crate) fn new(
        pods: Api<Pod>,
        pod_name: impl Into<String>,
        container: impl Into<String>,
    ) -> Self {
        Self {
            pods,
            pod_name: pod_name.into(),
            container: container.into(),
        }
    }

    #[expect(dead_code, reason = "kept as part of the internal exec abstraction")]
    pub(crate) fn pods(&self) -> &Api<Pod> {
        &self.pods
    }

    /// Spawn `argv` in the container with the given attach parameters.
    pub(crate) async fn spawn(
        &self,
        argv: &[&str],
        params: AttachParams,
    ) -> RemoteResult<AttachedProcess> {
        let params = params.container(self.container.clone());
        self.pods
            .exec(&self.pod_name, argv.to_vec(), &params)
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))
    }

    /// Run `cmd` through `/bin/sh -c`, returning its stdout and exit code.
    pub(crate) async fn shell(&self, cmd: &str) -> RemoteResult<ExecOutput> {
        let script = command_script(cmd);
        debug!("Executing shell command: {script}");
        let params = AttachParams::default()
            .stdin(false)
            .stdout(true)
            .stderr(true);
        let mut process = self.spawn(&["/bin/sh", "-c", &script], params).await?;

        let mut stdout = process.stdout().ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::ProtocolError, "failed to attach stdout")
        })?;
        let mut stderr = process.stderr().ok_or_else(|| {
            RemoteError::with_message(RemoteErrorType::ProtocolError, "failed to attach stderr")
        })?;

        // Drain both pipes concurrently: the background task blocks as soon
        // as either duplex buffer is full, so reading them one after the
        // other can deadlock on a chatty command.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let (out_res, err_res) =
            tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        out_res.map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))?;
        if err_res.is_ok() && !err.is_empty() {
            debug!("Shell command stderr: {}", String::from_utf8_lossy(&err));
        }
        drop(stdout);
        drop(stderr);

        process
            .join()
            .await
            .map_err(|err| RemoteError::with_source(RemoteErrorType::ProtocolError, err))?;

        let out = String::from_utf8_lossy(&out);
        let (stdout, rc) = out.rsplit_once(';').ok_or_else(|| {
            RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                "missing exit code in output",
            )
        })?;
        let rc = rc.trim().parse::<u32>().map_err(|_| {
            RemoteError::with_message(
                RemoteErrorType::ProtocolError,
                format!("invalid exit code token: {rc:?}"),
            )
        })?;
        debug!("Shell command exit code: {rc}");
        trace!("Shell command output: {stdout}");
        Ok(ExecOutput::new(rc, stdout))
    }

    /// Run `cmd` and map a non-zero exit code to `failure`.
    pub(crate) async fn shell_ok(&self, cmd: &str, failure: RemoteErrorType) -> RemoteResult<()> {
        let output = self.shell(cmd).await?;
        if output.exit_code == 0 {
            Ok(())
        } else {
            Err(RemoteError::with_message(
                failure,
                format!("command exited with status {}", output.exit_code),
            ))
        }
    }

    /// Run `cmd` and return whether it exited with status 0.
    pub(crate) async fn shell_test(&self, cmd: &str) -> RemoteResult<bool> {
        Ok(self.shell(cmd).await?.exit_code == 0)
    }
}

#[cfg(test)]
mod test {

    #[cfg(unix)]
    #[test]
    fn command_script_preserves_exit_code() {
        use std::process::Command;

        let output = Command::new("/bin/sh")
            .args(["-c", &super::command_script("printf output; exit 3")])
            .output()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"output;3");
    }
}
