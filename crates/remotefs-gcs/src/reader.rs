use std::fmt;
use std::io::Read;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use google_cloud_storage::streaming_source::StreamingSource;

pub(crate) const READ_CHUNK_SIZE: usize = 256 * 1024;

pub(crate) struct BlockingReaderSource {
    reader: Arc<Mutex<Box<dyn Read + Send>>>,
}

impl BlockingReaderSource {
    pub(crate) fn new(reader: Box<dyn Read + Send>) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
        }
    }
}

impl fmt::Debug for BlockingReaderSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockingReaderSource")
            .finish_non_exhaustive()
    }
}

impl StreamingSource for BlockingReaderSource {
    type Error = std::io::Error;

    async fn next(&mut self) -> Option<Result<Bytes, Self::Error>> {
        let reader = Arc::clone(&self.reader);
        match tokio::task::spawn_blocking(move || {
            let mut buffer = vec![0_u8; READ_CHUNK_SIZE];
            let mut reader = reader
                .lock()
                .map_err(|_error| std::io::Error::other("reader lock poisoned"))?;
            let read = reader.read(&mut buffer)?;
            buffer.truncate(read);
            Ok(Bytes::from(buffer))
        })
        .await
        {
            Ok(Ok(bytes)) if bytes.is_empty() => None,
            Ok(result) => Some(result),
            Err(error) => Some(Err(std::io::Error::other(error))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;

    use google_cloud_storage::streaming_source::StreamingSource;
    use tokio::runtime::Runtime;

    use super::{BlockingReaderSource, READ_CHUNK_SIZE};

    #[test]
    fn blocking_reader_source_yields_chunks_and_eof() {
        let runtime = Runtime::new().unwrap();
        let input = vec![b'x'; READ_CHUNK_SIZE + 17];
        let mut source = BlockingReaderSource::new(Box::new(Cursor::new(input.clone())));

        let chunks = runtime.block_on(async {
            let mut chunks = Vec::new();
            while let Some(chunk) = source.next().await {
                chunks.push(chunk.unwrap());
            }
            chunks
        });

        assert_eq!(
            chunks.iter().map(bytes::Bytes::len).sum::<usize>(),
            input.len()
        );
        assert_eq!(chunks.len(), 2);
        assert_eq!(Arc::strong_count(&source.reader), 1);
    }
}
