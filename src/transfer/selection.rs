//! Pasted path parsing, local validation, and the reviewed outbound selection.
//!
//! Review is local: paths are parsed without a shell, checked against the
//! local filesystem, and reduced to a names-and-sizes manifest. Local paths
//! never enter a wire value or a log; they travel only inside the in-memory
//! selection carried by the send action.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::discovery::escape_display;
use crate::protocol::{
    FileEntry, MAX_FILES, MAX_INTEGER, MAX_TRANSFER_REQUEST_BODY_BYTES, TransferRequest,
};

use super::archive::{self, ArchiveError};

/// Maximum size in bytes of the pending selection text.
///
/// Matches the `transfer_request` body limit: the largest manifest the wire
/// can carry.
pub(crate) const MAX_SELECTION_INPUT_BYTES: usize = MAX_TRANSFER_REQUEST_BODY_BYTES;

/// One reviewed local file or folder: the source path plus its manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedFile {
    path: PathBuf,
    name: String,
    size: u64,
    archive: Option<FolderArchive>,
}

/// Folder metadata captured locally before the archive is built.
///
/// `source_size` is the uncompressed size shown during review; the wire size
/// is the built zip's exact byte size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FolderArchive {
    pub(crate) items: u64,
    pub(crate) source_size: u64,
}

impl SelectedFile {
    /// Returns the local source path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the single-component filename sent to the peer.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns the reviewed byte size.
    ///
    /// For a folder this is the uncompressed source size, which is what the
    /// review shows; the zip size is measured after preparation.
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Returns folder metadata when this entry is a folder archive.
    pub(crate) fn archive(&self) -> Option<FolderArchive> {
        self.archive
    }
}

/// One rejected path with the reason it cannot be sent.
///
/// The path is escaped at construction, so it is safe to render and never
/// contains terminal controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectionIssue {
    path: String,
    reason: &'static str,
}

impl SelectionIssue {
    fn new(path: &str, reason: &'static str) -> Self {
        Self {
            path: escape_display(path),
            reason,
        }
    }

    /// Builds an issue for rendering tests.
    #[cfg(test)]
    pub(crate) fn for_test(path: &str, reason: &'static str) -> Self {
        Self::new(path, reason)
    }

    /// Returns the display-safe rejected path.
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    /// Returns the static rejection reason.
    pub(crate) fn reason(&self) -> &'static str {
        self.reason
    }
}

/// An ordered, locally reviewed set of files ready for a manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileSelection {
    files: Vec<SelectedFile>,
}

impl FileSelection {
    /// Returns the reviewed files in selection order.
    pub(crate) fn files(&self) -> &[SelectedFile] {
        &self.files
    }

    /// Returns the number of reviewed files.
    pub(crate) fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns whether no file has been reviewed yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns whether any reviewed entry still needs an archive built.
    pub(crate) fn has_archives(&self) -> bool {
        self.files.iter().any(|file| file.archive.is_some())
    }

    /// Returns the total reviewed size.
    ///
    /// Adding enforces the protocol integer bound, so the sum cannot overflow.
    pub(crate) fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// Removes one entry by index, returning whether it existed.
    pub(crate) fn remove(&mut self, index: usize) -> bool {
        if index < self.files.len() {
            self.files.remove(index);
            true
        } else {
            false
        }
    }

    /// Parses pasted or typed text and appends every valid new file.
    ///
    /// Returns one issue per rejected path. Order is preserved, duplicates
    /// are skipped, and the count and total stay inside the wire limits.
    pub(crate) fn add_text(&mut self, input: &str) -> Vec<SelectionIssue> {
        let mut issues = Vec::new();
        for path in parse_paths(input) {
            match self.add_path(Path::new(&path)) {
                Ok(()) => {}
                Err(reason) => {
                    // Terminals escape spaces and punctuation in drag-and-drop
                    // paths. Retry the unescaped form only when the literal
                    // path does not exist, so real backslashes survive.
                    if reason == "the file was not found" {
                        let unescaped = unescape_shell(&path);
                        if unescaped != path && self.add_path(Path::new(&unescaped)).is_ok() {
                            continue;
                        }
                    }
                    issues.push(SelectionIssue::new(&path, reason));
                }
            }
        }
        issues
    }

