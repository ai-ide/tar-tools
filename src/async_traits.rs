use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncSeek};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use crate::header::Header;
use crate::async_utils::AsyncMutexReader;

/// Fields for managing entries iteration state
#[derive(Clone)]
pub(crate) struct AsyncEntriesFields<R> {
    pub(crate) offset: u64,
    pub(crate) done: bool,
    pub(crate) obj: R,
}

/// Fields for managing entry reading state
pub struct AsyncEntryFields<R> {
    pub(crate) header: Header,
    pub(crate) size: u64,
    pub(crate) pos: u64,
    pub(crate) header_pos: u64,
    pub(crate) file_pos: u64,
    pub(crate) obj: Arc<Mutex<R>>,
    pub(crate) pax_extensions: Option<Vec<u8>>,
    pub(crate) long_pathname: Option<Vec<u8>>,
    pub(crate) long_linkname: Option<Vec<u8>>,
    pub(crate) _marker: PhantomData<R>,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync> AsyncRead for AsyncEntryFields<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let result = {
            let mut guard = self.obj.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
            Pin::new(&mut *guard).poll_read(cx, buf)
        };

        if let Poll::Ready(Ok(())) = result {
            let this = self.get_mut();
            this.pos += buf.filled().len() as u64;
        }
        result
    }
}

/// An asynchronous iterator over the entries in an archive.
#[derive(Clone)]
pub struct AsyncEntries<R> {
    pub(crate) fields: AsyncEntriesFields<R>,
    pub(crate) _marker: PhantomData<R>,
}

/// An entry within a tar archive
pub struct AsyncEntry<R> {
    pub(crate) header: Header,
    pub(crate) size: u64,
    pub(crate) pos: u64,
    pub(crate) header_pos: u64,
    pub(crate) file_pos: u64,
    pub(crate) obj: Arc<Mutex<R>>,
    pub(crate) pax_extensions: Option<Vec<u8>>,
    pub(crate) long_pathname: Option<Vec<u8>>,
    pub(crate) long_linkname: Option<Vec<u8>>,
    pub(crate) _marker: PhantomData<R>,
}

impl<R> AsyncEntry<R> {
    pub fn header(&self) -> &Header {
        &self.header
    }
}

/// Async interface for reading tar archives.
#[async_trait]
pub trait AsyncArchive: AsyncRead + AsyncSeek + Sized + Clone {
    /// Returns an iterator over the entries in this archive.
    async fn entries(&mut self) -> io::Result<AsyncEntries<Self>>;

    /// Unpacks the entire archive into the specified directory.
    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()>;
}

/// Async interface for reading and unpacking individual archive entries.
#[async_trait]
pub trait AsyncEntryTrait {
    /// Reads data from this entry into the specified buffer.
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Reads all data from this entry into a buffer.
    async fn read_all(&mut self) -> io::Result<Vec<u8>>;

    /// Unpacks this entry into the specified directory.
    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()>;
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync + 'static> AsyncRead for AsyncEntry<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos >= self.size {
            return Poll::Ready(Ok(()));
        }

        let remaining = buf.remaining() as u64;
        let to_read = std::cmp::min(remaining, self.size - self.pos) as usize;
        let mut reader = AsyncMutexReader::new(self.obj.clone());
        buf.limit(to_read);

        match Pin::new(&mut reader).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let this = self.get_mut();
                this.pos += buf.filled().len() as u64;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync + 'static> AsyncSeek for AsyncEntry<R> {
    fn start_seek(self: Pin<&mut Self>, pos: tokio::io::SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        match pos {
            tokio::io::SeekFrom::Start(n) => {
                this.pos = n;
                Ok(())
            }
            tokio::io::SeekFrom::Current(n) => {
                this.pos = this.pos.saturating_add_signed(n);
                Ok(())
            }
            tokio::io::SeekFrom::End(n) => {
                this.pos = this.size.saturating_add_signed(n);
                Ok(())
            }
        }
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(self.get_mut().pos))
    }
}

#[async_trait]
impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync + 'static> AsyncEntryTrait for AsyncEntry<R> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size {
            return Ok(0);
        }
        let amt = std::cmp::min(buf.len() as u64, self.size - self.pos) as usize;
        let mut read_buf = tokio::io::ReadBuf::new(&mut buf[..amt]);
        {
            let mut guard = self.obj.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
            Pin::new(&mut *guard).poll_read(&mut Context::from_waker(futures::task::noop_waker_ref()), &mut read_buf)?;
        }
        let n = read_buf.filled().len();
        self.pos += n as u64;
        Ok(n)
    }

    async fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let mut buf = vec![0; self.size as usize];
        let mut pos = 0;
        while pos < buf.len() {
            match AsyncEntryTrait::read(self, &mut buf[pos..]).await? {
                0 => break,
                n => pos += n,
            }
        }
        buf.truncate(pos);
        Ok(buf)
    }

    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()> {
        let dst = dst.as_ref();
        let path = dst.join(self.header.path()?);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        match self.header.entry_type() {
            crate::entry_type::EntryType::Regular => {
                let mut file = fs::File::create(&path).await?;
                let mut buf = vec![0; 8192];
                while let Ok(n) = AsyncEntryTrait::read(self, &mut buf).await {
                    if n == 0 { break; }
                    file.write_all(&buf[..n]).await?;
                }
            }
            crate::entry_type::EntryType::Directory => {
                fs::create_dir_all(&path).await?;
            }
            crate::entry_type::EntryType::Symlink => {
                if let Some(link_name) = self.header.link_name()? {
                    fs::symlink(&link_name, &path).await?;
                }
            }
            _ => {}
        }

        Ok(())
    }
}

/// Result of an unpacking operation.
pub struct Unpacked {
    _private: (),
}
