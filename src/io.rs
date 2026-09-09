//! Bounded runtime-neutral asynchronous I/O helpers.

use std::future::poll_fn;
use std::io::{Error, ErrorKind};
use std::pin::Pin;

use futures_io::{AsyncRead, AsyncWrite};

pub(crate) async fn copy(
    reader: &mut (dyn AsyncRead + Send + Unpin),
    writer: &mut (dyn AsyncWrite + Send + Unpin),
) -> std::io::Result<u64> {
    let mut buffer = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let read = poll_fn(|context| Pin::new(&mut *reader).poll_read(context, &mut buffer)).await;
        let count = match read {
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(total);
        }
        let mut written = 0;
        while written < count {
            let result = poll_fn(|context| {
                Pin::new(&mut *writer).poll_write(context, &buffer[written..count])
            })
            .await;
            match result {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(size) => written += size,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| Error::other("transfer byte count overflow"))?;
    }
}

pub(crate) async fn flush(writer: &mut (dyn AsyncWrite + Send + Unpin)) -> std::io::Result<()> {
    poll_fn(|context| Pin::new(&mut *writer).poll_flush(context)).await
}
