//! Parsed package manifests and name indexes scoped to one graph construction.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use repoctl_core::{
    DiscoveredEdge, GraphBuildInput, ProjectManifest, RepoRelativePath, RepoRoot, RepoctlError,
    WorkspaceInspectionInput, WorkspaceLanguage,
};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::{inspect_indexed, manifest_error, normalize_python_name, parse_toml, toml_path};

#[derive(Debug, Default)]
pub(super) struct PackageIndex {
    pub(super) names: BTreeMap<String, RepoRelativePath>,
    pub(super) documents: BTreeMap<PathBuf, PackageManifest>,
}

#[derive(Debug)]
pub(super) enum PackageManifest {
    Node(Value),
    Python(DocumentMut),
}

impl PackageManifest {
    fn parse(text: &str, path: &Path, language: &WorkspaceLanguage) -> Result<Self, RepoctlError> {
        if *language == WorkspaceLanguage::TypeScript {
            serde_json::from_str(text)
                .map(Self::Node)
                .map_err(|error| manifest_error(path, &error.to_string()))
        } else {
            parse_toml(text, path).map(Self::Python)
        }
    }

    fn name(&self) -> Option<String> {
        match self {
            Self::Node(doc) => doc
                .get("name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            Self::Python(doc) => toml_path(doc.as_item(), &["project", "name"])
                .or_else(|| toml_path(doc.as_item(), &["tool", "poetry", "name"]))
                .and_then(Item::as_str)
                .map(normalize_python_name),
        }
    }
}

pub(super) fn package_index(
    root: &RepoRoot,
    projects: &[ProjectManifest],
    language: &WorkspaceLanguage,
    reader: &mut impl FnMut(&Path, &Path) -> Result<Option<String>, RepoctlError>,
) -> Result<PackageIndex, RepoctlError> {
    let mut index = PackageIndex::default();
    for project in projects {
        for workspace in project
            .workspaces
            .iter()
            .filter(|w| &w.language == language)
        {
            let rel = project
                .path
                .join_project(&workspace.manifest)
                .map_err(RepoctlError::diagnostic)?;
            let path = root.join(&rel).into_std_path_buf();
            let Some(text) = reader(&path, root.absolute.as_std_path())? else {
                continue;
            };
            let doc = PackageManifest::parse(&text, &path, language)?;
            if let Some(name) = doc.name() {
                let target = project
                    .path
                    .join_project(&workspace.root)
                    .map_err(RepoctlError::diagnostic)?;
                if index.names.insert(name.clone(), target).is_some() {
                    return Err(manifest_error(
                        &path,
                        &format!("ambiguous local package `{name}`"),
                    ));
                }
            }
            index.documents.insert(path, doc);
        }
    }
    Ok(index)
}

pub(super) fn inspect_packages(
    input: &GraphBuildInput,
    language: &WorkspaceLanguage,
    reader: &mut impl FnMut(&Path, &Path) -> Result<Option<String>, RepoctlError>,
) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
    let index = package_index(&input.root, &input.projects, language, reader)?;
    let mut edges = Vec::new();
    for project in &input.projects {
        for workspace in project
            .workspaces
            .iter()
            .filter(|w| &w.language == language)
        {
            edges.extend(inspect_indexed(
                &WorkspaceInspectionInput {
                    root: &input.root,
                    projects: &input.projects,
                    project,
                    workspace,
                },
                &index,
            )?);
        }
    }
    Ok(edges)
}

#[cfg(test)]
mod tests {
    // The discovery API is synchronous; real temporary manifests exercise its IO boundary.
    #![allow(clippy::disallowed_methods)]

