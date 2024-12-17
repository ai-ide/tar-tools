use std::io;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncSeek};
use async_trait::async_trait;

use crate::{header::Header, other};

use crate::async_traits::{AsyncArchive, AsyncEntries, AsyncEntriesFields, AsyncEntry, AsyncEntryTrait};
use crate::async_utils::{try_read_all_async, seek_relative, AsyncMutexReader};

const BLOCK_SIZE: u64 = 512;

/// An asynchronous tar archive reader.
#[derive(Clone)]
pub struct AsyncArchiveReader<R: AsyncRead + AsyncSeek + Unpin + Send + Clone> {
    inner: ArchiveInner<R>,
}

#[derive(Clone)]
struct ArchiveInner<R> {
    obj: Arc<Mutex<R>>,
    pos: u64,
    unpack_xattrs: bool,
    preserve_permissions: bool,
    preserve_mtime: bool,
    preserve_ownerships: bool,
    overwrite: bool,
    ignore_zeros: bool,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Clone> AsyncArchiveReader<R> {
    /// Creates a new archive with the underlying object as the reader.
    pub fn new(obj: R) -> AsyncArchiveReader<R> {
        AsyncArchiveReader {
            inner: ArchiveInner {
                obj: Arc::new(Mutex::new(obj)),
                pos: 0,
                unpack_xattrs: false,
                preserve_permissions: true,
                preserve_mtime: true,
                preserve_ownerships: true,
                overwrite: false,
                ignore_zeros: false,
            },
        }
    }

    /// Sets the mask for file permissions when unpacking.
    pub fn set_mask(&mut self, _mask: Option<u32>) -> &mut Self {
        self
    }

    /// Indicates whether extended file attributes (xattrs) should be preserved.
    pub fn set_unpack_xattrs(&mut self, unpack_xattrs: bool) -> &mut Self {
        self.inner.unpack_xattrs = unpack_xattrs;
        self
    }

    /// Indicates whether file permissions should be preserved.
    pub fn set_preserve_permissions(&mut self, preserve: bool) -> &mut Self {
        self.inner.preserve_permissions = preserve;
        self
    }

    /// Indicates whether file modification times should be preserved.
    pub fn set_preserve_mtime(&mut self, preserve: bool) -> &mut Self {
        self.inner.preserve_mtime = preserve;
        self
    }

    /// Indicates whether file ownership should be preserved.
    pub fn set_preserve_ownerships(&mut self, preserve: bool) -> &mut Self {
        self.inner.preserve_ownerships = preserve;
        self
    }

    /// Indicates whether existing files should be overwritten.
    pub fn set_overwrite(&mut self, overwrite: bool) -> &mut Self {
        self.inner.overwrite = overwrite;
        self
    }

    /// Indicates whether to ignore zeros at the end of the archive.
    pub fn set_ignore_zeros(&mut self, ignore: bool) -> &mut Self {
        self.inner.ignore_zeros = ignore;
        self
    }
}

#[async_trait]
impl<R: AsyncRead + AsyncSeek + Unpin + Send + Sync + Clone + 'static> AsyncArchive for AsyncArchiveReader<R> {
    async fn entries(&mut self) -> io::Result<AsyncEntries<AsyncArchiveReader<R>>> {
        // Validate first header before creating entries
        let mut header = [0; 512];
        let mut reader = AsyncMutexReader::new(self.inner.obj.clone());
        if !try_read_all_async(&mut reader, &mut header).await? {
            return Err(other("archive has invalid header"));
        }
        // Check if first header is all zeros
        if header.iter().all(|&x| x == 0) {
            return Err(other("archive has invalid header"));
        }
        // Reset position since entries will read the header again
        let mut reader = AsyncMutexReader::new(self.inner.obj.clone());
        seek_relative(&mut reader, 0).await?;

        Ok(AsyncEntries {
            fields: AsyncEntriesFields {
                offset: self.inner.pos,
                done: false,
                obj: self.clone(),
            },
            _marker: PhantomData,
        })
    }

    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()> {
        let mut entries = self.entries().await?;
        while let Ok(Some(entry)) = entries.next().await {
            let mut entry = entry;
            let path_buf = entry.header().path()?.to_path_buf();
            let path = dst.as_ref().join(path_buf.strip_prefix("/").unwrap_or(&path_buf));
            AsyncEntryTrait::unpack(&mut entry, &path).await?;
        }
        Ok(())
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Clone> AsyncRead for AsyncArchiveReader<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let mut guard = self.inner.obj.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        Pin::new(&mut *guard).poll_read(cx, buf)
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Clone> AsyncSeek for AsyncArchiveReader<R> {
    fn start_seek(self: Pin<&mut Self>, pos: tokio::io::SeekFrom) -> io::Result<()> {
        let mut guard = self.inner.obj.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        Pin::new(&mut *guard).start_seek(pos)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        let mut guard = self.inner.obj.lock().map_err(|_| io::Error::new(io::ErrorKind::Other, "lock poisoned"))?;
        Pin::new(&mut *guard).poll_complete(cx)
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send + Clone> AsyncEntries<AsyncArchiveReader<R>> {
    /// Advances the iterator, returning the next entry.
    pub async fn next(&mut self) -> io::Result<Option<AsyncEntry<R>>> {
        if self.fields.done {
            return Ok(None);
        }

        // Let next_entry_raw handle all header validation
        match self.next_entry_raw().await {
            Ok(Some(entry)) => Ok(Some(entry)),
            Ok(None) => {
                self.fields.done = true;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    async fn next_entry_raw(&mut self) -> io::Result<Option<AsyncEntry<R>>> {
        let header_pos = self.fields.offset;
        let mut header = [0; 512];

        // Skip to where we want to read
        let delta = header_pos as i64 - self.fields.offset as i64;
        if delta != 0 {
            let mut reader = AsyncMutexReader::new(self.fields.obj.inner.obj.clone());
            seek_relative(&mut reader, delta).await?;
            self.fields.offset = header_pos;
        }

        // Read the header
        let mut reader = AsyncMutexReader::new(self.fields.obj.inner.obj.clone());
        if !try_read_all_async(&mut reader, &mut header).await? {
            self.fields.done = true;
            return Ok(None);
        }
        self.fields.offset += BLOCK_SIZE;

        // First check if it's all zeros (end of archive)
        let is_zero = header.iter().all(|i| *i == 0);
        if is_zero {
            // All-zero header at start of archive is an error
            if self.fields.offset == BLOCK_SIZE {
                return Err(other("archive has invalid header"));
            }
            // All-zero header after valid entries indicates end of archive
            if !self.fields.obj.inner.ignore_zeros {
                self.fields.done = true;
                return Ok(None);
            }
            return Err(other("archive header all zeros but ignore_zeros is true"));
        }

        // Check if all bytes are valid ASCII
        if header.iter().any(|&b| b != 0 && !b.is_ascii()) {
            return Err(other("archive header contains invalid bytes"));
        }

        // Only check ustar magic if basic validation passed
        let magic = &header[257..265];
        if magic != b"ustar\x0000" && magic != b"ustar  \x00" {
            return Err(other("archive header not recognized"));
        }

        // Validate checksum field format and value
        let cksum_valid = header[148..156]
            .iter()
            .all(|&b| b == b' ' || b == 0 || (b >= b'0' && b <= b'7'));
        if !cksum_valid {
            return Err(other("archive header checksum field contains invalid characters"));
        }

        let sum = header[..148]
            .iter()
            .chain(&header[156..])
            .fold(0, |a, b| a + (*b as u32))
            + 8 * 32;

        let cksum = u32::from_str_radix(
            std::str::from_utf8(&header[148..156])
                .map_err(|_| other("invalid header checksum"))?,
            8,
        ).map_err(|_| other("invalid header checksum"))?;

        if sum != cksum {
            return Err(other("archive header checksum mismatch"));
        }

        // Parse header
        let header = Header::from_byte_slice(&header);

        let file_pos = self.fields.offset;
        let size = header.size()?;

        let entry = AsyncEntry {
            header: header.clone(),
            size,
            pos: 0,
            header_pos,
            file_pos,
            obj: self.fields.obj.inner.obj.clone(),
            pax_extensions: None,
            long_pathname: None,
            long_linkname: None,
            _marker: PhantomData,
        };

        // Skip to the next file header
        let size = (size + (BLOCK_SIZE - 1)) & !(BLOCK_SIZE - 1);
        self.fields.offset += size;

        Ok(Some(entry))
    }

    async fn next_entry(&mut self) -> io::Result<Option<AsyncEntry<R>>> {
        let mut entry_result = self.next_entry_raw().await?;

        while let Some(entry) = entry_result {
            let is_recognized_header = entry.header.as_ustar().is_some() ||
                entry.header.as_gnu().is_some() ||
                !entry.header.as_bytes().iter().all(|&x| x == 0);

            if is_recognized_header {
                return Ok(Some(entry));
            }

            self.skip().await?;
            entry_result = self.next_entry_raw().await?;
        }

        Ok(None)
    }

    async fn skip(&mut self) -> io::Result<()> {
        // Skip to the next block boundary
        let size = (self.fields.offset + BLOCK_SIZE - 1) & !(BLOCK_SIZE - 1);
        let mut reader = AsyncMutexReader::new(self.fields.obj.inner.obj.clone());
        seek_relative(&mut reader, size as i64).await
    }
}
