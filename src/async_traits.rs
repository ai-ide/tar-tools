use std::io;
use std::path::Path;
use std::marker;
use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncSeek};
use crate::header::Header;

/// An asynchronous iterator over the entries in an archive.
pub struct AsyncEntries<'a, R: 'a> {
    pub(crate) fields: AsyncEntriesFields<'a, R>,
    pub(crate) _marker: marker::PhantomData<&'a mut AsyncArchiveReader<R>>,
}

/// An entry in a tar archive.
pub struct AsyncEntry<'a, R: 'a> {
    pub(crate) header: Header,
    pub(crate) size: u64,
    pub(crate) header_pos: u64,
    pub(crate) file_pos: u64,
    pub(crate) archive: &'a mut AsyncArchiveReader<R>,
    pub(crate) _marker: marker::PhantomData<&'a ()>,
}

/// Async interface for reading and unpacking tar archives.
#[async_trait]
pub trait AsyncArchive {
    /// Returns an iterator over the entries in this archive.
    async fn entries(&mut self) -> io::Result<AsyncEntries<'_>>;

    /// Unpacks the entire archive into the specified directory.
    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()>;
}

/// Async interface for reading and unpacking individual archive entries.
#[async_trait]
pub trait AsyncEntry {
    /// Reads data from this entry into the specified buffer.
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Extracts this entry into the specified directory.
    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()>;
}

/// Result of an unpacking operation.
pub struct Unpacked {
    _private: (),
}
