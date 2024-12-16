use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncSeek, AsyncSeekExt};
use tokio::fs;

use crate::async_traits::{AsyncEntry, Unpacked};
use crate::header::Header;
use crate::error::TarError;

const BLOCK_SIZE: u64 = 512;

/// An entry within a tar archive.
pub struct AsyncEntryReader<'a, R: AsyncRead + AsyncSeek + Unpin + Send> {
    header: Header,
    size: u64,
    pos: u64,
    header_pos: u64,
    file_pos: u64,
    archive: &'a mut R,
    pax_extensions: Option<Vec<u8>>,
    long_pathname: Option<Vec<u8>>,
    long_linkname: Option<Vec<u8>>,
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
            header,
            size,
            pos: 0,
            header_pos,
            file_pos,
            archive,
            pax_extensions: None,
            long_pathname: None,
            long_linkname: None,
        }
    }

    /// Returns the header of this entry.
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Returns the path name for this entry.
    pub fn path(&self) -> io::Result<PathBuf> {
        self.header.path()
    }

    /// Returns the link name for this entry, if any.
    pub fn link_name(&self) -> io::Result<Option<PathBuf>> {
        self.header.link_name()
    }

    /// Returns the size of the file this entry represents.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Sets the PAX extensions for this entry.
    pub(crate) fn set_pax_extensions(&mut self, pax: Vec<u8>) {
        self.pax_extensions = Some(pax);
    }

    /// Sets the long pathname for this entry.
    pub(crate) fn set_long_pathname(&mut self, pathname: Vec<u8>) {
        self.long_pathname = Some(pathname);
    }

    /// Sets the long linkname for this entry.
    pub(crate) fn set_long_linkname(&mut self, linkname: Vec<u8>) {
        self.long_linkname = Some(linkname);
    }

    /// Reads all bytes in this entry.
    pub async fn read_all(&mut self) -> io::Result<Vec<u8>> {
        let mut data = Vec::with_capacity(self.size as usize);
        let mut buf = [0u8; 4096];

        while let Ok(n) = self.read(&mut buf).await {
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
        }

        Ok(data)
    }
}

#[async_trait]
impl<'a, R: AsyncRead + AsyncSeek + Unpin + Send> AsyncEntry for AsyncEntryReader<'a, R> {
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.size {
            return Ok(0);
        }

        // Seek to the correct position if necessary
        let archive_pos = self.file_pos + self.pos;
        self.archive.seek(futures::io::SeekFrom::Start(archive_pos)).await?;

        // Read the data
        let max = std::cmp::min(buf.len() as u64, self.size - self.pos) as usize;
        let n = self.archive.read(&mut buf[..max]).await?;
        self.pos += n as u64;
        Ok(n)
    }

    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()> {
        let dst = dst.as_ref();
        let path = dst.join(self.path()?);

        // Create parent directories
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }

        match self.header.entry_type() {
            crate::entry_type::EntryType::Regular => {
                let mut file = fs::File::create(&path).await?;
                tokio::io::copy(&mut *self, &mut file).await?;
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
        if let Ok(mode) = self.header.mode() {
            use std::os::unix::fs::PermissionsExt;
            let perm = fs::Permissions::from_mode(mode);
            fs::set_permissions(&path, perm).await?;
        }

        Ok(())
    }
}
