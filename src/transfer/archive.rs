//! Bounded folder-to-zip preparation for folder transfers.
//!
//! The archive is streamed entry by entry to a temporary file, skipping
//! symlinks and special files, so a folder is never held in memory. The
//! caller owns the temporary file and deletes it when the transfer ends.

use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Maximum number of entries one folder archive may hold.
pub(crate) const MAX_FOLDER_ITEMS: u64 = 100_000;

/// Why a folder archive could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveError {
    /// The caller cancelled between entries.
    Cancelled,
    /// A source entry could not be read or written.
    Io,
    /// The folder exceeds the entry bound.
    TooLarge,
}

/// One collected folder entry relative to the archive root.
struct Collected {
    absolute: PathBuf,
    relative: String,
    directory: bool,
}

/// Zips `root` into `destination`, reporting progress after every entry.
///
/// `on_item` receives the number of entries written and the total. The
/// cancellation flag is checked before every entry so a long archive can stop
/// cleanly between items.
pub(crate) fn build_zip(
    root: &Path,
    destination: &Path,
    cancel: &AtomicBool,
    on_item: &mut dyn FnMut(u64, u64),
) -> Result<(), ArchiveError> {
    let entries = collect(root)?;
    let total = entries.len() as u64;
    let file = File::create(destination).map_err(|_| ArchiveError::Io)?;
    let mut writer = ZipWriter::new(file);
    let file_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644)
        .large_file(true);
    let directory_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o755);

    for (done, entry) in entries.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ArchiveError::Cancelled);
        }
        if entry.directory {
            writer
                .add_directory(&entry.relative, directory_options)
                .map_err(|_| ArchiveError::Io)?;
        } else {
            writer
                .start_file(&entry.relative, file_options)
                .map_err(|_| ArchiveError::Io)?;
            let mut source = File::open(&entry.absolute).map_err(|_| ArchiveError::Io)?;
            io::copy(&mut source, &mut writer).map_err(|_| ArchiveError::Io)?;
        }
        on_item(done as u64 + 1, total);
    }
    writer.finish().map_err(|_| ArchiveError::Io)?;
    Ok(())
}

/// Collects sanitized, sorted entries below `root`.
///
/// Symlinks and special files are skipped and never followed.
fn collect(root: &Path) -> Result<Vec<Collected>, ArchiveError> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(directory) = stack.pop() {
        let children = std::fs::read_dir(&directory).map_err(|_| ArchiveError::Io)?;
        for child in children {
            let child = child.map_err(|_| ArchiveError::Io)?;
            let metadata = std::fs::symlink_metadata(child.path()).map_err(|_| ArchiveError::Io)?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                continue;
            }
            let relative = relative_name(root, &child.path())?;
            if file_type.is_dir() {
                entries.push(Collected {
                    absolute: child.path(),
                    relative,
                    directory: true,
                });
                stack.push(child.path());
            } else if file_type.is_file() {
                entries.push(Collected {
                    absolute: child.path(),
                    relative,
                    directory: false,
                });
            }
            if entries.len() as u64 > MAX_FOLDER_ITEMS {
                return Err(ArchiveError::TooLarge);
            }
        }
    }

    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(entries)
}

/// Builds a `/`-separated UTF-8 archive name for one path.
fn relative_name(root: &Path, path: &Path) -> Result<String, ArchiveError> {
    let relative = path.strip_prefix(root).map_err(|_| ArchiveError::Io)?;
    let mut name = String::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(ArchiveError::Io);
        };
        let part = part.to_str().ok_or(ArchiveError::Io)?;
        if part.is_empty() || part.contains('/') || part.contains('\\') || part == ".." {
            return Err(ArchiveError::Io);
        }
        if !name.is_empty() {
            name.push('/');
        }
        name.push_str(part);
    }
    if name.is_empty() {
        return Err(ArchiveError::Io);
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    use super::{ArchiveError, MAX_FOLDER_ITEMS, build_zip, collect};

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lanweave-archive-{tag}-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn archives_preserve_structure_and_skip_symlinks() {
        let root = temp_root("zip");
        write(&root, "one.txt", b"one");
        write(&root, "nested/two.txt", b"two");
        std::fs::create_dir_all(root.join("empty")).unwrap();
        let destination = root.with_extension("zip");
        let cancel = AtomicBool::new(false);

        let mut observed = Vec::new();
        build_zip(&root, &destination, &cancel, &mut |done, total| {
            observed.push((done, total));
        })
        .unwrap();

        let file = std::fs::File::open(&destination).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect();

        assert!(names.contains(&"one.txt".to_owned()));
        assert!(names.contains(&"nested/".to_owned()));
        assert!(names.contains(&"nested/two.txt".to_owned()));
        assert!(names.contains(&"empty/".to_owned()));
        assert_eq!(observed.first(), Some(&(1, 4)));
        assert_eq!(observed.last(), Some(&(4, 4)));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(root.join("one.txt"), root.join("link.txt")).unwrap();
            let second = root.with_extension("zip2");
            build_zip(&root, &second, &cancel, &mut |_, _| {}).unwrap();
            let file = std::fs::File::open(&second).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            let names: Vec<String> = (0..archive.len())
                .map(|index| archive.by_index(index).unwrap().name().to_owned())
                .collect();
            assert!(!names.iter().any(|name| name.starts_with("link")));
        }

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&destination);
    }

    #[test]
    fn cancellation_stops_between_entries() {
        let root = temp_root("cancel");
        write(&root, "a.txt", b"a");
        write(&root, "b.txt", b"b");
        let destination = root.with_extension("zip");
        let cancel = AtomicBool::new(true);

        assert_eq!(
            build_zip(&root, &destination, &cancel, &mut |_, _| {}),
            Err(ArchiveError::Cancelled)
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&destination);
    }

    #[test]
    fn entries_are_collected_in_a_bounded_sorted_list() {
        let root = temp_root("bound");
        for index in [3u32, 1, 4, 2, 0] {
            write(&root, &format!("f{index}.txt"), b"x");
        }

        let entries = collect(&root).unwrap();
        assert_eq!(entries.len(), 5);
        assert!((entries.len() as u64) <= MAX_FOLDER_ITEMS);
        let names: Vec<&str> = entries
            .iter()
            .map(|entry| entry.relative.as_str())
            .collect();
        assert_eq!(names, ["f0.txt", "f1.txt", "f2.txt", "f3.txt", "f4.txt"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
