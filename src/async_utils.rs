use std::io;
use tokio::io::{AsyncRead, AsyncSeek};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

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
        match reader.read(&mut buf[read..]).await? {
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
