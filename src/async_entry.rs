use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::fs::Permissions;
use futures::io::{AsyncRead, AsyncSeek};
use futures::{AsyncReadExt, AsyncSeekExt};
use tokio::fs;
use async_trait::async_trait;

use crate::async_traits::{AsyncEntry, AsyncEntryTrait};
use crate::header::Header;

const BLOCK_SIZE: u64 = 512;

/// An entry within a tar archive.
pub struct AsyncEntryReader<'a, R: AsyncRead + AsyncSeek + Unpin + Send> {
    fields: AsyncEntry<'a, R>,
}

impl<'a, R: AsyncRead + AsyncSeek + Unpin + Send> AsyncEntryReader<'a, R> {
    /// Creates a new AsyncEntryReader.
    pub(crate) fn new(
        header: Header,
        size: u64,
        header_pos: u64,
        file_pos: u64,
        archive: &'a mut R,
    ) -> AsyncEntryReader<'a, R> {
        AsyncEntryReader {
            fields: AsyncEntry {
                header,
                size,
                pos: 0,
                header_pos,
                file_pos,
                archive,
                pax_extensions: None,
                long_pathname: None,
                long_linkname: None,
                _marker: std::marker::PhantomData,
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
impl<'a, R: AsyncRead + AsyncSeek + Unpin + Send> AsyncEntryTrait for AsyncEntryReader<'a, R> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.fields.pos >= self.fields.size {
            return Ok(0);
        }

        // Seek to the correct position if necessary
        let archive_pos = self.fields.file_pos + self.fields.pos;
        self.fields.archive.seek(futures::io::SeekFrom::Start(archive_pos)).await?;

        // Read the data
        let max = std::cmp::min(buf.len() as u64, self.fields.size - self.fields.pos) as usize;
        let n = self.fields.archive.read(&mut buf[..max]).await?;
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

impl<'a, R: AsyncRead + AsyncSeek + Unpin + Send> tokio::io::AsyncRead for AsyncEntryReader<'a, R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut temp_buf = vec![0u8; buf.remaining()];
        match AsyncRead::poll_read(Pin::new(&mut self.fields.archive), cx, &mut temp_buf) {
            Poll::Ready(Ok(n)) => {
                buf.put_slice(&temp_buf[..n]);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}
