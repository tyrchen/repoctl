//! Shared, bounded source traversal for inventory and reviewed copy planning.

// Migration is synchronous CLI orchestration, outside Tokio request handling.
#![allow(clippy::disallowed_types)]

use std::{
    fs::{self, File},
    io::Read as _,
};

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{Diagnostic, ProjectRelativePath, RepoctlError};

use super::ADOPTION_EXCLUDED_DIRS;

pub(super) const MAX_FILES: usize = 20_000;
pub(super) const MAX_ENTRIES: usize = 100_000;
pub(super) const MAX_SOURCES: usize = 256;
const MAX_DEPTH: usize = 64;
const MAX_TEXT_BYTES: u64 = 262_144;
const MAX_TOTAL_TEXT_BYTES: u64 = 67_108_864;

#[derive(Debug, Default)]
pub(super) struct ScanBudget {
    entries: usize,
    files: usize,
    text_bytes: u64,
}

impl ScanBudget {
    pub fn visit(&mut self, path: &Utf8Path, file: bool, depth: usize) -> Result<(), RepoctlError> {
        self.entries = self.entries.saturating_add(1);
        self.files = self.files.saturating_add(usize::from(file));
        if depth > MAX_DEPTH {
            return Err(limit("depth", path));
        }
        if self.entries > MAX_ENTRIES {
            return Err(limit("entries", path));
        }
        if self.files > MAX_FILES {
            return Err(limit("files", path));
        }
        Ok(())
    }

    fn text(&mut self, path: &Utf8Path) -> Result<(), RepoctlError> {
        let size = fs::metadata(path)
            .map_err(|error| RepoctlError::io(path.to_path_buf(), error))?
            .len();
        self.text_bytes = self.text_bytes.saturating_add(size);
        if size > MAX_TEXT_BYTES || self.text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(limit("text_bytes", path));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(super) struct SourceBudgets {
    pub inventory: ScanBudget,
    pub copy: ScanBudget,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum SourceEntryKind {
    File,
    Directory,
    Excluded,
}

#[derive(Debug)]
pub(super) struct SourceEntry {
    pub path: Utf8PathBuf,
    pub relative: ProjectRelativePath,
    pub kind: SourceEntryKind,
}

pub(super) fn limit(resource: &str, path: &Utf8Path) -> RepoctlError {
    RepoctlError::diagnostic(
        Diagnostic::error(
            format!("adoption.limit.{resource}"),
            format!(
                "adoption exceeds the {resource} budget (20000 files/operations, 100000 entries, \
                 256 sources, 64 levels, 256 KiB per inspected text file, 64 MiB total text)"
            ),
        )
        .with_path(path.as_str()),
    )
}

pub(super) fn walk_source(
    root: &Utf8Path,
    destination: &Utf8Path,
    budget: &mut ScanBudget,
    visitor: &mut impl FnMut(SourceEntry) -> Result<(), RepoctlError>,
) -> Result<(), RepoctlError> {
    let exclude_destination = destination.starts_with(root);
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        for entry in
            fs::read_dir(&directory).map_err(|error| RepoctlError::io(directory.clone(), error))?
        {
            let entry = entry.map_err(|error| RepoctlError::io(directory.clone(), error))?;
            let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
                RepoctlError::diagnostic(Diagnostic::error(
                    "path.non_utf8",
                    format!("path is not valid UTF-8: {}", path.display()),
                ))
            })?;
            if exclude_destination && path.starts_with(destination) {
                continue;
            }
            let entry = inspect_entry(root, path, budget, depth.saturating_add(1))?;
            if entry.kind == SourceEntryKind::Directory {
                pending.push((entry.path.clone(), depth.saturating_add(1)));
            }
            visitor(entry)?;
        }
    }
    Ok(())
}

fn inspect_entry(
    root: &Utf8Path,
    path: Utf8PathBuf,
    budget: &mut ScanBudget,
    depth: usize,
) -> Result<SourceEntry, RepoctlError> {
    let kind = fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type())
        .map_err(|error| RepoctlError::io(path.clone(), error))?;
    if kind.is_symlink() || (!kind.is_dir() && !kind.is_file()) {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            if kind.is_symlink() {
                "adoption.copy.symlink"
            } else {
                "adoption.copy.file_type"
            },
            format!(
                "adoption requires regular files/directories, not symlinks or special files: \
                 {path}"
            ),
        )));
    }
    let excluded = path
        .file_name()
        .is_some_and(|name| ADOPTION_EXCLUDED_DIRS.contains(&name));
    budget.visit(&path, kind.is_file() && !excluded, depth)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    let relative = ProjectRelativePath::new(relative.as_str()).map_err(RepoctlError::diagnostic)?;
    let entry_kind = if excluded {
        SourceEntryKind::Excluded
    } else if kind.is_dir() {
        SourceEntryKind::Directory
    } else {
        SourceEntryKind::File
    };
    if entry_kind == SourceEntryKind::File && is_inspected_text(&path) {
        budget.text(&path)?;
    }
    Ok(SourceEntry {
        path,
        relative,
        kind: entry_kind,
    })
}

