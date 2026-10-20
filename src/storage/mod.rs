//! Safe filesystem boundary for received files.
//!
//! Handles destination name validation, restrictive no-follow temporary file
//! creation, and no-replace finalization. Lanweave never overwrites a
//! destination entry: an occupied name is preserved and the incoming file is
//! published under a numbered copy name such as `report (Copy).txt`.
//!
//! Creation and no-replace publication are delegated to the `tempfile` crate
//! with a `.lanweave-*.part` name; this module keeps destination policy,
//! streaming writes, copy naming, and `StorageError` mapping.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::protocol::{FileEntry, MAX_FILES, MAX_NAME_BYTES, is_valid_filename};

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
    /// No free copy name was available after bounded attempts.
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

/// Suffix added to the first duplicate of a filename.
const COPY_SUFFIX: &str = " (Copy)";
/// Prefix of a numbered duplicate suffix, completed with `N)`.
const COPY_NUMBER_PREFIX: &str = " (Copy ";
/// Highest existing copy number recognized when continuing a sequence.
const MAX_COPY_SEQUENCE: u64 = 1_000_000;
/// Number of copy names tried before finalization gives up.
const MAX_COPY_ATTEMPTS: u32 = 10_000;

/// Splits a filename before its final extension.
///
/// A leading dot belongs to the stem (`.env`), and a trailing dot with nothing
/// after it is not an extension (`report.`). This keeps copy tags before the
/// final extension, matching the familiar `report (Copy).txt` form.
fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 && index + 1 < name.len() => (&name[..index], &name[index..]),
        _ => (name, ""),
    }
}

/// Parses a `(Copy)` or `(Copy N)` suffix from a filename stem.
///
/// Only tags produced by this naming scheme are recognized. The returned count
/// is the existing copy number, so the next candidate is `count + 1`.
fn copy_sequence(stem: &str) -> Option<(&str, u64)> {
    if let Some(base) = stem.strip_suffix(COPY_SUFFIX) {
        return Some((base, 1));
    }
    let inner = stem.strip_suffix(')')?;
    let (base, number) = inner.rsplit_once(COPY_NUMBER_PREFIX)?;
    let count = number.parse::<u64>().ok()?;
    (1..=MAX_COPY_SEQUENCE)
        .contains(&count)
        .then_some((base, count))
}

/// Builds the `ordinal`th copy name for `requested`, starting at ` (Copy)`.
///
/// Existing copy tags are continued instead of nested, so a taken
/// `report (Copy).txt` becomes `report (Copy 2).txt`. A tag that follows the
/// final dot (`report. (Copy)`) is recognized on the full name first so it
/// continues rather than nesting. The result is truncated on a UTF-8 boundary
/// to stay within [`MAX_NAME_BYTES`].
fn copy_name(requested: &str, ordinal: u32) -> String {
    let (base, extension, current) = if let Some((base, current)) = copy_sequence(requested) {
        (base, "", current)
    } else {
        let (stem, extension) = split_extension(requested);
        match copy_sequence(stem) {
            Some((base, current)) => (base, extension, current),
            None => (stem, extension, 0),
        }
    };
    let count = current.saturating_add(u64::from(ordinal));
    let tag = if count == 1 {
        COPY_SUFFIX.to_owned()
    } else {
        format!("{COPY_NUMBER_PREFIX}{count})")
    };

    let reserved = tag.len() + extension.len();
    if reserved < MAX_NAME_BYTES {
        let base = truncate_utf8(base, MAX_NAME_BYTES - reserved);
        format!("{base}{tag}{extension}")
    } else {
        // The extension cannot fit alongside the copy tag. Keep the original
        // name intact as the stem and truncate it instead.
        let base = truncate_utf8(requested, MAX_NAME_BYTES - tag.len());
        format!("{base}{tag}")
    }
}

/// Truncates UTF-8 text to at most `max` bytes without splitting a character.
fn truncate_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
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
    ///
    /// Occupied destination names are allowed; finalization chooses a copy
    /// name. Only unsafe names and names that conflict with one another are
    /// rejected here.
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
        }
        Ok(())
    }

    /// Creates the restrictive partial file for one verified manifest entry.
    ///
    /// An occupied destination name is not refused here. Finalization keeps
    /// the existing entry and publishes the incoming file under a copy name.
    pub(crate) async fn begin_file(
        &self,
        entry: &FileEntry,
    ) -> Result<TemporaryFile, StorageError> {
        validate_name(&entry.name, Platform::current())?;
        TemporaryFile::create(&self.root, &entry.name)
    }
}

