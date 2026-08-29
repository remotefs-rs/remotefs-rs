#![cfg(feature = "with-gcs-ci")]

use std::env;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use remotefs::RemoteFs;
use remotefs::fs::Metadata;
use remotefs_gcs::GoogleCloudStorageFs;
use tokio::runtime::Runtime;

fn unique_name(prefix: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_nanos();
    format!("{prefix}-{timestamp}")
}

#[test]
fn live_gcs_smoke_test() {
    let Some(bucket) = env::var_os("GCS_TEST_BUCKET") else {
        eprintln!("skipping live GCS test: GCS_TEST_BUCKET is not set");
        return;
    };
    let runtime = Arc::new(Runtime::new().expect("failed to create Tokio runtime"));
    let mut client = GoogleCloudStorageFs::new(bucket.to_string_lossy(), &runtime);
    client.connect().expect("failed to connect to live GCS");

    let prefix = unique_name("remotefs-gcs-live");
    let result = (|| {
        client.create_dir(Path::new(&prefix), remotefs::fs::UnixPex::from(0o755))?;
        let path = Path::new(&prefix).join("smoke.txt");
        let bytes = b"live GCS smoke test";
        let metadata = Metadata {
            size: bytes.len() as u64,
            ..Metadata::default()
        };
        client.create_file(
            &path,
            &metadata,
            Box::new(std::io::Cursor::new(bytes.to_vec())),
        )?;
        if !client.stat(&path)?.is_file() {
            return Err("live object was not reported as a file".into());
        }
        if client.list_dir(Path::new(&prefix))?.len() != 1 {
            return Err("live directory listing had an unexpected length".into());
        }

        let mut output = tempfile::tempfile()?;
        client.open_file(&path, Box::new(output.try_clone()?))?;
        output.seek(SeekFrom::Start(0))?;
        let mut downloaded = Vec::new();
        output.read_to_end(&mut downloaded)?;
        if downloaded != bytes {
            return Err("live object contents differed after download".into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    let cleanup = client.remove_dir_all(Path::new(&prefix));
    client
        .disconnect()
        .expect("failed to disconnect from live GCS");

    if let Err(error) = result {
        panic!("live GCS smoke test failed: {error}");
    }
    cleanup.expect("failed to clean up live GCS test objects");
}
