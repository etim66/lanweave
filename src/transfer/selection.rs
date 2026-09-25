//! Pasted path parsing, local validation, and the reviewed outbound selection.
//!
//! Review is local: paths are parsed without a shell, checked against the
//! local filesystem, and reduced to a names-and-sizes manifest. Local paths
//! never enter a wire value or a log; they travel only inside the in-memory
//! selection carried by the send action.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};

use crate::discovery::escape_display;
use crate::protocol::{
    FileEntry, MAX_FILES, MAX_INTEGER, MAX_TRANSFER_REQUEST_BODY_BYTES, TransferRequest,
};

/// Maximum size in bytes of the pending selection text.
///
/// Matches the `transfer_request` body limit: the largest manifest the wire
/// can carry.
pub(crate) const MAX_SELECTION_INPUT_BYTES: usize = MAX_TRANSFER_REQUEST_BODY_BYTES;

/// One reviewed local file: the source path plus its manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedFile {
    path: PathBuf,
    name: String,
    size: u64,
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

    /// Returns the exact reviewed byte size.
    pub(crate) fn size(&self) -> u64 {
        self.size
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
    pub(crate) fn request(&self) -> Option<TransferRequest> {
        if self.files.is_empty() || self.files.len() > usize::from(MAX_FILES) {
            return None;
        }
        let files = self
            .files
            .iter()
            .map(|file| FileEntry {
                name: file.name.clone(),
                size: file.size,
            })
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
                })
                .collect(),
        }
    }
}

/// Splits pasted text into candidate paths without invoking a shell.
///
/// Newlines separate entries. Each entry is trimmed, `file://` URIs are
/// percent-decoded, and one matching pair of outer single or double quotes is
/// removed. Inside double quotes, `\\` and `\"` are unescaped; every other
/// backslash is kept so Windows paths and unquoted paths with spaces survive.
pub(crate) fn parse_paths(input: &str) -> Vec<String> {
    input
        .split('\n')
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line).trim();
            (!line.is_empty()).then(|| normalize_entry(line))
        })
        .filter(|entry| !entry.is_empty())
        .collect()
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

/// Captures the manifest entry of one safe, readable regular file.
fn inspect(path: &Path) -> Result<SelectedFile, &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "the file was not found")?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err("symbolic links are not supported");
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
    })
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
            (
                "file:///home/me/a%20b.txt\nfile://localhost/tmp/x\n",
                vec!["/home/me/a b.txt", "/tmp/x"],
            ),
            ("\"C:\\dir\\file.txt\"", vec!["C:\\dir\\file.txt"]),
            ("  \n\n ", vec![]),
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

        assert_eq!(selection.len(), 3);
        assert_eq!(selection.total_size(), 7);
        assert_eq!(selection.files()[0].path(), alpha);
        assert_eq!(selection.files()[2].name(), "café ünï.txt");
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].reason(), "only regular files can be sent");
        assert_eq!(issues[1].reason(), "the file was not found");

        let request = selection.request().unwrap();
        assert_eq!(request.files[0].name, "alpha.txt");
        assert_eq!(request.files[0].size, 5);
        assert_eq!(request.total_size, 7);
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
    fn hostile_input_is_escaped_before_display() {
        let mut selection = FileSelection::default();
        let issues = selection.add_text("\u{1b}[31mfake\u{0}a");

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

}
