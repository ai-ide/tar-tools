use std::io;
use std::path::Path;
use futures::io::{AsyncRead, AsyncSeek};
use futures::{AsyncReadExt, AsyncSeekExt};
use async_trait::async_trait;

use crate::async_traits::{AsyncArchive, AsyncEntries, AsyncEntriesFields, AsyncEntryFields};
use crate::async_utils::try_read_all_async;
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
}

impl<R: AsyncRead + AsyncSeek + Unpin> AsyncArchiveReader<R> {
    /// Creates a new archive with the underlying object as the reader.
    pub fn new(obj: R) -> AsyncArchiveReader<R> {
        AsyncArchiveReader {
            inner: ArchiveInner {
                obj,
                pos: 0,
                unpack_xattrs: false,
                preserve_permissions: false,
                preserve_mtime: true,
                preserve_ownerships: false,
                overwrite: false,
                ignore_zeros: false,
            },
        }
    }

    /// Sets the mask for file permissions when unpacking.
    pub fn set_mask(&mut self, mask: Option<u32>) -> &mut Self {
        self.inner.mask = mask;
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
    async fn entries(&mut self) -> io::Result<AsyncEntries<'_, Self>>
    where
        Self: AsyncRead + AsyncSeek + Sized,
    {
        Ok(AsyncEntries {
            fields: AsyncEntriesFields {
                offset: self.inner.pos,
                done: false,
                obj: &mut self.inner.obj,
            },
            _marker: std::marker::PhantomData,
        })
    }

    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()> {
        let dst = dst.as_ref();

        // Create destination directory if it doesn't exist
        if dst.symlink_metadata().is_err() {
            tokio::fs::create_dir_all(&dst).await
                .map_err(|e| io::Error::new(
                    io::ErrorKind::Other,
                    format!("failed to create `{}`", dst.display())
                ))?;
        }

        // Get canonical path to handle extended-length paths on Windows
        let dst = &tokio::fs::canonicalize(&dst).await
            .unwrap_or_else(|_| dst.to_path_buf());

        // Delay directory entries until the end to handle permissions correctly
        let mut directories = Vec::new();

        // Process all entries
        let mut entries = self.entries().await?;
        while let Some(entry) = entries.next().await? {
            let mut file = entry?;
            if file.header().entry_type().is_dir() {
                directories.push(file);
            } else {
                file.unpack_in(dst).await?;
            }
        }

        // Sort directories in reverse order to handle nested directories correctly
        directories.sort_by(|a, b| b.path_bytes().cmp(&a.path_bytes()));
        for mut dir in directories {
            dir.unpack_in(dst).await?;
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
        if self.fields.raw {
            self.next_entry_raw(None).await
        } else {
            self.next_entry().await
        }
    }

    async fn next_entry_raw(
        &mut self,
        pax_extensions: Option<&[u8]>,
    ) -> io::Result<Option<AsyncEntry<'a, R>>> {
        let mut header = Header::new_old();
        let header_pos = self.fields.next;

        loop {
            // Seek to next header position
            let delta = self.fields.next - self.fields.archive.inner.pos;
            self.skip(delta).await?;

            // Read header block
            if !try_read_all_async(&mut self.fields.archive.inner.obj, header.as_mut_bytes()).await? {
                return Ok(None);
            }

            // Check if header is empty (end of archive)
            if !header.as_bytes().iter().all(|i| *i == 0) {
                self.fields.next += BLOCK_SIZE;
                break;
            }

            if !self.fields.archive.inner.ignore_zeros {
                return Ok(None);
            }
            self.fields.next += BLOCK_SIZE;
        }

        // Verify checksum
        let sum = header.as_bytes()[..148]
            .iter()
            .chain(&header.as_bytes()[156..])
            .fold(0, |a, b| a + (*b as u32))
            + 8 * 32;
        let cksum = header.cksum()?;
        if sum != cksum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "archive header checksum mismatch",
            ));
        }

        // Create entry
        let file_pos = self.fields.next;
        let size = header.entry_size()?;

        let entry = AsyncEntry {
            header,
            size,
            header_pos,
            file_pos,
            archive: self.fields.archive,
            _marker: marker::PhantomData,
        };

        // Update position for next entry
        let size = (size + BLOCK_SIZE - 1) & !(BLOCK_SIZE - 1);
        self.fields.next = self
            .fields.next
            .checked_add(size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "size overflow"))?;

        Ok(Some(entry))
    }


    async fn next_entry(&mut self) -> io::Result<Option<AsyncEntry<'a, R>>> {
        if self.fields.raw {
            return self.next_entry_raw(None).await;
        }

        let mut gnu_longname = None;
        let mut gnu_longlink = None;
        let mut pax_extensions = None;
        let mut processed = 0;

        loop {
            processed += 1;
            let entry = match self.next_entry_raw(pax_extensions.as_deref()).await? {
                Some(entry) => entry,
                None if processed > 1 => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "members found describing a future member but no future member found",
                    ));
                }
                None => return Ok(None),
            };

            let is_recognized_header =
                entry.header().as_gnu().is_some() || entry.header().as_ustar().is_some();

            if is_recognized_header && entry.header().entry_type().is_gnu_longname() {
                if gnu_longname.is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "two long name entries describing the same member",
                    ));
                }
                gnu_longname = Some(entry.read_all().await?);
                continue;
            }

            let mut entry = entry;
            entry.long_pathname = gnu_longname;
            entry.long_linkname = gnu_longlink;
            entry.pax_extensions = pax_extensions;
            return Ok(Some(entry));
        }
    }

    async fn skip(&mut self, amt: u64) -> io::Result<()> {
        if amt > 0 {
            self.fields.archive.inner.obj.seek(futures::io::SeekFrom::Current(amt as i64)).await?;
            self.fields.archive.inner.pos += amt;
        }
        Ok(())
    }
}
