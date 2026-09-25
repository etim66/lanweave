//! Safe filesystem boundary for received files.
//!
//! Handles destination name validation, restrictive no-follow temporary file
//! creation, and no-replace finalization. Lanweave never overwrites or
//! silently renames a destination file.
//!
//! Creation and no-replace publication are delegated to the `tempfile` crate
//! with a `.lanweave-*.part` name; this module keeps destination policy,
//! streaming writes, and `StorageError` mapping.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::protocol::{FileEntry, MAX_FILES, is_valid_filename};

/// Destination platform rules applied to every manifest name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Unix,
}

impl Platform {
    /// Returns the rules for the platform this build runs on.
    pub(crate) const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// Why a destination cannot accept a manifest or a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageError {
    /// The manifest count is outside `1..=MAX_FILES`.
    InvalidManifest,
    /// A name is not a safe single filename component.
    InvalidName,
    /// Two names collide on the destination platform.
    NameConflict,
    /// An entry with that name already exists.
    DestinationExists,
    /// The chosen destination root is missing or not a directory.
    InvalidDestination,
    /// Preparation, writing, or finalization failed.
    Io,
}

/// Validates a directory the recipient chose for incoming files.
///
/// Choosing the directory is a local user decision, so a symlinked directory
/// is followed; only the manifest names stay no-follow and no-replace.
pub(crate) fn validate_destination(path: &Path) -> Result<(), StorageError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        _ => Err(StorageError::InvalidDestination),
    }
}

/// Validates one filename component for the destination platform.
pub(crate) fn validate_name(name: &str, platform: Platform) -> Result<(), StorageError> {
    if !is_valid_filename(name) {
        return Err(StorageError::InvalidName);
    }
    if platform == Platform::Windows && !is_windows_name(name) {
        return Err(StorageError::InvalidName);
    }
    Ok(())
}

/// Returns whether a name is usable on Windows.
fn is_windows_name(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    if name.ends_with('.') || name.ends_with(' ') {
        return false;
    }
    if name
        .chars()
        .any(|character| matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name);
    !RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// Returns whether two names collide on the destination platform.
///
/// Windows additionally compares case-insensitively and without trailing
/// dots or spaces. Case-insensitive macOS volumes rely on the no-replace
/// finalization as the safety net instead of name comparison.
fn name_conflict(first: &str, second: &str, platform: Platform) -> bool {
    if first == second {
        return true;
    }
    if platform != Platform::Windows {
        return false;
    }
    let normalize = |name: &str| name.trim_end_matches(['.', ' ']).to_lowercase();
    normalize(first) == normalize(second)
}

/// One local destination directory chosen by the recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Destination {
    root: PathBuf,
}

impl Destination {
    /// Creates a destination rooted at a local directory.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Returns the destination directory.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Validates every manifest entry before the transfer is accepted.
    pub(crate) fn check_manifest(&self, entries: &[FileEntry]) -> Result<(), StorageError> {
        if entries.is_empty() || entries.len() > usize::from(MAX_FILES) {
            return Err(StorageError::InvalidManifest);
        }

        let platform = Platform::current();
        for (index, entry) in entries.iter().enumerate() {
            validate_name(&entry.name, platform)?;
            if entries[..index]
                .iter()
                .any(|previous| name_conflict(&previous.name, &entry.name, platform))
            {
                return Err(StorageError::NameConflict);
            }
            if self.exists(&entry.name) {
                return Err(StorageError::DestinationExists);
            }
        }
        Ok(())
    }

    /// Creates the restrictive partial file for one verified manifest entry.
    pub(crate) async fn begin_file(
        &self,
        entry: &FileEntry,
    ) -> Result<TemporaryFile, StorageError> {
        validate_name(&entry.name, Platform::current())?;
        if self.exists(&entry.name) {
            return Err(StorageError::DestinationExists);
        }
        TemporaryFile::create(&self.root, &entry.name)
    }

    /// Checks for any directory entry, following no links.
    fn exists(&self, name: &str) -> bool {
        std::fs::symlink_metadata(self.root.join(name)).is_ok()
    }
}

/// A restrictive, unpredictable partial file that is removed unless finalized.
#[derive(Debug)]
pub(crate) struct TemporaryFile {
    file: tokio::fs::File,
    temp: tempfile::NamedTempFile,
    final_path: PathBuf,
}

impl TemporaryFile {
    /// Creates a new partial file without following links or replacing anything.
    fn create(root: &Path, name: &str) -> Result<Self, StorageError> {
        let temp = tempfile::Builder::new()
            .prefix(".lanweave-")
            .suffix(".part")
            .tempfile_in(root)
            .map_err(|_| StorageError::Io)?;
        let file = tokio::fs::File::from_std(temp.reopen().map_err(|_| StorageError::Io)?);
        Ok(Self {
            file,
            temp,
            final_path: root.join(name),
        })
    }

    /// Appends verified bytes to the partial file.
    pub(crate) async fn write_all(&mut self, bytes: &[u8]) -> Result<(), StorageError> {
        use tokio::io::AsyncWriteExt;
        self.file
            .write_all(bytes)
            .await
            .map_err(|_| StorageError::Io)
    }

