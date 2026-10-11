//! Cargo workspace ownership for bounded offline adoption grouping.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use repoctl_core::{ProjectRelativePath, RepoRoot, RepoctlError};
use toml_edit::DocumentMut;

use super::{
    MAX_MEMBERS, TraversalScope, enqueue_path_package, manifest_error, members, parse_toml,
    patterns, read_manifest, visit_dependencies,
};

const MAX_TOTAL_BYTES: usize = 67_108_864;

type Documents = BTreeMap<PathBuf, DocumentMut>;

pub(crate) fn workspace_roots(
    root: &RepoRoot,
    manifests: &[ProjectRelativePath],
) -> Result<Vec<ProjectRelativePath>, RepoctlError> {
    let absolute = root
        .absolute
        .canonicalize_utf8()
        .map_err(|error| RepoctlError::io(root.absolute.clone(), error))?;
    let repository = absolute.as_std_path();
    let documents = load_documents(repository, manifests)?;
    let mut owners = BTreeMap::new();
    for (manifest, document) in &documents {
        if let Some(workspace) = document.get("workspace") {
            if workspace.as_table_like().is_none() {
                return Err(manifest_error(manifest, "workspace must be a table"));
            }
            collect_members(repository, manifest, document, &documents, &mut owners)?;
        }
    }
    for (manifest, document) in &documents {
        if !owners.contains_key(manifest) {
            validate_standalone(manifest, document, &documents)?;
            owners.insert(manifest.clone(), manifest.clone());
        }
    }
    owners
        .values()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|manifest| {
            let relative = manifest
                .strip_prefix(repository)
                .map_err(|error| manifest_error(manifest, &error.to_string()))?;
            ProjectRelativePath::new(relative.to_string_lossy().replace('\\', "/"))
                .map_err(RepoctlError::diagnostic)
        })
        .collect()
}

fn load_documents(
    repository: &Path,
    manifests: &[ProjectRelativePath],
) -> Result<Documents, RepoctlError> {
    if manifests.len() > MAX_MEMBERS {
        return Err(manifest_error(
            repository,
            "Cargo inventory exceeds 4096 manifests",
        ));
    }
    let mut documents = BTreeMap::new();
    let mut bytes = 0_usize;
    for relative in manifests {
        let path = repository.join(relative.as_str());
        if path.file_name().and_then(|name| name.to_str()) != Some("Cargo.toml") {
            return Err(manifest_error(
                &path,
                "Cargo inventory entries must be Cargo.toml files",
            ));
        }
        let text = read_manifest(&path, repository)?
            .ok_or_else(|| manifest_error(&path, "Cargo inventory manifest is missing"))?;
        bytes = bytes.saturating_add(text.len());
        if bytes > MAX_TOTAL_BYTES {
            return Err(manifest_error(
                repository,
                "Cargo inventory exceeds 64 MiB of manifest text",
            ));
        }
        let canonical = path
            .canonicalize()
            .map_err(|error| RepoctlError::io(path.display().to_string(), error))?;
        if canonical.to_str().is_none() {
            return Err(manifest_error(
                &path,
                "Cargo canonical manifest path is not UTF-8",
            ));
        }
        documents.insert(canonical, parse_toml(&text, &path)?);
    }
    Ok(documents)
}

fn collect_members(
    repository: &Path,
    manifest: &Path,
    document: &DocumentMut,
    documents: &Documents,
    owners: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<(), RepoctlError> {
    let directory = manifest
        .parent()
        .ok_or_else(|| manifest_error(manifest, "manifest has no parent"))?;
    let excluded = patterns(document, "exclude", manifest)?;
    let inventory = documents.keys().cloned().collect::<BTreeSet<_>>();
    let scope = TraversalScope {
        root: directory,
        exclusions: Some((&excluded, directory)),
        inventory: Some(&inventory),
    };
    let mut queue = VecDeque::from([(manifest.to_path_buf(), document.clone())]);
    for member in members(directory, manifest, document, &excluded, repository)? {
        let doc = documents.get(&member).ok_or_else(|| {
            manifest_error(
                &member,
                "Cargo member is absent from the inventoried manifest set",
            )
        })?;
        queue.push_back((member, doc.clone()));
    }
    let mut visited = BTreeSet::new();
    while let Some((path, document)) = queue.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }
        validate_member(&path, &document, manifest, documents, owners)?;
        owners.insert(path.clone(), manifest.to_path_buf());
        visit_dependencies(&path, &document, repository, &mut |origin, local, _| {
            enqueue_path_package(origin, local, &scope, repository, &visited, &mut queue)
        })?;
    }
    Ok(())
}

