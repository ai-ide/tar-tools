use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncSeek, AsyncReadExt, AsyncSeekExt};

/// A wrapper type that implements AsyncRead and AsyncSeek for Arc<Mutex<R>>
pub(crate) struct AsyncMutexReader<R> {
    inner: Arc<Mutex<R>>,
}

impl<R> AsyncMutexReader<R> {
    pub(crate) fn new(inner: Arc<Mutex<R>>) -> Self {
        Self { inner }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncMutexReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut guard = this.inner.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        Pin::new(&mut *guard).poll_read(cx, buf)
    }
}

impl<R: AsyncSeek + Unpin> AsyncSeek for AsyncMutexReader<R> {
    fn start_seek(self: Pin<&mut Self>, pos: tokio::io::SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        let mut guard = this.inner.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        Pin::new(&mut *guard).start_seek(pos)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let this = self.get_mut();
        let mut guard = this.inner.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        Pin::new(&mut *guard).poll_complete(cx)
    }
}

/// Attempts to read exactly buf.len() bytes into buf.
///
/// Returns Ok(true) if the buffer was completely filled, or Ok(false) if EOF was reached
/// before filling the entire buffer. Returns Err if an I/O error occurs.
pub(crate) async fn try_read_all_async<R: AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut [u8],
) -> io::Result<bool> {
    let mut read = 0;
    while read < buf.len() {
        let mut read_buf = tokio::io::ReadBuf::new(&mut buf[read..]);
        match reader.read_buf(&mut read_buf).await? {
            0 => return Ok(false),
            n => read += n,
        }
    }
    Ok(true)
}

/// Seeks the reader to the specified position from the current position.
///
/// Returns Ok(()) if the seek was successful, or Err if an I/O error occurs.
pub(crate) async fn seek_relative<R: AsyncSeek + Unpin>(
    reader: &mut R,
    offset: i64,
) -> io::Result<()> {
    reader.seek(tokio::io::SeekFrom::Current(offset)).await?;
    Ok(())
}