/// A restrictive, unpredictable partial file that is removed unless finalized.
#[derive(Debug)]
pub(crate) struct TemporaryFile {
    file: tokio::fs::File,
    temp: tempfile::NamedTempFile,
    root: PathBuf,
    requested_name: String,
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
            root: root.to_path_buf(),
            requested_name: name.to_owned(),
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

    /// Flushes, syncs, and publishes the file, returning its final name.
    ///
    /// Publication never replaces an existing destination. The requested name
    /// is tried first; each collision advances through ` (Copy)`,
    /// ` (Copy 2)`, and so on. A race after preparation therefore produces a
    /// copy instead of overwriting the new entry or failing the transfer.
    pub(crate) async fn finalize(self) -> Result<String, StorageError> {
        self.finalize_bounded(MAX_COPY_ATTEMPTS).await
    }

    /// Publishes with at most `max_attempts` copy names after the requested
    /// name. Split from [`Self::finalize`] so tests can exercise exhaustion
    /// without creating thousands of destination entries.
    async fn finalize_bounded(self, max_attempts: u32) -> Result<String, StorageError> {
        use tokio::io::AsyncWriteExt;

        // No custom `Drop` impl is needed: `NamedTempFile` removes the partial
        // unless the no-replace persist succeeds, and a failed persist hands
        // the partial back inside the error so dropping it also removes it.
        let Self {
            mut file,
            mut temp,
            root,
            requested_name,
        } = self;
        file.flush().await.map_err(|_| StorageError::Io)?;
        file.sync_all().await.map_err(|_| StorageError::Io)?;
        drop(file);

        for attempt in 0..=max_attempts {
            let name = if attempt == 0 {
                requested_name.clone()
            } else {
                copy_name(&requested_name, attempt)
            };
            let final_path = root.join(&name);
            match temp.persist_noclobber(&final_path) {
                Ok(_) => return Ok(name),
                Err(error) => {
                    let collision = error.error.kind() == std::io::ErrorKind::AlreadyExists
                        || std::fs::symlink_metadata(&final_path).is_ok();
                    temp = error.file;
                    if !collision {
                        return Err(StorageError::Io);
                    }
                }
            }
        }
        Err(StorageError::DestinationExists)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        COPY_SUFFIX, Destination, Platform, StorageError, TemporaryFile, copy_name, name_conflict,
        validate_destination, validate_name,
    };
    use crate::protocol::{FileEntry, MAX_NAME_BYTES};

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

    #[test]
    fn copy_names_follow_the_linux_convention() {
        assert_eq!(copy_name("NYSC.pdf", 1), "NYSC (Copy).pdf");
        assert_eq!(copy_name("NYSC.pdf", 2), "NYSC (Copy 2).pdf");
        assert_eq!(copy_name("NYSC (Copy).pdf", 1), "NYSC (Copy 2).pdf");
        assert_eq!(copy_name("NYSC (Copy 2).pdf", 1), "NYSC (Copy 3).pdf");
        assert_eq!(copy_name("report. (Copy)", 1), "report. (Copy 2)");
        assert_eq!(copy_name("report.txt (Copy)", 1), "report.txt (Copy 2)");
        assert_eq!(copy_name("archive.tar.gz", 1), "archive.tar (Copy).gz");
        assert_eq!(copy_name(".env", 1), ".env (Copy)");
        assert_eq!(copy_name("README", 1), "README (Copy)");
        assert_eq!(copy_name("foo.", 1), "foo. (Copy)");
        assert_eq!(copy_name("foo...txt", 1), "foo.. (Copy).txt");
        assert_eq!(
            copy_name("NYSC (Copyright).pdf", 1),
            "NYSC (Copyright) (Copy).pdf"
        );
    }