    use std::{
        error::Error,
        fs,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use camino::Utf8PathBuf;
    use repoctl_core::{
        DiscoveredEdge, EdgeKind, GraphBuildInput, GraphBuilder, ManifestParser, ManifestSource,
        RepoRoot, RepoctlError, WorkspaceInspectionInput, WorkspaceInspector, WorkspaceLanguage,
        YamlManifestParser,
    };
    use tempfile::TempDir;

    use super::inspect_packages;
    use crate::{DefaultGraphBuilder, local_inspectors::read_manifest};

    #[derive(Debug)]
    struct CountingInspector {
        language: WorkspaceLanguage,
        reads: Arc<AtomicUsize>,
    }

    impl WorkspaceInspector for CountingInspector {
        fn language(&self) -> WorkspaceLanguage {
            self.language.clone()
        }

        fn inspect(
            &self,
            _: &WorkspaceInspectionInput<'_>,
        ) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
            Err(RepoctlError::Internal(
                "graph must use command-scoped batch inspection".to_string(),
            ))
        }

        fn inspect_all(
            &self,
            input: &GraphBuildInput,
        ) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
            inspect_packages(input, &self.language, &mut |path, root| {
                self.reads.fetch_add(1, Ordering::Relaxed);
                read_manifest(path, root)
            })
        }
    }

    #[test]
    fn test_should_read_each_package_once_per_graph_and_refresh_between_builds()
    -> Result<(), Box<dyn Error>> {
        for language in [WorkspaceLanguage::TypeScript, WorkspaceLanguage::Python] {
            let (temp, input) = package_fixture(&language)?;
            let reads = Arc::new(AtomicUsize::new(0));
            let builder = DefaultGraphBuilder::new(vec![Arc::new(CountingInspector {
                language: language.clone(),
                reads: Arc::clone(&reads),
            })]);
            let graph = builder.build(input.clone())?;
            assert_eq!(reads.load(Ordering::Relaxed), 32);
            assert_eq!(
                graph
                    .edges
                    .iter()
                    .filter(|e| e.kind == EdgeKind::DependsOnProject)
                    .count(),
                31
            );
            let (manifest, content) = package_content(&language, 31, false);
            fs::write(temp.path().join("tools/pkg31").join(manifest), content)?;
            let fresh = builder.build(input)?;
            assert_eq!(reads.load(Ordering::Relaxed), 64);
            assert_eq!(
                fresh
                    .edges
                    .iter()
                    .filter(|e| e.kind == EdgeKind::DependsOnProject)
                    .count(),
                30
            );
        }
        Ok(())
    }

    fn package_fixture(
        language: &WorkspaceLanguage,
    ) -> Result<(TempDir, GraphBuildInput), Box<dyn Error>> {
        let temp = tempfile::tempdir()?;
        let root = RepoRoot::new(Utf8PathBuf::try_from(fs::canonicalize(temp.path())?)?)
            .map_err(RepoctlError::diagnostic)?;
        let parser = YamlManifestParser;
        let repo_manifest = parser.parse_repo(ManifestSource::new(
            "repo.yaml",
            b"schema: company.repo/v1\nname: test\nlayout: functional\n".to_vec(),
        ))?;
        let mut projects = Vec::new();
        for index in 0..32 {
            let (manifest, content) = package_content(language, index, index > 0);
            let path = root.absolute.join(format!("tools/pkg{index}"));
            fs::create_dir_all(&path)?;
            fs::write(path.join(manifest), content)?;
            let language_name = if *language == WorkspaceLanguage::Python {
                "python"
            } else {
                "typescript"
            };
            let yaml = format!(
                "schema: company.project/v1\nname: tools.pkg{index}\nkind: tool\npath: \
                 tools/pkg{index}\nworkspaces:\n  - name: api\n    language: {language_name}\n    \
                 root: .\n    manifest: {manifest}\n"
            );
            projects.push(parser.parse_project(ManifestSource::new(
                format!("tools/pkg{index}/project.yaml"),
                yaml.into_bytes(),
            ))?);
        }
        Ok((
            temp,
            GraphBuildInput {
                root,
                repo_manifest,
                projects,
            },
        ))
    }

    fn package_content(
        language: &WorkspaceLanguage,
        index: usize,
        depends: bool,
    ) -> (&'static str, String) {
        if *language == WorkspaceLanguage::Python {
            let deps = if depends {
                format!("\"pkg{}>=1\"", index.saturating_sub(1))
            } else {
                String::new()
            };
            (
                "pyproject.toml",
                format!("[project]\nname = \"pkg{index}\"\ndependencies = [{deps}]\n"),
            )
        } else {
            let deps = if depends {
                format!("\"pkg{}\": \"workspace:*\"", index.saturating_sub(1))
            } else {
                String::new()
            };
            (
                "package.json",
                format!("{{\"name\":\"pkg{index}\",\"dependencies\":{{{deps}}}}}"),
            )
        }
    }
}