    /// Builds the wire manifest: ordered names and exact sizes only.
    ///
    /// Returns `None` while a reviewed folder still needs its archive built;
    /// the session prepares the selection before it builds the manifest.
    pub(crate) fn request(&self) -> Option<TransferRequest> {
        if self.files.is_empty() || self.files.len() > usize::from(MAX_FILES) || self.has_archives()
        {
            return None;
        }
        let files = self
            .files
            .iter()
            .map(|file| FileEntry::new(file.name.clone(), file.size))
            .collect();
        TransferRequest::new(files)
    }

    /// Checks one path, enforces the selection rules, and appends it.
    fn add_path(&mut self, path: &Path) -> Result<(), &'static str> {
        let file = inspect(path)?;
        if self.files.len() >= usize::from(MAX_FILES) {
            return Err("too many files; at most 1024 can be sent at once");
        }
        if self.files.iter().any(|existing| existing.path == file.path) {
            return Err("already selected");
        }
        if self.files.iter().any(|existing| existing.name == file.name) {
            return Err("another selected file already uses this name");
        }
        let total = self
            .total_size()
            .checked_add(file.size)
            .filter(|total| *total <= MAX_INTEGER);
        if total.is_none() {
            return Err("the selected total is too large");
        }
        self.files.push(file);
        Ok(())
    }

    /// Builds every folder archive and returns streamable sources.
    ///
    /// `on_item` reports `(entry_index, archive_name, done, total)` while one
    /// folder is compressed. Temporary archives are deleted when the returned
    /// selection is dropped. Cancellation is honored between entries.
    pub(crate) fn prepare(
        &self,
        cancel: &AtomicBool,
        on_item: &mut dyn FnMut(usize, &str, u64, u64),
    ) -> Result<PreparedSelection, PrepareError> {
        let mut files = Vec::with_capacity(self.files.len());
        for (index, file) in self.files.iter().enumerate() {
            let Some(archive) = file.archive else {
                files.push(PreparedFile {
                    path: file.path.clone(),
                    name: file.name.clone(),
                    size: file.size,
                    folder: None,
                    cleanup: None,
                });
                continue;
            };

            let temp = tempfile::Builder::new()
                .prefix(".lanweave-")
                .suffix(".zip")
                .tempfile()
                .map_err(|_| PrepareError::Failed)?;
            archive::build_zip(&file.path, temp.path(), cancel, &mut |done, total| {
                on_item(index, &file.name, done, total);
            })
            .map_err(|error| match error {
                ArchiveError::Cancelled => PrepareError::Cancelled,
                ArchiveError::TooLarge => PrepareError::TooLarge,
                ArchiveError::Io => PrepareError::Failed,
            })?;
            let size = temp
                .as_file()
                .metadata()
                .map_err(|_| PrepareError::Failed)?
                .len();
            files.push(PreparedFile {
                path: temp.path().to_path_buf(),
                name: file.name.clone(),
                size,
                folder: Some(archive),
                cleanup: Some(temp.into_temp_path()),
            });
        }
        Ok(PreparedSelection { files })
    }

    /// Builds a selection from names for tests that do not touch the filesystem.
    #[cfg(test)]
    pub(crate) fn for_test(files: &[(&str, u64)]) -> Self {
        Self {
            files: files
                .iter()
                .map(|(name, size)| SelectedFile {
                    path: PathBuf::from(name),
                    name: (*name).to_owned(),
                    size: *size,
                    archive: None,
                })
                .collect(),
        }
    }
}

/// Why a reviewed selection could not be prepared for sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrepareError {
    /// The caller cancelled while an archive was being built.
    Cancelled,
    /// An archive could not be written.
    Failed,
    /// The folder exceeds the archive entry bound.
    TooLarge,
}

impl PrepareError {
    /// Returns the user-facing reason shown when preparation fails.
    pub(crate) const fn notice(self) -> &'static str {
        match self {
            Self::TooLarge => {
                "A folder holds more than 100,000 items, so it cannot be sent as one archive."
            }
            Self::Failed => {
                "A folder could not be compressed; files may have changed while it was reviewed."
            }
            Self::Cancelled => "Folder preparation was cancelled.",
        }
    }
}

