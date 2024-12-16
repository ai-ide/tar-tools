use std::io;
use std::marker::PhantomData;
use std::path::Path;
use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncSeek};
use crate::header::Header;

/// Fields for entries iterator state
pub(crate) struct AsyncEntriesFields<'a, R: 'a> {
    pub(crate) offset: u64,
    pub(crate) done: bool,
    pub(crate) obj: &'a mut R,
}

/// An asynchronous iterator over the entries in an archive.
pub struct AsyncEntries<'a, R: 'a> {
    pub(crate) fields: AsyncEntriesFields<'a, R>,
    pub(crate) _marker: PhantomData<&'a mut R>,
}

/// An entry within a tar archive
pub struct AsyncEntry<'a, R: 'a> {
    pub(crate) header: Header,
    pub(crate) size: u64,
    pub(crate) pos: u64,
    pub(crate) header_pos: u64,
    pub(crate) file_pos: u64,
    pub(crate) archive: &'a mut R,
    pub(crate) pax_extensions: Option<Vec<u8>>,
    pub(crate) long_pathname: Option<Vec<u8>>,
    pub(crate) long_linkname: Option<Vec<u8>>,
    pub(crate) _marker: PhantomData<&'a ()>,
}

/// Async interface for reading and unpacking tar archives.
#[async_trait]
pub trait AsyncArchive {
    /// Returns an iterator over the entries in this archive.
    async fn entries(&mut self) -> io::Result<AsyncEntries<'_, Self>>
    where
        Self: AsyncRead + AsyncSeek + Sized;

    /// Unpacks the entire archive into the specified directory.
    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()>;
}

/// Async interface for reading and unpacking individual archive entries.
#[async_trait]
pub trait AsyncEntryTrait {
    /// Reads data from this entry into the specified buffer.
    async fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Extracts this entry into the specified directory.
    async fn unpack<P: AsRef<Path> + Send>(&mut self, dst: P) -> io::Result<()>;
}

/// Result of an unpacking operation.
pub struct Unpacked {
    _private: (),
}
