//! Conservative cache keys including source closure, runtimes and environment.

use std::{collections::BTreeSet, path::Path, sync::Arc};

use blake3::Hasher;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use repoctl_core::{
    CommandSpec, ProcessCommand, ProcessRunner, RepoRelativePath, RepoSnapshot, RepoctlError,
};

use super::{
    MAX_ENTRY_BYTES, bounded_read, field, heavy, json_error, relative, root_config, safe_path,
};

const MAX_VISITED_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;

pub(super) fn fingerprint(
    runner: &Arc<dyn ProcessRunner>,
    command: &ProcessCommand,
    snapshot: &RepoSnapshot,
    preceding_fingerprints: &[Option<String>],
) -> Result<String, RepoctlError> {
    let mut hasher = Hasher::new();
    field(&mut hasher, env!("CARGO_PKG_VERSION").as_bytes());
    field(&mut hasher, std::env::consts::OS.as_bytes());
    field(&mut hasher, std::env::consts::ARCH.as_bytes());
    field(
        &mut hasher,
        &serde_json::to_vec(command).map_err(|error| json_error(&error))?,
    );
    field(&mut hasher, b"preceding-command-artifacts");
    field(
        &mut hasher,
        &serde_json::to_vec(preceding_fingerprints).map_err(|error| json_error(&error))?,
    );
    for name in command
        .execution
        .cache
        .env
        .iter()
        .map(String::as_str)
        .chain(["PATH", "PATHEXT", "RUSTUP_TOOLCHAIN", "VIRTUAL_ENV"])
    {
        field(&mut hasher, name.as_bytes());
        field(
            &mut hasher,
            std::env::var_os(name)
                .as_deref()
                .map_or(b"<unset>".as_slice(), |v| v.as_encoded_bytes()),
        );
    }
    hash_runtimes(&mut hasher, runner, command)?;
    let mut builder = GlobSetBuilder::new();
    for pattern in &command.execution.cache.inputs {
        builder.add(
            Glob::new(pattern.as_str())
                .map_err(|error| RepoctlError::Environment(error.to_string()))?,
        );
    }
    let inputs = builder
        .build()
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    let roots = dependency_roots(command, snapshot);
    let outputs = snapshot
        .projects
        .iter()
        .flat_map(|p| p.tasks.values().flatten())
        .filter(|task| task.execution.cache.enabled)
        .flat_map(|t| &t.execution.cache.outputs)
        .collect::<Vec<_>>();
    let root = snapshot.root.absolute.as_std_path();
    let files = collect_files(root, &inputs, &roots, &outputs, MAX_VISITED_ENTRIES)?;
    for path in files {
        let absolute = root.join(path.as_str());
        safe_path(root, &absolute)?;
        let bytes = bounded_read(&absolute, MAX_ENTRY_BYTES)?;
        field(&mut hasher, path.as_str().as_bytes());
        field(&mut hasher, &bytes);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_files(
    root: &Path,
    inputs: &GlobSet,
    roots: &[RepoRelativePath],
    outputs: &[&RepoRelativePath],
    entry_limit: usize,
) -> Result<BTreeSet<RepoRelativePath>, RepoctlError> {
    let mut files = BTreeSet::new();
    for (visited, entry) in WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .filter_entry(|entry| !heavy(entry.file_name().to_string_lossy().as_ref()))
        .build()
        .enumerate()
    {
        let entry = entry.map_err(|error| RepoctlError::Environment(error.to_string()))?;
        if visited >= entry_limit || entry.depth() > MAX_DEPTH {
            return Err(RepoctlError::Environment(format!(
                "cache input traversal exceeds {entry_limit} entries or {MAX_DEPTH} levels"
            )));
        }
        if entry.path() == root {
            continue;
        }
        let relative = relative(root, entry.path())?;
        if entry.file_type().is_some_and(|kind| kind.is_symlink())
            && (inputs.is_match(relative.as_str())
                || roots.iter().any(|prefix| relative.starts_with(prefix)))
        {
            return Err(RepoctlError::Environment(
                "cache inputs cannot contain symlinks".to_string(),
            ));
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if !inputs.is_match(relative.as_str())
            && outputs.iter().any(|output| relative.starts_with(output))
        {
            continue;
        }
        if inputs.is_match(relative.as_str())
            || roots.iter().any(|prefix| relative.starts_with(prefix))
            || root_config(relative.as_str())
        {
            files.insert(relative);
        }
        if files.len() > 20_000 {
            return Err(RepoctlError::Environment(
                "cache input set exceeds 20000 files".to_string(),
            ));
        }
    }
    Ok(files)
}

fn hash_runtimes(
    hasher: &mut Hasher,
    runner: &Arc<dyn ProcessRunner>,
    command: &ProcessCommand,
) -> Result<(), RepoctlError> {
    let mut probes = command.execution.cache.runtime.clone();
    if matches!(
        command.program.as_str(),
        "cargo" | "rustc" | "node" | "npm" | "pnpm" | "yarn" | "bun" | "uv" | "python" | "python3"
    ) {
        probes.push(CommandSpec {
            program: command.program.clone(),
            args: vec!["--version".to_string()],
        });
    }
    for probe in &probes {
        let probe = ProcessCommand {
            program: probe.program.clone(),
            args: probe.args.clone(),
            execution: command.execution.clone(),
            ..command.clone()
        };
        let output = runner.run(&probe)?;
        if output.status != 0 {
            return Err(RepoctlError::Environment(format!(
                "cache runtime probe `{}` failed",
                probe.program
            )));
        }
        field(hasher, output.stdout.as_bytes());
        field(hasher, output.stderr.as_bytes());
    }
    Ok(())
}

fn dependency_roots(command: &ProcessCommand, snapshot: &RepoSnapshot) -> Vec<RepoRelativePath> {
    let mut names = command
        .project
        .iter()
        .map(ToString::to_string)
        .chain(
            command
                .prerequisites
                .iter()
                .filter_map(|id| id.split_once(':').map(|(project, _)| project.to_string())),
        )
        .collect::<BTreeSet<_>>();
    loop {
        let mut changed = false;
        for edge in &snapshot.graph.edges {
            if let (Some(from), Some(to)) = (
                edge.from.strip_prefix("project:"),
                edge.to.strip_prefix("project:"),
            ) && names.contains(from)
            {
                changed |= names.insert(to.to_string());
            }
        }
        if !changed {
            break;
        }
    }
    snapshot
        .projects
        .iter()
        .filter(|p| names.contains(p.name.as_str()))
        .map(|p| p.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use globset::GlobSet;

    use super::{MAX_DEPTH, collect_files};

    #[test]
    fn test_should_cap_unmatched_files_and_empty_directories() -> Result<(), Box<dyn Error>> {
        for directories in [false, true] {
            let temp = tempfile::tempdir()?;
            for index in 0..5 {
                let path = temp.path().join(format!("unowned-{index}"));
                if directories {
                    fs::create_dir(path)?;
                } else {
                    fs::write(path, "unrelated")?;
                }
            }
            let error = collect_files(temp.path(), &GlobSet::empty(), &[], &[], 4)
                .expect_err("unmatched entries must count");
            assert!(error.to_string().contains("4 entries"));
        }
        Ok(())
    }

    #[test]
    fn test_should_bound_unmatched_directory_depth() -> Result<(), Box<dyn Error>> {
        let temp = tempfile::tempdir()?;
        let mut path = temp.path().to_path_buf();
        for _ in 0..=MAX_DEPTH {
            path.push("nested");
            fs::create_dir(&path)?;
        }
        assert!(collect_files(temp.path(), &GlobSet::empty(), &[], &[], 1000).is_err());
        Ok(())
    }
}
