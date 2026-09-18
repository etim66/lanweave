//! Local sequential transfer engine: bounded reads, hashing, and writes.
//!
//! The sender streams one reviewed file into a bounded channel; the recipient
//! writes chunks into a restrictive partial file and verifies the exact size
//! and SHA-256 digest before finalization. The session owner connects this
//! engine to `DATA` frames in a later feature.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use crate::protocol::{FileEntry, FileFailure};
use crate::storage::{Destination, StorageError, TemporaryFile};

/// Bytes moved per chunk, at most one allowed `DATA` body.
pub(crate) const CHUNK_SIZE: usize = 65_536;
/// Capacity of the bounded channel between sender and recipient.
pub(crate) const DATA_CHANNEL_CAPACITY: usize = 4;

/// Why a reviewed source could not be streamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceError {
    /// The path is missing, is not a regular file, or changed since review.
    Changed,
    /// The file could not be read or the sink stopped accepting bytes.
    Io,
}

/// Streams one reviewed file into `sink` and returns its SHA-256 digest.
///
/// The source is re-checked immediately before reading: a file that is no
/// longer a regular file of the reviewed size fails without sending bytes.
pub(crate) async fn send_file(
    path: &Path,
    size: u64,
    sink: &mpsc::Sender<Bytes>,
) -> Result<[u8; 32], SourceError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| SourceError::Changed)?;
    if !metadata.file_type().is_file() || metadata.len() != size {
        return Err(SourceError::Changed);
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| SourceError::Changed)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];

    loop {
        let read = file.read(&mut buffer).await.map_err(|_| SourceError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        sink.send(Bytes::copy_from_slice(&buffer[..read]))
            .await
            .map_err(|_| SourceError::Io)?;
    }

    Ok(hasher.finalize().into())
}

/// One in-progress received file: partial file, hash, and running count.
pub(crate) struct IncomingFile {
    temp: TemporaryFile,
    hasher: Sha256,
    written: u64,
    expected: u64,
}

impl IncomingFile {
    /// Prepares the destination partial file for one manifest entry.
    pub(crate) async fn begin(
        destination: &Destination,
        entry: &FileEntry,
    ) -> Result<Self, StorageError> {
        Ok(Self {
            temp: destination.begin_file(entry).await?,
            hasher: Sha256::new(),
            written: 0,
            expected: entry.size,
        })
    }

    /// Writes one DATA chunk, rejecting bytes beyond the declared size.
    pub(crate) async fn write(&mut self, chunk: &[u8]) -> Result<(), FileFailure> {
        if self.written.saturating_add(chunk.len() as u64) > self.expected {
            return Err(FileFailure::SizeMismatch);
        }
        self.temp
            .write_all(chunk)
            .await
            .map_err(|_| FileFailure::WriteFailed)?;
        self.hasher.update(chunk);
        self.written += chunk.len() as u64;
        Ok(())
    }

