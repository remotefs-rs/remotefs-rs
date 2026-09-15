#![cfg(feature = "with-gcs-ci")]

use std::env;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use remotefs::AsyncRemoteFs;
use remotefs::fs::{ReadOptions, WriteOptions};
use remotefs_gcs::GoogleCloudStorageFs;

fn unique_name(prefix: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before Unix epoch")
        .as_nanos();
    format!("{prefix}-{timestamp}")
}

#[tokio::test]
async fn live_gcs_smoke_test() {
    let Some(bucket) = env::var_os("GCS_TEST_BUCKET") else {
        eprintln!("skipping live GCS test: GCS_TEST_BUCKET is not set");
        return;
    };
    let mut client = GoogleCloudStorageFs::new(bucket.to_string_lossy());
    client
        .connect()
        .await
        .expect("failed to connect to live GCS");

    let prefix = Path::new("/").join(unique_name("remotefs-gcs-live"));
    let result = async {
        client.create_dir(&prefix, None).await?;
        let path = prefix.join("smoke.txt");
        let bytes = b"live GCS smoke test";
        let mut source = futures::io::Cursor::new(bytes.to_vec());
        client
            .write_file(
                &path,
                &WriteOptions::default().size_hint(bytes.len() as u64),
                &mut source,
            )
            .await?;
        if !client.stat(&path).await?.is_file() {
            return Err("live object was not reported as a file".into());
        }
        if client.list_dir(&prefix).await?.len() != 1 {
            return Err("live directory listing had an unexpected length".into());
        }
        let mut downloaded = futures::io::Cursor::new(Vec::new());
        client
            .read_file(&path, &ReadOptions::default(), &mut downloaded)
            .await?;
        if downloaded.into_inner() != bytes {
            return Err("live object contents differed after download".into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    let cleanup = client.remove_dir_all(&prefix).await;
    client
        .disconnect()
        .await
        .expect("failed to disconnect from live GCS");

    if let Err(error) = result {
        panic!("live GCS smoke test failed: {error}");
    }
    cleanup.expect("failed to clean up live GCS test objects");
}