fn validate_member(
    path: &Path,
    document: &DocumentMut,
    workspace: &Path,
    documents: &Documents,
    owners: &BTreeMap<PathBuf, PathBuf>,
) -> Result<(), RepoctlError> {
    if !documents.contains_key(path) {
        return Err(manifest_error(
            path,
            "Cargo member is absent from the inventoried manifest set",
        ));
    }
    if (path != workspace && document.get("workspace").is_some())
        || owners.get(path).is_some_and(|owner| owner != workspace)
    {
        return Err(manifest_error(
            path,
            "Cargo package belongs to multiple workspaces",
        ));
    }
    if let Some(pointer) = workspace_pointer(document, path)? {
        let directory = path
            .parent()
            .ok_or_else(|| manifest_error(path, "manifest has no parent"))?;
        let expected = directory.join(pointer).join("Cargo.toml");
        let expected = fs::canonicalize(&expected)
            .map_err(|error| RepoctlError::io(expected.display().to_string(), error))?;
        if expected != workspace {
            return Err(manifest_error(
                path,
                "package.workspace points to a different Cargo workspace",
            ));
        }
    }
    Ok(())
}

fn validate_standalone(
    path: &Path,
    document: &DocumentMut,
    documents: &Documents,
) -> Result<(), RepoctlError> {
    if workspace_pointer(document, path)?.is_some() {
        return Err(manifest_error(
            path,
            "package is not a member of its explicit Cargo workspace",
        ));
    }
    for ancestor in path
        .parent()
        .and_then(Path::parent)
        .into_iter()
        .flat_map(Path::ancestors)
    {
        let root = ancestor.join("Cargo.toml");
        if let Some(doc) = documents.get(&root)
            && doc.get("workspace").is_some()
        {
            let relative = path
                .parent()
                .and_then(|parent| parent.strip_prefix(ancestor).ok())
                .ok_or_else(|| manifest_error(path, "Cargo package escaped workspace"))?;
            if patterns(doc, "exclude", &root)?.is_match(relative) {
                return Ok(());
            }
            return Err(manifest_error(
                path,
                "package is under a Cargo workspace but is not a member or excluded",
            ));
        }
    }
    Ok(())
}

fn workspace_pointer<'a>(
    document: &'a DocumentMut,
    path: &Path,
) -> Result<Option<&'a str>, RepoctlError> {
    let Some(value) = document
        .get("package")
        .and_then(|package| package.get("workspace"))
    else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| manifest_error(path, "package.workspace must be a string"))?;
    if value.len() > 4096 || value.contains('\0') {
        return Err(manifest_error(path, "invalid package.workspace path"));
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use camino::Utf8Path;
    use repoctl_core::{ProjectRelativePath, RepoRoot, RepoctlError};

    use super::workspace_roots;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn test_should_reject_members_missing_from_the_inventory() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = Utf8Path::from_path(temp.path()).ok_or("non-UTF8 fixture")?;
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers=['child']\n")?;
        fs::create_dir(root.join("child"))?;
        fs::write(
            root.join("child/Cargo.toml"),
            "[package]\nname='child'\nversion='0.1.0'\n",
        )?;
        let root = RepoRoot::new(root.to_path_buf()).map_err(RepoctlError::diagnostic)?;
        let manifests = [ProjectRelativePath::new("Cargo.toml").map_err(RepoctlError::diagnostic)?];
        let error = workspace_roots(&root, &manifests)
            .err()
            .ok_or("missing inventory was accepted")?;
        assert!(
            error
                .to_string()
                .contains("absent from the inventoried manifest set")
        );
        Ok(())
    }

    #[test]
    fn test_should_reject_conflicting_explicit_workspace_pointers() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = Utf8Path::from_path(temp.path()).ok_or("non-UTF8 fixture")?;
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers=['child']\nexclude=['other']\n",
        )?;
        fs::create_dir(root.join("child"))?;
        fs::create_dir(root.join("other"))?;
        fs::write(
            root.join("child/Cargo.toml"),
            "[package]\nname='child'\nversion='0.1.0'\nworkspace='../other'\n",
        )?;
        fs::write(root.join("other/Cargo.toml"), "[workspace]\n")?;
        let root = RepoRoot::new(root.to_path_buf()).map_err(RepoctlError::diagnostic)?;
        let manifests = ["Cargo.toml", "child/Cargo.toml", "other/Cargo.toml"]
            .into_iter()
            .map(|path| ProjectRelativePath::new(path).map_err(RepoctlError::diagnostic))
            .collect::<Result<Vec<_>, _>>()?;
        let error = workspace_roots(&root, &manifests)
            .err()
            .ok_or("conflicting pointer was accepted")?;
        assert!(error.to_string().contains("different Cargo workspace"));
        Ok(())
    }
}
