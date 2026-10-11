//! Explicit, project-scoped source snippets within a serialized byte budget.

use std::{
    collections::BTreeSet,
    fs,
    io::{BufReader, Read},
    path::Path,
};

use globset::{Glob, GlobSetBuilder};
use repoctl_core::{
    AiContextRequest, Diagnostic, ProjectManifest, RepoRelativePath, RepoSnapshot, RepoctlError,
};
use serde_json::{Value, json};

const MAX_DOC_ENTRIES: usize = 4096;
const MAX_CONTEXT_FILES: usize = 512;

pub(super) fn enrich(
    mut payload: Value,
    request: &AiContextRequest,
    snapshot: &RepoSnapshot,
    project: &ProjectManifest,
) -> Result<Value, RepoctlError> {
    if !(256..=1_048_576).contains(&request.max_bytes) || request.changed_files.len() > 256 {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "context.budget",
            "context budget must be 256..=1048576 bytes and at most 256 changed files",
        )));
    }
    if request.changed_files.is_empty() && !request.include_docs {
        if size(&payload)?.saturating_add(1) > request.max_bytes {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "context.budget",
                "context budget is too small for mandatory project metadata",
            )));
        }
        return Ok(payload);
    }
    let paths = selected_paths(request, snapshot, project)?;
    let mut exclusions = GlobSetBuilder::new();
    for pattern in &project.ai.do_not_edit {
        exclusions.add(
            Glob::new(pattern.as_str())
                .map_err(|error| RepoctlError::Environment(error.to_string()))?,
        );
        exclusions.add(
            Glob::new(&format!("{}/{}", project.path, pattern))
                .map_err(|error| RepoctlError::Environment(error.to_string()))?,
        );
    }
    let exclusions = exclusions
        .build()
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    initialize(&mut payload, request.max_bytes);
    if size(&payload)?.saturating_add(128) > request.max_bytes {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "context.budget",
            "context budget is too small for mandatory project metadata",
        )));
    }
    let mut omitted = 0_usize;
    for path in paths {
        let reason = if !project.contains_path(&path) {
            Some("outside project")
        } else if exclusions.is_match(path.as_str()) || excluded(path.as_str()) {
            Some("protected, generated or secret-like path")
        } else {
            None
        };
        if let Some(reason) = reason {
            omit(&mut payload, &path, reason, request.max_bytes, &mut omitted)?;
            continue;
        }
        let text = match read_text(snapshot, &path) {
            Ok(text) => text,
            Err(reason) => {
                omit(&mut payload, &path, reason, request.max_bytes, &mut omitted)?;
                continue;
            }
        };
        let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
        let mut end = text.len().min(request.max_bytes);
        loop {
            while !text.is_char_boundary(end) {
                end = end.saturating_sub(1);
            }
            let snippet = json!({"path": path, "hash": hash, "content": text.get(..end).unwrap_or(""), "truncated": end < text.len()});
            let mut trial = payload.clone();
            if let Some(snippets) = trial["snippets"].as_array_mut() {
                snippets.push(snippet);
            }
            if size(&trial)?.saturating_add(128) <= request.max_bytes {
                payload = trial;
                break;
            }
            if end == 0 {
                omit(
                    &mut payload,
                    &path,
                    "byte budget exhausted",
                    request.max_bytes,
                    &mut omitted,
                )?;
                break;
            }
            end /= 2;
        }
    }
    payload["omittedCount"] = json!(omitted);
    if size(&payload)? > request.max_bytes {
        return Err(RepoctlError::Internal(
            "context budget accounting failed".to_string(),
        ));
    }
    Ok(payload)
}

fn selected_paths(
    request: &AiContextRequest,
    snapshot: &RepoSnapshot,
    project: &ProjectManifest,
) -> Result<BTreeSet<RepoRelativePath>, RepoctlError> {
    let mut paths = request
        .changed_files
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if request.include_docs {
        let mut visited = 0_usize;
        for doc in &project.ai.docs {
            visit_document_entry(&mut visited)?;
            let path = project
                .path
                .join_project(doc)
                .map_err(RepoctlError::diagnostic)?;
            let absolute = snapshot.root.join(&path);
            if absolute.is_dir()
                && absolute
                    .as_std_path()
                    .ancestors()
                    .take_while(|p| p.starts_with(snapshot.root.absolute.as_std_path()))
                    .all(|p| !fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
            {
                collect_documents(
                    snapshot.root.absolute.as_std_path(),
                    absolute.as_std_path(),
                    &mut paths,
                    &mut visited,
                )?;
            } else {
                paths.insert(path);
            }
            validate_selection(&paths)?;
        }
    }
    Ok(paths)
}

fn collect_documents(
    root: &Path,
    directory: &Path,
    paths: &mut BTreeSet<RepoRelativePath>,
    visited: &mut usize,
) -> Result<(), RepoctlError> {
    if excluded(relative_document_path(root, directory)?.as_str()) {
        return Ok(());
    }
    let mut pending = vec![(directory.to_path_buf(), 0_usize)];
    while let Some((directory, depth)) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| RepoctlError::io(directory.display().to_string(), error))?
        {
            visit_document_entry(visited)?;
            let entry =
                entry.map_err(|error| RepoctlError::io(directory.display().to_string(), error))?;
            let path = entry.path();
            let relative = relative_document_path(root, &path)?;
            if excluded(relative.as_str()) {
                continue;
            }
            let kind = entry
                .file_type()
                .map_err(|error| RepoctlError::io(path.display().to_string(), error))?;
            if kind.is_dir() && depth < 31 {
                pending.push((path, depth.saturating_add(1)));
            } else if kind.is_file() || kind.is_symlink() {
                paths.insert(relative);
                validate_selection(paths)?;
            }
        }
    }
    Ok(())
}

