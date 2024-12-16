use std::io::{self, Cursor};
use tar::{
    AsyncArchive, AsyncEntries, AsyncEntry, AsyncEntryReader,
    Header, EntryType,
};

#[tokio::test]
async fn test_async_archive_read_empty() {
    let data = Vec::new();
    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    assert!(entries.next().await.unwrap().is_none());
}

#[tokio::test]
async fn test_async_archive_read_single_file() {
    // Create a simple tar archive with one file
    let mut header = Header::new_gnu();
    header.set_path("test.txt").unwrap();
    header.set_size(4);
    header.set_cksum();

    let mut data = Vec::new();
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(b"test");
    data.extend_from_slice(&[0; 508]); // Padding to block size

    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();

    let entry = entries.next().await.unwrap().unwrap();
    assert_eq!(entry.header().path().unwrap().to_str().unwrap(), "test.txt");
    assert_eq!(entry.header().size().unwrap(), 4);

    let mut contents = Vec::new();
    entry.read_all().await.unwrap();
    assert_eq!(&contents, b"test");

    assert!(entries.next().await.unwrap().is_none());
}

#[tokio::test]
async fn test_async_entry_read() {
    let mut header = Header::new_gnu();
    header.set_path("test.txt").unwrap();
    let content = b"Hello, World!";
    header.set_size(content.len() as u64);
    header.set_cksum();

    let mut data = Vec::new();
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(content);
    // Pad to block size
    data.extend_from_slice(&[0; 499]); // 512 - 13 = 499

    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    let mut entry = entries.next().await.unwrap().unwrap();

    let mut buffer = [0; 5];
    let n = entry.read(&mut buffer).await.unwrap();
    assert_eq!(n, 5);
    assert_eq!(&buffer, b"Hello");

    let n = entry.read(&mut buffer).await.unwrap();
    assert_eq!(n, 5);
    assert_eq!(&buffer[..n], b", Wor");

    let n = entry.read(&mut buffer).await.unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buffer[..n], b"ld!");

    let n = entry.read(&mut buffer).await.unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn test_async_unpack() {
    use tokio::fs;
    use std::path::Path;

    let temp_dir = tempfile::tempdir().unwrap();
    let test_path = temp_dir.path().join("test.txt");
    let test_content = b"Hello, World!";

    // Create test archive
    let mut header = Header::new_gnu();
    header.set_path("test.txt").unwrap();
    header.set_size(test_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();

    let mut data = Vec::new();
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(test_content);
    data.extend_from_slice(&[0; 499]); // Padding

    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    archive.unpack(temp_dir.path()).await.unwrap();

    // Verify unpacked content
    assert!(test_path.exists());
    let content = fs::read(&test_path).await.unwrap();
    assert_eq!(content, test_content);

    // Verify file permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&test_path).await.unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
    }
}

#[tokio::test]
async fn test_async_large_file() {
    let size = 1024 * 1024; // 1MB
    let mut header = Header::new_gnu();
    header.set_path("large.bin").unwrap();
    header.set_size(size);
    header.set_cksum();

    let mut data = Vec::new();
    data.extend_from_slice(header.as_bytes());
    // Create pattern data
    let pattern: Vec<u8> = (0..255).cycle().take(size as usize).collect();
    data.extend_from_slice(&pattern);
    data.extend_from_slice(&vec![0; 512 - (size % 512) as usize]);

    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    let mut entry = entries.next().await.unwrap().unwrap();

    let content = entry.read_all().await.unwrap();
    assert_eq!(content, pattern);
}

#[tokio::test]
async fn test_async_directory_entry() {
    let mut header = Header::new_gnu();
    header.set_path("test_dir/").unwrap();
    header.set_entry_type(EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    header.set_cksum();

    let data = header.as_bytes().to_vec();
    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    let entry = entries.next().await.unwrap().unwrap();

    assert_eq!(entry.header().entry_type(), EntryType::Directory);
    assert_eq!(entry.header().path().unwrap().to_str().unwrap(), "test_dir/");
}

#[tokio::test]
async fn test_async_symlink_entry() {
    let mut header = Header::new_gnu();
    header.set_path("link").unwrap();
    header.set_entry_type(EntryType::Symlink);
    header.set_size(0);
    header.set_link_name("target").unwrap();
    header.set_cksum();

    let data = header.as_bytes().to_vec();
    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    let entry = entries.next().await.unwrap().unwrap();

    assert_eq!(entry.header().entry_type(), EntryType::Symlink);
    assert_eq!(entry.header().link_name().unwrap().unwrap().to_str().unwrap(), "target");
}

#[tokio::test]
async fn test_async_malformed_header() {
    let data = vec![0; 512]; // Invalid header (all zeros)
    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let result = archive.entries().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_async_truncated_archive() {
    let mut header = Header::new_gnu();
    header.set_path("test.txt").unwrap();
    header.set_size(1000);
    header.set_cksum();

    let mut data = Vec::new();
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(b"truncated"); // Not enough data as promised in header

    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    let mut entry = entries.next().await.unwrap().unwrap();

    let result = entry.read_all().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_async_long_filename() {
    let long_name = "a".repeat(200); // Longer than ustar header name field
    let mut header = Header::new_gnu();
    header.set_path(&long_name).unwrap();
    header.set_size(4);
    header.set_cksum();

    let mut data = Vec::new();
    data.extend_from_slice(header.as_bytes());
    data.extend_from_slice(b"test");
    data.extend_from_slice(&[0; 508]); // Padding

    let cursor = Cursor::new(data);
    let mut archive = AsyncArchiveReader::new(cursor);
    let mut entries = archive.entries().await.unwrap();
    let entry = entries.next().await.unwrap().unwrap();

    assert_eq!(entry.header().path().unwrap().to_str().unwrap(), &long_name);
}