    #[test]
    fn copy_names_fit_the_protocol_limit() {
        let long = format!("{}.pdf", "n".repeat(MAX_NAME_BYTES - 4));
        assert_eq!(long.len(), MAX_NAME_BYTES);
        let copy = copy_name(&long, 1);
        assert!(copy.len() <= MAX_NAME_BYTES);
        assert!(copy.ends_with(" (Copy).pdf"));
        assert_eq!(validate_name(&copy, Platform::Unix), Ok(()));
        assert_eq!(validate_name(&copy, Platform::Windows), Ok(()));

        let unicode = format!("{}.pdf", "é".repeat(124));
        let copy = copy_name(&unicode, 1);
        assert!(copy.len() <= MAX_NAME_BYTES);
        assert!(copy.ends_with(" (Copy).pdf"));
        assert_eq!(validate_name(&copy, Platform::Unix), Ok(()));

        // A huge extension cannot fit beside the tag; the name is still bounded.
        let huge_extension = format!("a.{}", "x".repeat(MAX_NAME_BYTES - 5));
        let copy = copy_name(&huge_extension, 1);
        assert!(copy.len() <= MAX_NAME_BYTES);
        assert!(copy.ends_with(COPY_SUFFIX));
        assert_eq!(validate_name(&copy, Platform::Unix), Ok(()));
    }

    #[tokio::test]
    async fn manifest_checks_accept_existing_names_but_reject_conflicts() {
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
        assert!(destination.check_manifest(&[entry("existing.txt")]).is_ok());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("existing.txt"), root.join("link.txt")).unwrap();
            assert!(destination.check_manifest(&[entry("link.txt")]).is_ok());
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn finalization_uses_copy_names_without_replacing() {
        let root = temp_root("finalize");
        let destination = Destination::new(root.clone());

        let mut file = destination.begin_file(&entry("report.txt")).await.unwrap();
        file.write_all(b"first").await.unwrap();
        assert_eq!(file.finalize().await.unwrap(), "report.txt");

        let mut file = destination.begin_file(&entry("report.txt")).await.unwrap();
        file.write_all(b"second").await.unwrap();
        assert_eq!(file.finalize().await.unwrap(), "report (Copy).txt");

        let mut file = destination.begin_file(&entry("report.txt")).await.unwrap();
        file.write_all(b"third").await.unwrap();
        assert_eq!(file.finalize().await.unwrap(), "report (Copy 2).txt");

        assert_eq!(std::fs::read(root.join("report.txt")).unwrap(), b"first");
        assert_eq!(
            std::fs::read(root.join("report (Copy).txt")).unwrap(),
            b"second"
        );
        assert_eq!(
            std::fs::read(root.join("report (Copy 2).txt")).unwrap(),
            b"third"
        );

        // A gap is reused before a higher number.
        std::fs::remove_file(root.join("report (Copy).txt")).unwrap();
        let mut file = destination.begin_file(&entry("report.txt")).await.unwrap();
        file.write_all(b"fourth").await.unwrap();
        assert_eq!(file.finalize().await.unwrap(), "report (Copy).txt");
        assert_eq!(
            std::fs::read(root.join("report (Copy).txt")).unwrap(),
            b"fourth"
        );

        assert_eq!(partial_count(&root), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn finalization_reports_exhaustion_after_the_bounded_search() {
        let root = temp_root("exhausted");
        std::fs::write(root.join("report.txt"), b"original").unwrap();
        std::fs::write(root.join("report (Copy).txt"), b"first copy").unwrap();

        let mut file = TemporaryFile::create(&root, "report.txt").unwrap();
        file.write_all(b"partial").await.unwrap();
        assert_eq!(
            file.finalize_bounded(1).await,
            Err(StorageError::DestinationExists)
        );
        assert_eq!(partial_count(&root), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_destination_created_after_preparation_receives_a_copy() {
        let root = temp_root("race");
        let mut raced = TemporaryFile::create(&root, "raced.txt").unwrap();
        raced.write_all(b"partial").await.unwrap();
        std::fs::write(root.join("raced.txt"), b"original").unwrap();

        assert_eq!(raced.finalize().await.unwrap(), "raced (Copy).txt");
        assert_eq!(std::fs::read(root.join("raced.txt")).unwrap(), b"original");
        assert_eq!(
            std::fs::read(root.join("raced (Copy).txt")).unwrap(),
            b"partial"
        );
        assert_eq!(partial_count(&root), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn existing_symlinks_are_preserved_and_skipped() {
        let root = temp_root("symlink");
        let destination = Destination::new(root.clone());
        std::fs::write(root.join("target.txt"), b"target").unwrap();
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("link.txt")).unwrap();

        let mut file = destination.begin_file(&entry("link.txt")).await.unwrap();
        file.write_all(b"incoming").await.unwrap();
        assert_eq!(file.finalize().await.unwrap(), "link (Copy).txt");
        assert!(
            std::fs::symlink_metadata(root.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(root.join("link (Copy).txt")).unwrap(),
            b"incoming"
        );
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
