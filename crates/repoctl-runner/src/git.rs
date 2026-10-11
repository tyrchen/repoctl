//! Merge-base-aware Git ranges and NUL-safe local change discovery.

use std::{collections::BTreeSet, path::Path};

use repoctl_core::{ProcessCommand, RepoRelativePath, RepoctlError, TaskExecutionOptions};

use super::{GitProvider, process::run_bytes};

/// Local Git provider including both sides of renamed paths.
#[derive(Clone, Debug, Default)]
pub struct LocalGitProvider;

impl GitProvider for LocalGitProvider {
    fn changed_files(
        &self,
        repo: Option<&Path>,
        base: &str,
        head: &str,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError> {
        let head = resolve(repo, head)?;
        let base = resolve(repo, base)?;
        let common = git(repo, &["merge-base", &base, &head])?;
        let common = std::str::from_utf8(&common)
            .map_err(|error| RepoctlError::Environment(error.to_string()))?
            .trim();
        parse_status(&git(
            repo,
            &[
                "diff",
                "--name-status",
                "-z",
                "--find-renames",
                common,
                &head,
                "--",
            ],
        )?)
    }
}

/// Includes staged, unstaged, deleted and untracked files for local commands.
pub(super) fn local_changes(repo: Option<&Path>) -> Result<Vec<RepoRelativePath>, RepoctlError> {
    let bytes = git(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut paths = BTreeSet::new();
    while let Some(field) = fields.next() {
        let status = field
            .get(..2)
            .ok_or_else(|| RepoctlError::Environment("malformed git status".to_string()))?;
        let path = field
            .get(3..)
            .ok_or_else(|| RepoctlError::Environment("malformed git status path".to_string()))?;
        paths.insert(parse_path(path)?);
        if status.contains(&b'R') || status.contains(&b'C') {
            paths.insert(parse_path(fields.next().ok_or_else(|| {
                RepoctlError::Environment("missing rename source".to_string())
            })?)?);
        }
    }
    Ok(paths.into_iter().collect())
}

fn parse_status(bytes: &[u8]) -> Result<Vec<RepoRelativePath>, RepoctlError> {
    let mut fields = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut paths = BTreeSet::new();
    while let Some(status) = fields.next() {
        paths.insert(parse_path(fields.next().ok_or_else(|| {
            RepoctlError::Environment("missing diff path".to_string())
        })?)?);
        if status.starts_with(b"R") || status.starts_with(b"C") {
            paths.insert(parse_path(fields.next().ok_or_else(|| {
                RepoctlError::Environment("missing rename destination".to_string())
            })?)?);
        }
    }
    Ok(paths.into_iter().collect())
}

fn parse_path(bytes: &[u8]) -> Result<RepoRelativePath, RepoctlError> {
    let path = std::str::from_utf8(bytes)
        .map_err(|error| RepoctlError::Environment(format!("git path is not UTF-8: {error}")))?;
    RepoRelativePath::new(path).map_err(RepoctlError::diagnostic)
}

fn resolve(repo: Option<&Path>, reference: &str) -> Result<String, RepoctlError> {
    if reference.is_empty()
        || reference.len() > 256
        || reference.starts_with('-')
        || reference.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RepoctlError::Environment(
            "invalid bounded git ref".to_string(),
        ));
    }
    let object = git(
        repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )?;
    Ok(std::str::from_utf8(&object)
        .map_err(|error| RepoctlError::Environment(error.to_string()))?
        .trim()
        .to_string())
}

fn git(repo: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, RepoctlError> {
    let mut execution = TaskExecutionOptions::default();
    execution.max_output_bytes = 16_777_216;
    execution.timeout_seconds = 30;
    let command = ProcessCommand {
        program: "git".to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        absolute_cwd: repo.map(Path::to_path_buf),
        execution,
        ..ProcessCommand::default()
    };
    let output = run_bytes(&command)?;
    if output.status != 0 {
        return Err(RepoctlError::Environment(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output.stdout)
}
