//! Versioned local task-result caching with explicit input/output contracts.

mod artifacts;
mod inputs;

use std::{
    fs,
    io::{Read as _, Write as _},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use artifacts::{read_entry, restore, snapshot_artifacts};
use blake3::Hasher;
use inputs::fingerprint;
use repoctl_core::{
    ProcessCommand, ProcessOutput, ProcessRunner, RepoRelativePath, RepoSnapshot, RepoctlError,
};
use serde::{Deserialize, Serialize};

const MAX_ENTRY_BYTES: usize = 67_108_864;
const MAX_ARTIFACTS: usize = 4096;
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Entry {
    version: u32,
    key: String,
    output: ProcessOutput,
    roots: Vec<RepoRelativePath>,
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Artifact {
    path: RepoRelativePath,
    bytes: Vec<u8>,
    digest: String,
    executable: bool,
    #[serde(default)]
    directory: bool,
}

pub(super) fn run(
    runner: &Arc<dyn ProcessRunner>,
    command: &ProcessCommand,
    snapshot: &RepoSnapshot,
    no_cache: bool,
    preceding_fingerprints: &[Option<String>],
) -> Result<ProcessOutput, RepoctlError> {
    if no_cache || !command.execution.cache.enabled {
        return runner.run(command);
    }
    validate_outputs(command, snapshot)?;
    if command.prerequisite_fingerprints.len() != command.prerequisites.len()
        || preceding_fingerprints.iter().any(Option::is_none)
    {
        let mut output = runner.run(command)?;
        output.fingerprint = None;
        output.cache_warning = Some(
            "cache bypassed: prerequisite task or earlier command has no artifact-aware \
             fingerprint"
                .to_string(),
        );
        return Ok(output);
    }
    let key = fingerprint(runner, command, snapshot, preceding_fingerprints)?;
    let root = snapshot.root.absolute.as_std_path();
    let dir = root.join(".repoctl/cache/tasks");
    safe_path(root, &dir)?;
    let path = dir.join(format!("{key}.json"));
    safe_path(root, &path)?;
    let existed = path.exists();
    if let Some(entry) = read_entry(&path, &key, command) {
        restore(root, &entry)?;
        let mut output = entry.output;
        output.cached = true;
        return Ok(output);
    }
    let mut output = runner.run(command)?;
    if output.status != 0 {
        return Ok(output);
    }
    let artifacts = snapshot_artifacts(root, &command.execution.cache.outputs)?;
    output.fingerprint = Some(artifact_fingerprint(&key, &artifacts));
    if existed {
        output.cache_warning =
            Some("ignored corrupt or incompatible local task cache entry".to_string());
    }
    let mut stored_output = output.clone();
    stored_output.cache_warning = None;
    let entry = Entry {
        version: 1,
        key,
        output: stored_output,
        roots: command.execution.cache.outputs.clone(),
        artifacts,
    };
    let bytes = serde_json::to_vec(&entry).map_err(|error| json_error(&error))?;
    if bytes.len() > MAX_ENTRY_BYTES {
        return Err(RepoctlError::Environment(
            "task cache entry exceeds 64 MiB".to_string(),
        ));
    }
    fs::create_dir_all(&dir)
        .map_err(|source| RepoctlError::io(dir.display().to_string(), source))?;
    safe_path(root, &dir)?;
    let temp = dir.join(format!(
        ".{}-{}.tmp",
        std::process::id(),
        TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|source| RepoctlError::io(temp.display().to_string(), source))?;
    file.write_all(&bytes)
        .map_err(|source| RepoctlError::io(temp.display().to_string(), source))?;
    file.sync_all()
        .map_err(|source| RepoctlError::io(temp.display().to_string(), source))?;
    if let Err(source) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(RepoctlError::io(path.display().to_string(), source));
    }
    Ok(output)
}

pub(super) fn validate_outputs(
    command: &ProcessCommand,
    snapshot: &RepoSnapshot,
) -> Result<(), RepoctlError> {
    if !command.execution.cache.enabled {
        return Ok(());
    }
    if command
        .execution
        .cache
        .inputs
        .iter()
        .any(|pattern| pattern.as_str().split('/').any(heavy))
    {
        return Err(RepoctlError::Environment(
            "cache source patterns cannot select excluded output or cache trees".to_string(),
        ));
    }
    let own = &command.execution.cache.outputs;
    let project = command
        .project
        .as_ref()
        .and_then(|name| snapshot.project(name))
        .ok_or_else(|| {
            RepoctlError::Environment("cached task requires an owned project".to_string())
        })?;
    let configs = project
        .workspaces
        .iter()
        .map(|workspace| {
            project
                .path
                .join_project(&workspace.manifest)
                .map_err(RepoctlError::diagnostic)
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (index, output) in own.iter().enumerate() {
        let input_overlap = overlaps_input(command, output);
        if !output.starts_with(&command.cwd)
            || output == &command.cwd
            || input_overlap
            || output.as_str().split('/').any(|part| {
                matches!(part, ".git" | ".repoctl") || root_config(part) || part == "project.yaml"
            })
            || configs.iter().any(|config| config.starts_with(output))
            || project.source.starts_with(output)
            || own.iter().enumerate().any(|(other_index, other)| {
                index != other_index && (output.starts_with(other) || other.starts_with(output))
            })
        {
            return Err(RepoctlError::Environment(format!(
                "unsafe or overlapping cache output `{output}`"
            )));
        }
        for project in &snapshot.projects {
            for (task, commands) in &project.tasks {
                for candidate in commands {
                    if command.project.as_ref() == Some(&project.name)
                        && command.task.as_ref() == Some(task)
                        && command.workspace.as_ref() == Some(&candidate.workspace)
                    {
                        continue;
                    }
                    if candidate.execution.cache.enabled
                        && candidate
                            .execution
                            .cache
                            .outputs
                            .iter()
                            .any(|other| output.starts_with(other) || other.starts_with(output))
                    {
                        return Err(RepoctlError::Environment(format!(
                            "cache output `{output}` overlaps another task"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn overlaps_input(command: &ProcessCommand, output: &RepoRelativePath) -> bool {
    command.execution.cache.inputs.iter().any(|pattern| {
        let pattern = pattern.as_str();
        let literal = pattern.split(['*', '?', '[', '{']).next().unwrap_or("");
        let prefix = if literal.len() == pattern.len() || literal.ends_with('/') {
            literal.trim_end_matches('/')
        } else {
            literal.rsplit_once('/').map_or("", |(parent, _)| parent)
        };
        prefix.is_empty()
            || output.as_str() == prefix
            || output
                .as_str()
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
            || prefix
                .strip_prefix(output.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn root_config(path: &str) -> bool {
    matches!(
        path,
        "repo.yaml"
            | "Cargo.toml"
            | "Cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "pyproject.toml"
            | "uv.lock"
            | "rust-toolchain.toml"
    )
}

fn artifact_fingerprint(key: &str, artifacts: &[Artifact]) -> String {
    let mut digest = Hasher::new();
    field(&mut digest, key.as_bytes());
    for artifact in artifacts {
        field(&mut digest, artifact.path.as_str().as_bytes());
        field(&mut digest, artifact.digest.as_bytes());
        field(
            &mut digest,
            &[u8::from(artifact.executable), u8::from(artifact.directory)],
        );
    }
    digest.finalize().to_hex().to_string()
}

fn safe_path(root: &Path, path: &Path) -> Result<(), RepoctlError> {
    if !path.starts_with(root) {
        return Err(RepoctlError::Environment(
            "cache path escaped repository".to_string(),
        ));
    }
    for ancestor in path.ancestors().take_while(|p| p.starts_with(root)) {
        if fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(RepoctlError::Environment(
                "cache paths cannot contain symlinks".to_string(),
            ));
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> Result<RepoRelativePath, RepoctlError> {
    RepoRelativePath::new(
        path.strip_prefix(root)
            .map_err(|error| RepoctlError::Environment(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/"),
    )
    .map_err(RepoctlError::diagnostic)
}

fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>, RepoctlError> {
    let metadata = fs::metadata(path)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    if !metadata.is_file() {
        return Err(RepoctlError::Environment(
            "cache reads require regular files".to_string(),
        ));
    }
    if metadata.len() > limit as u64 {
        return Err(RepoctlError::Environment(format!(
            "cache file `{}` exceeds limit",
            path.display()
        )));
    }
    let file = fs::File::open(path)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    if bytes.len() > limit {
        return Err(RepoctlError::Environment(
            "cache read exceeded limit".to_string(),
        ));
    }
    Ok(bytes)
}

fn field(hasher: &mut Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}
fn heavy(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".repoctl"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".venv"
            | "__pycache__"
            | ".next"
            | ".cache"
    )
}
fn json_error(error: &serde_json::Error) -> RepoctlError {
    RepoctlError::Environment(format!("task cache JSON: {error}"))
}
