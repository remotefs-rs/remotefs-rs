//! Read adapters shared by the SSH clients.

use std::io::{self, Read};

use remotefs::RemoteResult;
use remotefs::fs::{ReadOptions, RemoteRead};

/// A reader bounded to an optional number of bytes.
///
/// `skip_and_limit` additionally consumes and discards the requested read
/// offset up front, which is how protocols without native ranges (SCP) honor
/// `ReadOptions::offset`. A bounded reader is never seekable.
pub(crate) struct RangedRead<R> {
    inner: R,
    remaining: Option<u64>,
}

impl<R> RangedRead<R>
where
    R: Read,
{
    /// Bounds `inner` to `length` bytes without skipping anything.
    pub(crate) fn new(inner: R, length: Option<u64>) -> Self {
        Self {
            inner,
            remaining: length,
        }
    }

    /// Discards `opts.offset` bytes from `inner`, then bounds it to `opts.length`.
    ///
    /// An offset beyond the end of the data yields an empty reader.
    pub(crate) fn skip_and_limit(mut inner: R, opts: &ReadOptions) -> io::Result<Self> {
        if let Some(offset) = opts.offset.filter(|offset| *offset > 0) {
            io::copy(&mut (&mut inner).take(offset), &mut io::sink())?;
        }
        Ok(Self::new(inner, opts.length))
    }
}

impl<R> Read for RangedRead<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.remaining {
            Some(0) => Ok(0),
            Some(remaining) => {
                let max = buf
                    .len()
                    .min(usize::try_from(remaining).unwrap_or(usize::MAX));
                let read = self.inner.read(&mut buf[..max])?;
                self.remaining = Some(remaining - read as u64);
                Ok(read)
            }
            None => self.inner.read(buf),
        }
    }
}

impl<R> RemoteRead for RangedRead<R>
where
    R: RemoteRead,
{
    fn finish(self: Box<Self>) -> RemoteResult<()> {
        Box::new(self.inner).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use pretty_assertions::assert_eq;
    use remotefs::fs::ReadOptions;

    use super::*;

    fn read_all(mut reader: RangedRead<Cursor<&[u8]>>) -> Vec<u8> {
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        output
    }

    #[test]
    fn should_skip_offset_and_limit_length() {
        let reader = RangedRead::skip_and_limit(
            Cursor::new(b"abcdef".as_slice()),
            &ReadOptions::default().offset(2).length(2),
        )
        .unwrap();
        assert_eq!(read_all(reader), b"cd");
    }

    #[test]
    fn should_yield_nothing_for_zero_length() {
        let reader = RangedRead::skip_and_limit(
            Cursor::new(b"abcdef".as_slice()),
            &ReadOptions::default().offset(2).length(0),
        )
        .unwrap();
        assert!(read_all(reader).is_empty());
    }

    #[test]
    fn should_yield_nothing_beyond_eof() {
        let reader = RangedRead::skip_and_limit(
            Cursor::new(b"abcdef".as_slice()),
            &ReadOptions::default().offset(100),
        )
        .unwrap();
        assert!(read_all(reader).is_empty());
    }

    #[test]
    fn should_pass_through_without_options() {
        let reader =
            RangedRead::skip_and_limit(Cursor::new(b"abc".as_slice()), &ReadOptions::default())
                .unwrap();
        assert_eq!(read_all(reader), b"abc");
    }
}