fn relative_document_path(root: &Path, path: &Path) -> Result<RepoRelativePath, RepoctlError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    let relative = relative
        .to_str()
        .ok_or_else(|| RepoctlError::Environment("documentation path is not UTF-8".to_string()))?;
    RepoRelativePath::new(relative.replace('\\', "/")).map_err(RepoctlError::diagnostic)
}

fn visit_document_entry(visited: &mut usize) -> Result<(), RepoctlError> {
    *visited = visited.saturating_add(1);
    if *visited > MAX_DOC_ENTRIES {
        return Err(RepoctlError::Environment(format!(
            "context documentation traversal exceeds {MAX_DOC_ENTRIES} entries"
        )));
    }
    Ok(())
}

fn validate_selection(paths: &BTreeSet<RepoRelativePath>) -> Result<(), RepoctlError> {
    if paths.len() > MAX_CONTEXT_FILES {
        return Err(RepoctlError::Environment(format!(
            "context selection exceeds {MAX_CONTEXT_FILES} files"
        )));
    }
    Ok(())
}

fn initialize(payload: &mut Value, budget: usize) {
    payload["contextVersion"] = json!(1);
    payload["snippets"] = json!([]);
    payload["omitted"] = json!([]);
    payload["omittedCount"] = json!(0);
    payload["maxBytes"] = json!(budget);
}

fn read_text(snapshot: &RepoSnapshot, path: &RepoRelativePath) -> Result<String, &'static str> {
    let absolute = snapshot.root.join(path);
    let canonical = fs::canonicalize(&absolute).map_err(|_| "unreadable or missing")?;
    if !canonical.starts_with(snapshot.root.absolute.as_std_path())
        || absolute
            .as_std_path()
            .ancestors()
            .take_while(|p| p.starts_with(snapshot.root.absolute.as_std_path()))
            .any(|p| fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()))
    {
        return Err("symlink or escaped path");
    }
    if !canonical.is_file() {
        return Err("not a regular file");
    }
    let file = fs::File::open(&canonical).map_err(|_| "unreadable or missing")?;
    let mut bytes = Vec::new();
    BufReader::new(file)
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| "unreadable or missing")?;
    if bytes.len() > 1_048_576 || bytes.contains(&0) {
        return Err("binary, non-UTF-8 or oversized");
    }
    String::from_utf8(bytes).map_err(|_| "binary, non-UTF-8 or oversized")
}

fn omit(
    payload: &mut Value,
    path: &RepoRelativePath,
    reason: &str,
    budget: usize,
    omitted: &mut usize,
) -> Result<(), RepoctlError> {
    *omitted = omitted.saturating_add(1);
    let mut trial = payload.clone();
    if let Some(entries) = trial["omitted"].as_array_mut() {
        entries.push(json!({"path":path,"reason":reason}));
    }
    if size(&trial)?.saturating_add(128) <= budget {
        *payload = trial;
    }
    Ok(())
}

fn size(value: &Value) -> Result<usize, RepoctlError> {
    // CLI JSON rendering is pretty-printed; account for those bytes too.
    serde_json::to_vec_pretty(value)
        .map(|bytes| bytes.len())
        .map_err(|error| RepoctlError::Environment(error.to_string()))
}

fn excluded(path: &str) -> bool {
    let normalized = path.to_ascii_lowercase();
    let path = normalized.as_str();
    path.split('/').any(|part| {
        part == "generated"
            || part == "gen"
            || part == "node_modules"
            || part == "target"
            || part == ".git"
            || part == ".repoctl"
            || part.starts_with(".env")
            || matches!(
                part,
                "credentials"
                    | "secrets"
                    | "secret"
                    | "id_rsa"
                    | "id_ed25519"
                    | ".netrc"
                    | ".npmrc"
                    | ".pypirc"
            )
            || part.starts_with("credentials.")
            || part.starts_with("secrets.")
    }) || Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pem") || ext.eq_ignore_ascii_case("key"))
        || path.ends_with(".pb.rs")
        || path.ends_with("_pb.py")
        || path.ends_with("_pb2.py")
        || path.ends_with("_pb2_grpc.py")
        || path.ends_with(".generated.ts")
}
