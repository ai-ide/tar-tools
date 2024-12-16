use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::fs::Permissions;
use tokio::io::{AsyncRead, AsyncSeek, AsyncReadExt, AsyncSeekExt};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::async_traits::{AsyncEntryFields, AsyncEntryTrait};
use crate::header::Header;
use crate::async_utils::AsyncMutexReader;

const BLOCK_SIZE: u64 = 512;

/// An entry within a tar archive.
pub struct AsyncEntryReader<R: AsyncRead + AsyncSeek + Unpin + Send + Sync> {
    fields: AsyncEntryFields<R>,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync> tokio::io::AsyncRead for AsyncEntryReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let max = std::cmp::min(buf.remaining() as u64, this.fields.size - this.fields.pos) as usize;
        if max == 0 {
            return Poll::Ready(Ok(()));
        }

        let initial_remaining = buf.remaining();
        let result = {
            let mut guard = this.fields.obj.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
            Pin::new(&mut *guard).poll_read(cx, buf)
        };

        if let Poll::Ready(Ok(())) = result {
            this.fields.pos += (initial_remaining - buf.remaining()) as u64;
        }
        result
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync> tokio::io::AsyncSeek for AsyncEntryReader<R> {
    fn start_seek(mut self: Pin<&mut Self>, pos: tokio::io::SeekFrom) -> io::Result<()> {
        let this = self.get_mut();
        match pos {
            tokio::io::SeekFrom::Start(n) => {
                this.fields.pos = n;
                Ok(())
            }
            tokio::io::SeekFrom::Current(n) => {
                this.fields.pos = this.fields.pos.saturating_add_signed(n);
                Ok(())
            }
            tokio::io::SeekFrom::End(n) => {
                this.fields.pos = this.fields.size.saturating_add_signed(n);
                Ok(())
            }
        }
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(self.get_mut().fields.pos))
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync> AsyncEntryReader<R> {
    /// Creates a new AsyncEntryReader.
    pub(crate) fn new(
        header: Header,
        size: u64,
        header_pos: u64,
        file_pos: u64,
        archive: Arc<Mutex<R>>,
    ) -> AsyncEntryReader<R> {
        AsyncEntryReader {
            fields: AsyncEntryFields {
                header,
                size,
                pos: 0,
                header_pos,
                file_pos,
                obj: archive,
                pax_extensions: None,
                long_pathname: None,
                long_linkname: None,
                _marker: PhantomData,
            },
        }
    }

    /// Returns the header of this entry.
    pub fn header(&self) -> &Header {
        &self.fields.header
    }

    /// Returns the path name for this entry.
    pub fn path(&self) -> io::Result<PathBuf> {
        Ok(self.fields.header.path()?.into_owned().into())
    }

    /// Returns the link name for this entry, if any.
    pub fn link_name(&self) -> io::Result<Option<PathBuf>> {
        Ok(self.fields.header.link_name()?.map(|p| p.into_owned().into()))
    }

    /// Returns the size of the file this entry represents.
    pub fn size(&self) -> u64 {
        self.fields.size
    }

    /// Sets the PAX extensions for this entry.
    pub(crate) fn set_pax_extensions(&mut self, pax: Vec<u8>) {
        self.fields.pax_extensions = Some(pax);
    }

    /// Sets the long pathname for this entry.
    pub(crate) fn set_long_pathname(&mut self, pathname: Vec<u8>) {
        self.fields.long_pathname = Some(pathname);
    }

    /// Sets the long linkname for this entry.
    pub(crate) fn set_long_linkname(&mut self, linkname: Vec<u8>) {
        self.fields.long_linkname = Some(linkname);
    }
}

#[async_trait]
impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync> AsyncEntryTrait for AsyncEntryReader<R> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.fields.pos >= self.fields.size {
            return Ok(0);
        }

        // Perform seek and read operations with AsyncMutexReader
        let archive_pos = self.fields.file_pos + self.fields.pos;
        let mut reader = AsyncMutexReader::new(self.fields.obj.clone());
        reader.seek(tokio::io::SeekFrom::Start(archive_pos)).await?;

        let amt = std::cmp::min(buf.len() as u64, self.fields.size - self.fields.pos) as usize;
        let mut read_buf = tokio::io::ReadBuf::new(&mut buf[..amt]);
        Pin::new(&mut reader).poll_read(&mut Context::from_waker(futures::task::noop_waker_ref()), &mut read_buf)?;

        let n = read_buf.filled().len();
        self.fields.pos += n as u64;
        Ok(n)
    }

    async fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let mut data = Vec::with_capacity(self.fields.size as usize);
        let mut buf = [0u8; 8192];
        while let Ok(n) = AsyncReadExt::read(self, &mut buf).await {
            if n == 0 { break; }
            data.extend_from_slice(&buf[..n]);
        }
        Ok(data)
    }

    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()> {
        let dst = dst.as_ref();
        let path = dst.join(self.path()?);

        // Create parent directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        match self.fields.header.entry_type() {
            crate::entry_type::EntryType::Regular => {
                let mut file = fs::File::create(&path).await?;
                let mut buf = vec![0; 8192];
                while let Ok(n) = AsyncReadExt::read(self, &mut buf).await {
                    if n == 0 { break; }
                    file.write_all(&buf[..n]).await?;
                }
            }
            crate::entry_type::EntryType::Directory => {
                fs::create_dir_all(&path).await?;
            }
            crate::entry_type::EntryType::Symlink => {
                let src = self.link_name()?.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "symlink missing target")
                })?;
                tokio::fs::symlink(&src, &path).await?;
            }
            _ => {
                // Handle other entry types as needed
                return Ok(());
            }
        }

        // Set permissions if available
        #[cfg(unix)]
        if let Ok(mode) = self.fields.header.mode() {
            use std::os::unix::fs::PermissionsExt;
            let perm = Permissions::from_mode(mode);
            fs::set_permissions(&path, perm).await?;
        }

        Ok(())
    }
}