/// A selection whose folder archives have been built.
///
/// Dropping it deletes any temporary archives it owns.
#[derive(Debug)]
pub(crate) struct PreparedSelection {
    files: Vec<PreparedFile>,
}

impl PreparedSelection {
    /// Returns the streamable sources in manifest order.
    pub(crate) fn files(&self) -> &[PreparedFile] {
        &self.files
    }

    /// Builds the wire manifest, including folder metadata.
    pub(crate) fn request(&self) -> Option<TransferRequest> {
        if self.files.is_empty() || self.files.len() > usize::from(MAX_FILES) {
            return None;
        }
        let files = self
            .files
            .iter()
            .map(|file| match file.folder {
                Some(folder) => FileEntry::folder(
                    file.name.clone(),
                    file.size,
                    folder.items,
                    folder.source_size,
                ),
                None => FileEntry::new(file.name.clone(), file.size),
            })
            .collect();
        TransferRequest::new(files)
    }
}

/// One source ready to stream, with its temporary archive when it is a folder.
#[derive(Debug)]
pub(crate) struct PreparedFile {
    path: PathBuf,
    name: String,
    size: u64,
    folder: Option<FolderArchive>,
    cleanup: Option<tempfile::TempPath>,
}

impl PreparedFile {
    /// Returns the path to stream.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the single-component filename sent to the peer.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns the exact byte size that will be streamed.
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// Returns folder metadata when this source is a zip archive.
    pub(crate) fn folder(&self) -> Option<FolderArchive> {
        self.folder
    }

    /// Returns whether this source owns a temporary archive to delete.
    pub(crate) fn owns_archive(&self) -> bool {
        self.cleanup.is_some()
    }
}

/// Splits pasted text into candidate paths without invoking a shell.
///
/// Any run of line separators (`\r`, `\n`, `\r\n` or `\0`) separates entries,
/// because file managers send URI lists with CRLF and some terminals paste
/// them with a lone carriage return. Each entry is trimmed, `file://` URIs are
/// percent-decoded, and one matching pair of outer single or double quotes is
/// removed. Inside double quotes, `\\` and `\"` are unescaped; every other
/// backslash is kept so Windows paths and unquoted paths with spaces survive.
pub(crate) fn parse_paths(input: &str) -> Vec<String> {
    input
        .split(['\r', '\n', '\0'])
        .flat_map(split_chunk)
        .collect()
}

/// Splits one separator-free chunk into one or more candidate paths.
///
/// A chunk can still hold several `file://` URIs that a file manager joined
/// with spaces, so those are split again; ordinary paths keep their spaces.
fn split_chunk(chunk: &str) -> Vec<String> {
    let chunk = chunk.trim();
    if chunk.is_empty() {
        return Vec::new();
    }
    if chunk.matches("file://").count() > 1 {
        return chunk
            .split_whitespace()
            .map(normalize_entry)
            .filter(|entry| !entry.is_empty())
            .collect();
    }
    let entry = normalize_entry(chunk);
    if entry.is_empty() {
        Vec::new()
    } else {
        vec![entry]
    }
}

/// Converts one pasted entry into a candidate path.
fn normalize_entry(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        return percent_decode(path);
    }
    unquote(line)
}

