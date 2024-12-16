use std::io;
use std::marker::PhantomData;
use std::path::Path;
use futures::io::{AsyncRead, AsyncSeek};
use async_trait::async_trait;

use crate::async_traits::{AsyncArchive, AsyncEntries, AsyncEntriesFields, AsyncEntry};
use crate::async_utils::{try_read_all_async, seek_relative};
use crate::header::Header;

const BLOCK_SIZE: u64 = 512;

/// An asynchronous tar archive reader.
pub struct AsyncArchiveReader<R> {
    inner: ArchiveInner<R>,
}

struct ArchiveInner<R> {
    obj: R,
    pos: u64,
    unpack_xattrs: bool,
    preserve_permissions: bool,
    preserve_mtime: bool,
    preserve_ownerships: bool,
    overwrite: bool,
    ignore_zeros: bool,
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send> AsyncArchiveReader<R> {
    /// Creates a new archive with the underlying object as the reader.
    pub fn new(obj: R) -> AsyncArchiveReader<R> {
        AsyncArchiveReader {
            inner: ArchiveInner {
                obj,
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
    pub fn set_mask(&mut self, mask: Option<u32>) -> &mut Self {
        // PLACEHOLDER: existing implementation
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
impl<R: AsyncRead + AsyncSeek + Unpin + Send> AsyncArchive for AsyncArchiveReader<R> {
    async fn entries(&mut self) -> io::Result<AsyncEntries<'_, R>> {
        Ok(AsyncEntries {
            fields: AsyncEntriesFields {
                offset: self.inner.pos,
                done: false,
                obj: &mut self.inner.obj,
            },
            _marker: PhantomData,
        })
    }

    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()> {
        let dst = dst.as_ref();
        let mut entries = self.entries().await?;

        while let Some(entry) = entries.next().await? {
            let mut entry = entry;
            entry.unpack(dst).await?;
        }

        Ok(())
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send> AsyncRead for AsyncArchiveReader<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<io::Result<usize>> {
        AsyncRead::poll_read(std::pin::Pin::new(&mut self.inner.obj), cx, buf)
    }
}

impl<R: AsyncRead + AsyncSeek + Unpin + Send> AsyncSeek for AsyncArchiveReader<R> {
    fn poll_seek(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        pos: futures::io::SeekFrom,
    ) -> std::task::Poll<io::Result<u64>> {
        AsyncSeek::poll_seek(std::pin::Pin::new(&mut self.inner.obj), cx, pos)
    }
}

impl<'a, R: AsyncRead + AsyncSeek + Unpin + Send> AsyncEntries<'a, R> {
    /// Advances the iterator, returning the next entry.
    pub async fn next(&mut self) -> io::Result<Option<AsyncEntry<'a, R>>> {
        if self.fields.done {
            return Ok(None);
        }

        match self.next_entry_raw().await? {
            Some(entry) => Ok(Some(entry)),
            None => {
                self.fields.done = true;
                Ok(None)
            }
        }
    }

    async fn next_entry_raw(&mut self) -> io::Result<Option<AsyncEntry<'a, R>>> {
        let header_pos = self.fields.offset;
        let mut header = [0; 512];

        // Skip to where we want to read
        let delta = header_pos as i64 - self.fields.offset as i64;
        if delta != 0 {
            seek_relative(self.fields.obj, delta).await?;
            self.fields.offset = header_pos;
        }

        // Read the header
        if !try_read_all_async(self.fields.obj, &mut header).await? {
            self.fields.done = true;
            return Ok(None);
        }
        self.fields.offset += BLOCK_SIZE;

        // Validate the header
        let sum = Header::new_old();
        sum.as_bytes_mut().copy_from_slice(&header);
        if !sum.as_bytes().iter().all(|i| *i == 0) {
            // Try to figure out if we're at the end of the archive or not
            let is_zero = header.iter().all(|i| *i == 0);
            if is_zero {
                self.fields.done = true;
                return Ok(None);
            }
        }

        let header = sum;

        let file_pos = self.fields.offset;
        let size = header.size()?;

        let entry = AsyncEntry {
            header,
            size,
            pos: 0,
            header_pos,
            file_pos,
            archive: self.fields.obj,
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

    async fn next_entry(&mut self) -> io::Result<Option<AsyncEntry<'a, R>>> {
        loop {
            match self.next_entry_raw().await? {
                Some(entry) => {
                    let header = entry.header;
                    let is_recognized_header = header.as_gnu().is_some() ||
                        header.as_ustar().is_some() ||
                        header.as_old().is_some();

                    if is_recognized_header {
                        return Ok(Some(entry));
                    }
                }
                None => return Ok(None),
            }
        }
    }

    async fn skip(&mut self, mut amt: u64) -> io::Result<()> {
        self.fields.offset += amt;
        Ok(())
    }
}