fn is_inspected_text(path: &Utf8Path) -> bool {
    path.file_name().is_some_and(|name| {
        matches!(
            name,
            "Cargo.toml"
                | "package.json"
                | "pyproject.toml"
                | "README.md"
                | "build.rs"
                | "Pulumi.yaml"
                | "operon.yaml"
        )
    })
}

pub(super) fn read_text(path: &Utf8Path) -> Result<String, RepoctlError> {
    let file = File::open(path).map_err(|error| RepoctlError::io(path.to_path_buf(), error))?;
    let mut text = String::new();
    file.take(MAX_TEXT_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| RepoctlError::io(path.to_path_buf(), error))?;
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err(limit("text_bytes", path));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use camino::Utf8Path;
    use repoctl_core::{RepoRelativePath, RepoctlError};

    use super::{
        MAX_DEPTH, MAX_ENTRIES, MAX_FILES, MAX_TEXT_BYTES, MAX_TOTAL_TEXT_BYTES, ScanBudget,
        read_text, walk_source,
    };
    use crate::adoption::{files::copy_operations_for_source, inventory::inventory_source};

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn test_should_share_file_budgets_across_inventory_and_copy_sources() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = Utf8Path::from_path(temp.path()).ok_or("non-UTF8 fixture")?;
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first)?;
        fs::create_dir_all(&second)?;
        fs::write(first.join("file"), "first")?;
        fs::write(second.join("file"), "second")?;
        let destination = root.join("destination");
        let mut inventory = ScanBudget {
            files: MAX_FILES - 1,
            ..ScanBudget::default()
        };
        inventory_source(&first, "first", &destination, &mut inventory)?;
        let error = inventory_source(&second, "second", &destination, &mut inventory)
            .err()
            .ok_or("file limit missing")?;
        assert!(error.to_string().contains("adoption.limit.files"));
        let mut copy = ScanBudget {
            files: MAX_FILES - 1,
            ..ScanBudget::default()
        };
        assert_eq!(
            copy_operations_for_source(
                &first,
                &RepoRelativePath::new("apps/first").map_err(RepoctlError::diagnostic)?,
                &destination,
                &mut copy
            )?
            .len(),
            1
        );
        let error = copy_operations_for_source(
            &second,
            &RepoRelativePath::new("apps/second").map_err(RepoctlError::diagnostic)?,
            &destination,
            &mut copy,
        )
        .err()
        .ok_or("copy limit missing")?;
        assert!(error.to_string().contains("adoption.limit.files"));
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn test_should_bound_directory_entries_and_depth_before_visiting() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = Utf8Path::from_path(temp.path()).ok_or("non-UTF8 fixture")?;
        fs::create_dir(root.join("directory"))?;
        let mut budget = ScanBudget {
            entries: MAX_ENTRIES,
            ..ScanBudget::default()
        };
        let mut visited = false;
        let result = walk_source(root, &root.join("destination"), &mut budget, &mut |_| {
            visited = true;
            Ok(())
        });
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("adoption.limit.entries"))
        );
        assert!(!visited);
        let mut path = root.join("directory");
        for _ in 0..MAX_DEPTH {
            path = path.join("d");
            fs::create_dir(&path)?;
        }
        let result = walk_source(
            root,
            &root.join("destination"),
            &mut ScanBudget::default(),
            &mut |_| Ok(()),
        );
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("adoption.limit.depth"))
        );
        Ok(())
    }

    #[test]
    fn test_should_bound_individual_and_aggregate_inspected_text() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = Utf8Path::from_path(temp.path()).ok_or("non-UTF8 fixture")?;
        let manifest = root.join("package.json");
        fs::write(&manifest, "{}")?;
        let mut budget = ScanBudget {
            text_bytes: MAX_TOTAL_TEXT_BYTES - 1,
            ..ScanBudget::default()
        };
        let result = walk_source(
            root,
            &root.join("destination"),
            &mut budget,
            &mut |_| Ok(()),
        );
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("adoption.limit.text_bytes"))
        );
        fs::write(&manifest, vec![b' '; usize::try_from(MAX_TEXT_BYTES)? + 1])?;
        assert!(
            read_text(&manifest)
                .err()
                .is_some_and(|error| error.to_string().contains("adoption.limit.text_bytes"))
        );
        let result = walk_source(
            root,
            &root.join("destination"),
            &mut ScanBudget::default(),
            &mut |_| Ok(()),
        );
        assert!(
            result
                .err()
                .is_some_and(|error| error.to_string().contains("adoption.limit.text_bytes"))
        );
        Ok(())
    }
}