/// Decodes `%XX` escapes in a URI path, leaving invalid escapes intact.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Returns the value of one hexadecimal digit.
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Removes shell backslash escapes from a path that does not exist as typed.
///
/// Only characters terminals commonly escape are unescaped, so Windows
/// separators and unknown sequences are preserved.
fn unescape_shell(path: &str) -> String {
    const ESCAPABLE: [char; 13] = [
        ' ', '(', ')', '[', ']', '{', '}', '&', '\'', '"', '$', '#', ';',
    ];

    let mut output = String::with_capacity(path.len());
    let mut characters = path.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some(next) if ESCAPABLE.contains(&next) => output.push(next),
                Some(other) => {
                    output.push('\\');
                    output.push(other);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// Removes one matching outer quote pair, if present.
fn unquote(line: &str) -> String {
    let bytes = line.as_bytes();
    let first = bytes.first().copied();
    let last = bytes.last().copied();
    if line.len() >= 2 && first == Some(b'\'') && last == Some(b'\'') {
        return line[1..line.len() - 1].to_owned();
    }
    if line.len() >= 2 && first == Some(b'"') && last == Some(b'"') {
        return unescape_quoted(&line[1..line.len() - 1]);
    }
    line.to_owned()
}

/// Unescapes `\\` and `\"` only, leaving platform separators intact.
fn unescape_quoted(inner: &str) -> String {
    let mut output = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            match characters.next() {
                Some('\\') => output.push('\\'),
                Some('"') => output.push('"'),
                Some(other) => {
                    output.push('\\');
                    output.push(other);
                }
                None => output.push('\\'),
            }
        } else {
            output.push(character);
        }
    }
    output
}

/// Captures the manifest entry of one safe, readable regular file or folder.
fn inspect(path: &Path) -> Result<SelectedFile, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "the file was not found")?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err("symbolic links are not supported");
    }
    if file_type.is_dir() {
        return inspect_folder(path);
    }
    if !file_type.is_file() {
        return Err("only regular files can be sent");
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the file name is not valid UTF-8")?;
    if !crate::protocol::is_valid_filename(name) {
        return Err("the file name cannot be sent");
    }
    std::fs::File::open(path).map_err(|_| "the file is not readable")?;
    Ok(SelectedFile {
        path: path.to_owned(),
        name: name.to_owned(),
        size: metadata.len(),
        archive: None,
    })
}

/// Captures a folder as a single zip archive entry.
fn inspect_folder(path: &Path) -> Result<SelectedFile, &'static str> {
    let folder = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the folder name is not valid UTF-8")?;
    let name = format!("{folder}.zip");
    if !crate::protocol::is_valid_filename(&name) {
        return Err("the folder name cannot be sent");
    }
    let (items, source_size) = walk_folder(path)?;
    Ok(SelectedFile {
        path: path.to_owned(),
        name,
        size: source_size,
        archive: Some(FolderArchive { items, source_size }),
    })
}

