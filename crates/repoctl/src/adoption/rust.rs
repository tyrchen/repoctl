//! Cargo ownership shared by adoption workspace synthesis and full verification.

use camino::Utf8Path;
use repoctl_core::{
    AdoptedSource, ProcessCommand, ProjectManifestSynthesis, ProjectRelativePath, RepoRoot,
    RepoctlError, SourceInventory, WorkspaceLanguage, YamlManifestParser,
};
use repoctl_engine::LocalRustWorkspaceInspector;

use super::process_command;

pub(super) fn workspace_roots(
    source: &Utf8Path,
    inventory: &SourceInventory,
) -> Result<Vec<String>, RepoctlError> {
    let root = RepoRoot::new(source.to_path_buf()).map_err(RepoctlError::diagnostic)?;
    let manifests = inventory
        .manifests
        .iter()
        .filter(|path| path.ends_with("Cargo.toml"))
        .map(|path| ProjectRelativePath::new(path).map_err(RepoctlError::diagnostic))
        .collect::<Result<Vec<_>, _>>()?;
    LocalRustWorkspaceInspector
        .workspace_roots(&root, &manifests)
        .map(|roots| roots.into_iter().map(|root| root.to_string()).collect())
}

pub(super) fn append_verification(
    destination: &Utf8Path,
    source: &AdoptedSource,
    syntheses: &[ProjectManifestSynthesis],
    commands: &mut Vec<ProcessCommand>,
) -> Result<(), RepoctlError> {
    for synthesis in syntheses
        .iter()
        .filter(|synthesis| synthesis.source == source.name)
    {
        let project = YamlManifestParser.parse_project_bytes(
            synthesis.manifest_path.as_str(),
            synthesis.content.as_bytes(),
        )?;
        for workspace in project
            .workspaces
            .iter()
            .filter(|workspace| workspace.language == WorkspaceLanguage::Rust)
        {
            let cwd = source
                .destination_path
                .join_project(&workspace.root)
                .map_err(RepoctlError::diagnostic)?;
            let mut command = process_command(
                destination,
                &cwd,
                "cargo",
                ["check", "--workspace", "--all-features"],
            );
            if workspace.lockfile.is_some() {
                command.args.push("--locked".to_string());
            }
            commands.push(command);
        }
    }
    Ok(())
}