    /// Verifies the exact size and digest, then publishes the file.
    ///
    /// Dropping an unfinished or failed `IncomingFile` removes the partial.
    pub(crate) async fn finish(self, digest: [u8; 32]) -> Result<(), FileFailure> {
        if self.written != self.expected {
            return Err(FileFailure::SizeMismatch);
        }
        let actual: [u8; 32] = self.hasher.finalize().into();
        if actual != digest {
            return Err(FileFailure::HashMismatch);
        }
        self.temp.finalize().await.map_err(|error| match error {
            StorageError::DestinationExists => FileFailure::DestinationExists,
            _ => FileFailure::WriteFailed,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tokio::sync::mpsc;

    use super::{CHUNK_SIZE, DATA_CHANNEL_CAPACITY, IncomingFile, SourceError, send_file};
    use crate::protocol::{FileEntry, FileFailure};
    use crate::storage::Destination;

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lanweave-engine-{tag}-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn entry(name: &str, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_owned(),
            size,
        }
    }

    fn partial_count(root: &Path) -> usize {
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".lanweave-")
            })
            .count()
    }

    /// Streams one source through a bounded channel into a prepared partial.
    async fn stream_one(
        source: &Path,
        size: u64,
        destination: &Destination,
        entry: &FileEntry,
    ) -> Result<(), FileFailure> {
        let (sender, mut receiver) = mpsc::channel(DATA_CHANNEL_CAPACITY);
        let path = source.to_owned();
        let sender_task = tokio::spawn(async move { send_file(&path, size, &sender).await });

        let mut incoming = IncomingFile::begin(destination, entry)
            .await
            .expect("prepare destination");
        while let Some(chunk) = receiver.recv().await {
            incoming.write(&chunk).await?;
        }
        let digest = sender_task.await.unwrap().expect("stream source");
        incoming.finish(digest).await
    }

    #[tokio::test]
    async fn multi_file_streams_verify_in_order() {
        let root = temp_root("multi");
        let destination = Destination::new(root.join("out"));
        std::fs::create_dir_all(destination.root()).unwrap();

        let large = vec![7u8; CHUNK_SIZE * 2 + 7];
        let sources = [
            ("alpha.bin", large),
            ("empty.bin", Vec::new()),
            ("omega.txt", b"tail".to_vec()),
        ];

        for (name, bytes) in &sources {
            let source = write_source(&root, name, bytes);
            stream_one(
                &source,
                bytes.len() as u64,
                &destination,
                &entry(name, bytes.len() as u64),
            )
            .await
            .unwrap();
        }

        for (name, bytes) in &sources {
            assert_eq!(
                std::fs::read(destination.root().join(name)).unwrap(),
                *bytes,
                "name: {name}"
            );
        }
        assert_eq!(partial_count(destination.root()), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_failed_file_keeps_the_verified_prefix_and_removes_the_partial() {
        let root = temp_root("fail");
        let destination = Destination::new(root.join("out"));
        std::fs::create_dir_all(destination.root()).unwrap();

        let first = write_source(&root, "first.txt", b"first");
        stream_one(&first, 5, &destination, &entry("first.txt", 5))
            .await
            .unwrap();

        let second = write_source(&root, "second.txt", b"second");
        let (sender, mut receiver) = mpsc::channel(DATA_CHANNEL_CAPACITY);
        let path = second.clone();
        let sender_task = tokio::spawn(async move { send_file(&path, 6, &sender).await });
        let mut incoming = IncomingFile::begin(&destination, &entry("second.txt", 6))
            .await
            .unwrap();
        while let Some(chunk) = receiver.recv().await {
            incoming.write(&chunk).await.unwrap();
        }
        let _ = sender_task.await.unwrap();

        // The peer's digest does not match the received bytes.
        assert_eq!(
            incoming.finish([0u8; 32]).await,
            Err(FileFailure::HashMismatch)
        );

        assert_eq!(
            std::fs::read(destination.root().join("first.txt")).unwrap(),
            b"first"
        );
        assert!(!destination.root().join("second.txt").exists());
        assert_eq!(partial_count(destination.root()), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn declared_size_is_enforced() {
        let root = temp_root("sizes");
        let destination = Destination::new(root.join("out"));
        std::fs::create_dir_all(destination.root()).unwrap();

        // Short data fails verification.
        let short = write_source(&root, "short.txt", b"abc");
        let (sender, mut receiver) = mpsc::channel(DATA_CHANNEL_CAPACITY);
        let path = short.clone();
        let sender_task = tokio::spawn(async move { send_file(&path, 3, &sender).await });
        let mut incoming = IncomingFile::begin(&destination, &entry("short.txt", 3))
            .await
            .unwrap();
        let chunk = receiver.recv().await.unwrap();
        assert_eq!(incoming.write(&chunk[..2]).await, Ok(()));
        assert_eq!(
            incoming.finish([0u8; 32]).await,
            Err(FileFailure::SizeMismatch)
        );
        let _ = sender_task.await;

        // Excess bytes are rejected before the digest check.
        let mut incoming = IncomingFile::begin(&destination, &entry("short.txt", 3))
            .await
            .unwrap();
        assert_eq!(incoming.write(&chunk).await, Ok(()));
        assert_eq!(incoming.write(b"x").await, Err(FileFailure::SizeMismatch));
        drop(incoming);

        assert_eq!(partial_count(destination.root()), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_changed_source_is_not_streamed() {
        let root = temp_root("changed");
        let source = write_source(&root, "gone.txt", b"original");
        std::fs::write(&source, b"changed size").unwrap();

        let (sender, mut receiver) = mpsc::channel(DATA_CHANNEL_CAPACITY);
        assert_eq!(
            send_file(&source, 8, &sender).await,
            Err(SourceError::Changed)
        );
        drop(sender);
        assert!(receiver.recv().await.is_none());

        std::fs::remove_file(&source).unwrap();
        let (sender, _receiver) = mpsc::channel(DATA_CHANNEL_CAPACITY);
        assert_eq!(
            send_file(&source, 8, &sender).await,
            Err(SourceError::Changed)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
