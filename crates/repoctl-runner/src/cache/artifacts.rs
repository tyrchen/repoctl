//! Bounded cache decoding, artifact snapshots and validated restoration.

use std::{collections::BTreeSet, fs, path::Path};

use repoctl_core::{ProcessCommand, RepoRelativePath, RepoctlError};

use super::{
    Artifact, Entry, MAX_ARTIFACTS, MAX_ENTRY_BYTES, artifact_fingerprint, bounded_read, relative,
    safe_path,
};

pub(super) fn read_entry(path: &Path, key: &str, command: &ProcessCommand) -> Option<Entry> {
    if !path.is_file() {
        return None;
    }
    let Ok(bytes) = bounded_read(path, MAX_ENTRY_BYTES) else {
        return None;
    };
    let Ok(entry) = serde_json::from_slice::<Entry>(&bytes) else {
        return None;
    };
    if entry.version != 1
        || entry.key != key
        || entry.output.status != 0
        || entry.roots != command.execution.cache.outputs
        || entry.artifacts.len() > MAX_ARTIFACTS
    {
        return None;
    }
    if entry.output.stdout.len() > command.execution.max_output_bytes
        || entry.output.stderr.len() > command.execution.max_output_bytes
    {
        return None;
    }
    let mut paths = BTreeSet::new();
    for artifact in &entry.artifacts {
        if RepoRelativePath::new(artifact.path.as_str()).is_err()
            || !paths.insert(artifact.path.clone())
            || !entry
                .roots
                .iter()
                .any(|root| artifact.path.starts_with(root))
            || artifact.directory && !artifact.bytes.is_empty()
            || blake3::hash(&artifact.bytes).to_hex().as_str() != artifact.digest
        {
            return None;
        }
    }
    if entry.output.fingerprint.as_deref()
        != Some(artifact_fingerprint(key, &entry.artifacts).as_str())
    {
        return None;
    }
    let directories = entry
        .artifacts
        .iter()
        .filter(|artifact| artifact.directory)
        .map(|artifact| artifact.path.as_str())
        .collect::<BTreeSet<_>>();
    if entry.roots.iter().any(|root| !paths.contains(root)) {
        return None;
    }
    for artifact in &entry.artifacts {
        let mut parent = Path::new(artifact.path.as_str()).parent();
        while let Some(path) = parent {
            let path_str = path.to_str()?;
            if !entry.roots.iter().any(|root| {
                path_str == root.as_str()
                    || path_str
                        .strip_prefix(root.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            }) {
                break;
            }
            if !directories.contains(path_str) {
                return None;
            }
            parent = path.parent();
        }
    }
    Some(entry)
}

pub(super) fn snapshot_artifacts(
    root: &Path,
    outputs: &[RepoRelativePath],
) -> Result<Vec<Artifact>, RepoctlError> {
    let mut artifacts = Vec::new();
    for output in outputs {
        let path = root.join(output.as_str());
        if !path.exists() {
            return Err(RepoctlError::Environment(format!(
                "declared cache output `{output}` is missing"
            )));
        }
        collect_artifacts(root, &path, &mut artifacts, 0)?;
    }
    artifacts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(artifacts)
}

fn collect_artifacts(
    root: &Path,
    path: &Path,
    artifacts: &mut Vec<Artifact>,
    depth: u32,
) -> Result<(), RepoctlError> {
    safe_path(root, path)?;
    if depth > 64 {
        return Err(RepoctlError::Environment(
            "cache artifacts exceed 64 directory levels".to_string(),
        ));
    }
    if artifacts.len() >= MAX_ARTIFACTS {
        return Err(RepoctlError::Environment(
            "too many cache artifacts".to_string(),
        ));
    }
    if path.is_dir() {
        artifacts.push(Artifact {
            path: relative(root, path)?,
            bytes: Vec::new(),
            digest: blake3::hash(b"").to_hex().to_string(),
            executable: false,
            directory: true,
        });
        for entry in fs::read_dir(path)
            .map_err(|source| RepoctlError::io(path.display().to_string(), source))?
        {
            let entry =
                entry.map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
            collect_artifacts(root, &entry.path(), artifacts, depth.saturating_add(1))?;
        }
    } else {
        if artifacts.len() >= MAX_ARTIFACTS {
            return Err(RepoctlError::Environment(
                "too many cache artifacts".to_string(),
            ));
        }
        let bytes = bounded_read(path, MAX_ENTRY_BYTES)?;
        if artifacts
            .iter()
            .map(|a| a.bytes.len())
            .sum::<usize>()
            .saturating_add(bytes.len())
            > MAX_ENTRY_BYTES
        {
            return Err(RepoctlError::Environment(
                "cache artifacts exceed 64 MiB".to_string(),
            ));
        }
        artifacts.push(Artifact {
            path: relative(root, path)?,
            digest: blake3::hash(&bytes).to_hex().to_string(),
            bytes,
            executable: executable(path)?,
            directory: false,
        });
    }
    Ok(())
}

pub(super) fn restore(root: &Path, entry: &Entry) -> Result<(), RepoctlError> {
    for output in &entry.roots {
        safe_path(root, &root.join(output.as_str()))?;
    }
    for artifact in &entry.artifacts {
        safe_path(root, &root.join(artifact.path.as_str()))?;
    }
    for output in &entry.roots {
        let path = root.join(output.as_str());
        if path.is_dir() {
            fs::remove_dir_all(&path)
                .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
        } else if path.exists() {
            fs::remove_file(&path)
                .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
        }
    }
    for output in &entry.roots {
        if !entry
            .artifacts
            .iter()
            .any(|artifact| artifact.path == *output)
        {
            let path = root.join(output.as_str());
            fs::create_dir_all(&path)
                .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
        }
    }
    for artifact in &entry.artifacts {
        let path = root.join(artifact.path.as_str());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|source| RepoctlError::io(parent.display().to_string(), source))?;
        }
        if artifact.directory {
            fs::create_dir_all(&path)
                .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
            continue;
        }
        fs::write(&path, &artifact.bytes)
            .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
        set_executable(&path, artifact.executable)?;
    }
    Ok(())
}

#[cfg(unix)]
fn executable(path: &Path) -> Result<bool, RepoctlError> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))
}
#[cfg(not(unix))]
fn executable(_path: &Path) -> Result<bool, RepoctlError> {
    Ok(false)
}
#[cfg(unix)]
fn set_executable(path: &Path, enabled: bool) -> Result<(), RepoctlError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if enabled { 0o755 } else { 0o644 }),
    )
    .map_err(|source| RepoctlError::io(path.display().to_string(), source))
}
#[cfg(not(unix))]
fn set_executable(_path: &Path, _enabled: bool) -> Result<(), RepoctlError> {
    Ok(())
}