/// Counts the files and directories inside `root` and sums regular file sizes.
///
/// Symlinks and special files are skipped and never followed. The walk is
/// bounded so a hostile directory tree cannot exhaust the review step.
fn walk_folder(root: &Path) -> Result<(u64, u64), &'static str> {
    let mut items = 0u64;
    let mut size = 0u64;
    let mut stack = vec![root.to_owned()];

    while let Some(directory) = stack.pop() {
        let children = std::fs::read_dir(&directory).map_err(|_| "the folder is not readable")?;
        for child in children {
            let child = child.map_err(|_| "the folder is not readable")?;
            let metadata = std::fs::symlink_metadata(child.path())
                .map_err(|_| "the folder changed while it was reviewed")?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                continue;
            }
            items += 1;
            if items > archive::MAX_FOLDER_ITEMS {
                return Err("the folder has too many items");
            }
            if file_type.is_dir() {
                stack.push(child.path());
            } else if file_type.is_file() {
                size = size
                    .checked_add(metadata.len())
                    .filter(|total| *total <= MAX_INTEGER)
                    .ok_or("the folder is too large")?;
            }
        }
    }

    Ok((items, size))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{FileSelection, parse_paths};
    use crate::protocol::Control;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lanweave-selection-{tag}-{:016x}",
            fastrand::u64(..)
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_file(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn paths_split_on_newlines_and_strip_one_quote_layer() {
        let cases = [
            ("a.txt\nb c.txt\n", vec!["a.txt", "b c.txt"]),
            ("'a b.txt'\r\n\"c d.txt\"\n", vec!["a b.txt", "c d.txt"]),
            ("\"e \\\"q\\\".txt\"", vec!["e \"q\".txt"]),
            ("\"C:\\\\dir\\\\file.txt\"", vec!["C:\\dir\\file.txt"]),
            ("\"C:\\dir\\file.txt\"", vec!["C:\\dir\\file.txt"]),
            (
                "file:///home/me/a%20b.txt\nfile://localhost/tmp/x\n",
                vec!["/home/me/a b.txt", "/tmp/x"],
            ),
            // File managers and terminals paste a lone carriage return
            // between selected paths.
            ("a.txt\rb.txt\r", vec!["a.txt", "b.txt"]),
            ("a.txt\r\nb.txt\nc.txt", vec!["a.txt", "b.txt", "c.txt"]),
            ("a.txt\0b.txt", vec!["a.txt", "b.txt"]),
            (
                "file:///home/me/one%20file.txt file:///home/me/two.txt",
                vec!["/home/me/one file.txt", "/home/me/two.txt"],
            ),
            ("  \n\n ", vec![]),
            ("\r\n", vec![]),
            ("'unbalanced", vec!["'unbalanced"]),
        ];

        for (input, expected) in cases {
            assert_eq!(parse_paths(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn review_accepts_regular_files_and_reports_each_rejection() {
        let root = temp_root("review");
        let alpha = write_file(&root, "alpha.txt", b"12345");
        let empty = write_file(&root, "two words.txt", b"");
        let unicode = write_file(&root, "café ünï.txt", "é".as_bytes());
        let directory = root.join("nested");
        std::fs::create_dir_all(&directory).unwrap();
        let missing = root.join("missing.txt");

        let mut selection = FileSelection::default();
        let input = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            alpha.display(),
            empty.display(),
            unicode.display(),
            directory.display(),
            missing.display()
        );
        let issues = selection.add_text(&input);

        // Regular files and the folder are accepted; only the missing path fails.
        assert_eq!(selection.len(), 4);
        assert_eq!(selection.total_size(), 7);
        assert_eq!(selection.files()[0].path(), alpha);
        assert_eq!(selection.files()[2].name(), "café ünï.txt");
        assert_eq!(selection.files()[3].name(), "nested.zip");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].reason(), "the file was not found");

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let prepared = selection.prepare(&cancel, &mut |_, _, _, _| {}).unwrap();
        let request = prepared.request().unwrap();
        assert_eq!(request.files[0].name, "alpha.txt");
        assert_eq!(request.files[0].size, 5);
        assert_eq!(request.files[3].name, "nested.zip");
        assert!(request.files[3].folder.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_multi_selection_paste_reviews_files_and_folders_together() {
        let root = temp_root("multi-paste");
        let first = write_file(&root, "one.txt", b"123");
        let folder = root.join("docs");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("inner.txt"), b"45").unwrap();
        let second = write_file(&root, "two words.txt", b"6");

        // Exactly the shape a desktop clipboard produces: URI entries joined
        // by a lone carriage return, with spaces percent-encoded.
        let uri =
            |path: &Path| format!("file://{}", path.display().to_string().replace(' ', "%20"));
        let input = format!("{}\r{}\r{}\r", uri(&first), uri(&folder), uri(&second));

        let mut selection = FileSelection::default();
        let issues = selection.add_text(&input);

        assert!(issues.is_empty(), "unexpected issues: {issues:?}");
        assert_eq!(selection.len(), 3);
        assert_eq!(selection.files()[0].name(), "one.txt");
        assert_eq!(selection.files()[1].name(), "docs.zip");
        assert!(selection.files()[1].archive().is_some());
        assert_eq!(selection.files()[2].name(), "two words.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folders_are_reviewed_and_prepared_as_zip_archives() {
        let root = temp_root("folders");
        let folder = root.join("docs");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::write(folder.join("one.txt"), b"12345").unwrap();
        std::fs::write(folder.join("nested/two.txt"), b"678").unwrap();

        let mut selection = FileSelection::default();
        assert!(selection.add_text(&folder.display().to_string()).is_empty());
        assert_eq!(selection.len(), 1);

        let reviewed = &selection.files()[0];
        assert_eq!(reviewed.name(), "docs.zip");
        assert_eq!(reviewed.size(), 8);
        let archive = reviewed.archive().unwrap();
        assert_eq!(archive.items, 3);
        assert_eq!(archive.source_size, 8);

        // Folders have no ready manifest until their archive is built.
        assert!(selection.request().is_none());
        assert!(selection.has_archives());

        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut observed = Vec::new();
        let prepared = selection
            .prepare(&cancel, &mut |index, name, done, total| {
                observed.push((index, name.to_owned(), done, total));
            })
            .unwrap();
        assert!(
            observed
                .iter()
                .any(|(_, name, _, total)| name == "docs.zip" && *total == 3)
        );

        let file = &prepared.files()[0];
        assert_eq!(file.name(), "docs.zip");
        assert_eq!(file.folder().unwrap().items, 3);
        assert!(file.owns_archive());
        assert!(file.path().exists());
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), file.size());

        // The wire manifest carries the zip size plus folder metadata.
        let request = prepared.request().unwrap();
        assert_eq!(request.files[0].name, "docs.zip");
        assert_eq!(request.files[0].size, file.size());
        let folder_meta = request.files[0].folder.unwrap();
        assert_eq!(folder_meta.items, 3);
        assert_eq!(folder_meta.source_size, 8);

        // Dropping the prepared selection removes its temporary archive.
        let archive_path = file.path().to_path_buf();
        drop(prepared);
        assert!(!archive_path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_rejected() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let target = write_file(&root, "target.txt", b"data");
        let link = root.join("link.txt");
        symlink(&target, &link).unwrap();

        let mut selection = FileSelection::default();
        let issues = selection.add_text(&link.display().to_string());

        assert!(selection.is_empty());
        assert_eq!(issues[0].reason(), "symbolic links are not supported");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_paths_and_names_are_rejected() {
        let root = temp_root("dupes");
        let first = root.join("one");
        let second = root.join("two");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        write_file(&first, "same.txt", b"a");
        write_file(&second, "same.txt", b"b");
        let unique = write_file(&root, "unique.txt", b"c");

        let mut selection = FileSelection::default();
        let input = format!(
            "{}\n{}\n{}\n{}\n",
            first.join("same.txt").display(),
            second.join("same.txt").display(),
            unique.display(),
            unique.display()
        );
        let issues = selection.add_text(&input);

        assert_eq!(selection.len(), 2);
        assert_eq!(issues.len(), 2);
        assert_eq!(
            issues[0].reason(),
            "another selected file already uses this name"
        );
        assert_eq!(issues[1].reason(), "already selected");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn escaped_and_uri_paths_are_reviewed_when_the_literal_form_is_missing() {
        let root = temp_root("escapes");
        let spaced = write_file(&root, "my report.txt", b"data");

        let mut escaped = FileSelection::default();
        let input = spaced.display().to_string().replace(' ', "\\ ");
        assert!(escaped.add_text(&input).is_empty());
        assert_eq!(escaped.files()[0].path(), spaced);
        assert_eq!(escaped.files()[0].name(), "my report.txt");

        let mut uri = FileSelection::default();
        let input = format!(
            "file://{}",
            spaced.display().to_string().replace(' ', "%20")
        );
        assert!(uri.add_text(&input).is_empty());
        assert_eq!(uri.files()[0].path(), spaced);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hostile_input_is_escaped_before_display() {
        let mut selection = FileSelection::default();
        let issues = selection.add_text("\u{1b}[31mfake\u{7}a");

        assert_eq!(issues.len(), 1);
        assert!(!issues[0].path().chars().any(char::is_control));
        assert!(issues[0].path().contains("\\u{001B}"));
    }

    #[test]
    fn the_manifest_never_contains_local_paths() {
        let root = temp_root("manifest");
        let path = write_file(&root, "report.txt", b"hello");
        let mut selection = FileSelection::default();
        selection.add_text(&path.display().to_string());
        assert_eq!(selection.len(), 1);

        let request = selection.request().unwrap();
        assert_eq!(request.files[0].name, "report.txt");
        assert_eq!(request.files[0].size, 5);

        let encoded = String::from_utf8(Control::TransferRequest(request).encode()).unwrap();
        assert!(!encoded.contains(&root.display().to_string()));
        assert!(encoded.contains("report.txt"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_selections_have_no_manifest() {
        assert!(FileSelection::default().request().is_none());
    }
}