    /// Flushes, syncs, and publishes the file under its final name.
    ///
    /// Publication never replaces an existing destination. If an entry
    /// appeared after the manifest check, finalization fails instead.
    pub(crate) async fn finalize(self) -> Result<(), StorageError> {
        use tokio::io::AsyncWriteExt;

        // No custom `Drop` impl is needed: `NamedTempFile` removes the partial
        // unless the no-replace persist succeeds, and a failed persist hands
        // the partial back inside the error so dropping it also removes it.
        let Self {
            mut file,
            temp,
            final_path,
        } = self;
        file.flush().await.map_err(|_| StorageError::Io)?;
        file.sync_all().await.map_err(|_| StorageError::Io)?;
        drop(file);

        match temp.persist_noclobber(&final_path) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(StorageError::DestinationExists)
            }
            Err(_) if std::fs::symlink_metadata(&final_path).is_ok() => {
                Err(StorageError::DestinationExists)
            }
            Err(_) => Err(StorageError::Io),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        Destination, Platform, StorageError, TemporaryFile, name_conflict, validate_destination,
        validate_name,
    };
    use crate::protocol::FileEntry;

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lanweave-storage-{tag}-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn entry(name: &str) -> FileEntry {
        FileEntry::new(name.to_owned(), 1)
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

    #[test]
    fn unsafe_names_are_rejected_for_both_platforms() {
        let long = "n".repeat(crate::protocol::MAX_NAME_BYTES + 1);
        for name in ["", ".", "..", "a/b", "a\\b", "a\u{0}b", long.as_str()] {
            assert_eq!(
                validate_name(name, Platform::Unix),
                Err(StorageError::InvalidName),
                "name: {name:?}"
            );
            assert_eq!(
                validate_name(name, Platform::Windows),
                Err(StorageError::InvalidName),
                "name: {name:?}"
            );
        }

        for name in [
            "a:b",
            "trailing.",
            "trailing ",
            "CON",
            "con.txt",
            "LPT9",
            "<bad>",
        ] {
            assert_eq!(
                validate_name(name, Platform::Windows),
                Err(StorageError::InvalidName),
                "name: {name:?}"
            );
            assert_eq!(
                validate_name(name, Platform::Unix),
                Ok(()),
                "name: {name:?}"
            );
        }

        assert_eq!(validate_name("normal.txt", Platform::Windows), Ok(()));
    }

    #[test]
    fn windows_equivalence_detects_case_and_suffix_conflicts() {
        assert!(name_conflict("A.txt", "a.txt", Platform::Windows));
        assert!(name_conflict("a.txt.", "a.txt", Platform::Windows));
        assert!(name_conflict("a.txt", "a.txt ", Platform::Windows));
        assert!(!name_conflict("A.txt", "a.txt", Platform::Unix));
    }

    #[tokio::test]
    async fn manifest_checks_reject_conflicts_and_existing_entries() {
        let root = temp_root("check");
        let destination = Destination::new(root.clone());

        assert!(
            destination
                .check_manifest(&[entry("a.txt"), entry("b.txt")])
                .is_ok()
        );
        assert_eq!(
            destination.check_manifest(&[]),
            Err(StorageError::InvalidManifest)
        );
        assert_eq!(
            destination.check_manifest(&[entry("a.txt"), entry("a.txt")]),
            Err(StorageError::NameConflict)
        );

        std::fs::write(root.join("existing.txt"), b"x").unwrap();
        assert_eq!(
            destination.check_manifest(&[entry("existing.txt")]),
            Err(StorageError::DestinationExists)
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("existing.txt"), root.join("link.txt")).unwrap();
            assert_eq!(
                destination.check_manifest(&[entry("link.txt")]),
                Err(StorageError::DestinationExists)
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn finalization_publishes_without_replacing() {
        let root = temp_root("finalize");
        let destination = Destination::new(root.clone());

        let mut file = destination.begin_file(&entry("report.txt")).await.unwrap();
        file.write_all(b"payload").await.unwrap();
        file.finalize().await.unwrap();
        assert_eq!(std::fs::read(root.join("report.txt")).unwrap(), b"payload");

        // Preparing an existing name is refused before any bytes move.
        assert_eq!(
            destination
                .begin_file(&entry("report.txt"))
                .await
                .unwrap_err(),
            StorageError::DestinationExists
        );

        // A destination that races in after preparation is never replaced.
        let mut raced = TemporaryFile::create(&root, "raced.txt").unwrap();
        raced.write_all(b"partial").await.unwrap();
        std::fs::write(root.join("raced.txt"), b"original").unwrap();
        assert_eq!(
            raced.finalize().await.unwrap_err(),
            StorageError::DestinationExists
        );
        assert_eq!(std::fs::read(root.join("raced.txt")).unwrap(), b"original");
        assert_eq!(partial_count(&root), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn dropped_partials_are_removed() {
        let root = temp_root("cleanup");
        let destination = Destination::new(root.clone());

        let mut file = destination.begin_file(&entry("kept.txt")).await.unwrap();
        file.write_all(b"partial").await.unwrap();
        assert_eq!(partial_count(&root), 1);

        drop(file);
        assert_eq!(partial_count(&root), 0);
        assert!(!root.join("kept.txt").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn temporary_files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("mode");
        let destination = Destination::new(root.clone());
        let file = destination.begin_file(&entry("secret.txt")).await.unwrap();
        let metadata = std::fs::symlink_metadata(file.temp.path()).unwrap();

        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        let name = file.temp.path().file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(".lanweave-"), "name: {name}");
        assert!(name.ends_with(".part"), "name: {name}");
        drop(file);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn destination_roots_must_be_existing_directories() {
        let root = temp_root("destination");
        assert_eq!(validate_destination(&root), Ok(()));
        assert_eq!(
            validate_destination(&root.join("missing")),
            Err(StorageError::InvalidDestination)
        );

        let file = root.join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(
            validate_destination(&file),
            Err(StorageError::InvalidDestination)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
