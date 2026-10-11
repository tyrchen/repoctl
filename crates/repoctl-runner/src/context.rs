//! Explicit, project-scoped source snippets within a serialized byte budget.

use std::{
    collections::BTreeSet,
    fs,
    io::{BufReader, Read},
    path::Path,
};

use globset::{Glob, GlobSetBuilder};
use ignore::WalkBuilder;
use repoctl_core::{
    AiContextRequest, Diagnostic, ProjectManifest, RepoRelativePath, RepoSnapshot, RepoctlError,
};
use serde_json::{Value, json};

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
        for doc in &project.ai.docs {
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
                for entry in WalkBuilder::new(&absolute)
                    .hidden(false)
                    .follow_links(false)
                    .max_depth(Some(32))
                    .filter_entry(|entry| !excluded(entry.path().to_string_lossy().as_ref()))
                    .build()
                {
                    let entry =
                        entry.map_err(|error| RepoctlError::Environment(error.to_string()))?;
                    if entry
                        .file_type()
                        .is_some_and(|kind| kind.is_file() || kind.is_symlink())
                    {
                        let relative = entry
                            .path()
                            .strip_prefix(snapshot.root.absolute.as_std_path())
                            .map_err(|error| RepoctlError::Environment(error.to_string()))?;
                        paths.insert(
                            RepoRelativePath::new(relative.to_string_lossy().replace('\\', "/"))
                                .map_err(RepoctlError::diagnostic)?,
                        );
                    }
                    if paths.len() > 512 {
                        return Err(RepoctlError::Environment(
                            "context selection exceeds 512 files".to_string(),
                        ));
                    }
                }
            } else {
                paths.insert(path);
            }
        }
    }
    Ok(paths)
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
