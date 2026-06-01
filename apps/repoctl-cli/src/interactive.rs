use std::io::{self, IsTerminal};

use cliclack::{Confirm, Input, MultiSelect, Select, intro, outro};
use repoctl::{
    Diagnostic, IacProvider, OwnerHandle, ProjectKind, ProtoPackageName, RepoRelativePath,
    RepoctlError,
};

use super::{IacProviderArg, NewProjectArgs, OutputFormat};

const APP_ROOT: &str = "apps";
const FRAMEWORK_ROOT: &str = "frameworks";
const FOUNDATION_ROOT: &str = "foundations";
const KNOWN_PROJECT_ROOTS: [&str; 3] = [APP_ROOT, FRAMEWORK_ROOT, FOUNDATION_ROOT];

#[derive(Clone, Copy, Debug)]
pub(super) struct NewProjectPromptContext<'a> {
    pub(super) kind: &'a ProjectKind,
    pub(super) format: OutputFormat,
}

pub(super) trait InteractiveArgs: Sized {
    fn complete_interactively(
        self,
        context: NewProjectPromptContext<'_>,
    ) -> Result<Self, RepoctlError>;
}

impl InteractiveArgs for NewProjectArgs {
    fn complete_interactively(
        mut self,
        context: NewProjectPromptContext<'_>,
    ) -> Result<Self, RepoctlError> {
        if self.path.is_some() {
            return Ok(self);
        }
        if context.format != OutputFormat::Human {
            return Err(RepoctlError::diagnostic(missing_path_diagnostic(
                context.kind,
            )));
        }
        if !can_prompt() {
            return Err(RepoctlError::diagnostic(
                missing_path_diagnostic(context.kind).with_help(format!(
                    "pass a project name, for example `repoctl new {} {}`",
                    project_kind_command(context.kind),
                    example_slug(context.kind),
                )),
            ));
        }

        intro(format!("repoctl new {}", project_kind_label(context.kind))).map_err(prompt_error)?;
        self.path = Some(prompt_project_path(context.kind)?);
        match context.kind {
            ProjectKind::App => {
                if self.stack.is_empty() {
                    self.stack = prompt_app_stack()?;
                }
                if self.iac.is_none() {
                    self.iac = prompt_iac_provider()?;
                }
            }
            ProjectKind::Framework => {
                if self.languages.is_empty() {
                    self.languages = prompt_languages()?;
                }
                if !self.facade {
                    self.facade = prompt_framework_facade()?;
                }
            }
            ProjectKind::FoundationService => {
                if self.clients.is_empty() {
                    self.clients = prompt_clients()?;
                }
                if self.proto.is_none() {
                    self.proto = prompt_proto_package()?;
                }
                if self.iac.is_none() {
                    self.iac = prompt_iac_provider()?;
                }
            }
            ProjectKind::ProtoRoot | ProjectKind::CoreInfra => {}
        }
        if self.owner.is_none() {
            self.owner = prompt_owner()?;
        }
        outro("Project options collected.").map_err(prompt_error)?;
        Ok(self)
    }
}

pub(super) fn normalize_new_project_path(
    kind: &ProjectKind,
    path: Option<&str>,
) -> Result<RepoRelativePath, Diagnostic> {
    let raw = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| missing_path_diagnostic(kind))?;
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        return Err(missing_path_diagnostic(kind));
    }
    let expected_root = project_root(kind)?;
    if trimmed == expected_root {
        return Err(Diagnostic::error(
            "new.path.missing_slug",
            format!("project path must include a name below `{expected_root}/`"),
        )
        .with_help(format!(
            "use `{expected_root}/{}` or just `{}`",
            example_slug(kind),
            example_slug(kind),
        )));
    }
    let first_segment = trimmed.split('/').next().unwrap_or_default();
    let normalized = if KNOWN_PROJECT_ROOTS.contains(&first_segment) {
        if first_segment == expected_root {
            trimmed.to_string()
        } else {
            return Err(Diagnostic::error(
                "new.path.kind_mismatch",
                format!(
                    "`{}` creates {} projects, so the path must live under `{expected_root}/`",
                    project_kind_command(kind),
                    project_kind_label(kind),
                ),
            )
            .with_path(trimmed)
            .with_help(format!(
                "use `{expected_root}/{}` or run `repoctl new {first_segment_singular} {trimmed}`",
                trimmed
                    .rsplit('/')
                    .next()
                    .unwrap_or_else(|| example_slug(kind)),
                first_segment_singular = root_to_command(first_segment),
            )));
        }
    } else {
        format!("{expected_root}/{trimmed}")
    };
    RepoRelativePath::new(normalized)
}

fn can_prompt() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

fn prompt_project_path(kind: &ProjectKind) -> Result<String, RepoctlError> {
    let expected_root = project_root(kind).map_err(RepoctlError::diagnostic)?;
    let mut prompt = Input::new(format!("{} name", project_kind_label(kind)))
        .placeholder(example_slug(kind))
        .validate({
            let kind = kind.clone();
            move |value: &String| {
                normalize_new_project_path(&kind, Some(value))
                    .map(|_| ())
                    .map_err(|diagnostic| diagnostic.message.to_string())
            }
        });
    let value: String = prompt.interact().map_err(prompt_error)?;
    let normalized = normalize_new_project_path(kind, Some(&value))
        .map_err(RepoctlError::diagnostic)?
        .as_str()
        .to_string();
    let display = normalized
        .strip_prefix(&format!("{expected_root}/"))
        .unwrap_or(normalized.as_str());
    Ok(display.to_string())
}

fn prompt_app_stack() -> Result<Vec<String>, RepoctlError> {
    let mut prompt = MultiSelect::new("App stack")
        .item(
            "rust-api".to_string(),
            "Rust API",
            "service crate with Cargo tasks",
        )
        .item("bun-web".to_string(), "Bun web", "TypeScript web workspace")
        .item("uv-jobs".to_string(), "uv jobs", "Python jobs workspace")
        .initial_values(vec!["rust-api".to_string()]);
    prompt.interact().map_err(prompt_error)
}

fn prompt_languages() -> Result<Vec<String>, RepoctlError> {
    let mut prompt = MultiSelect::new("Framework languages")
        .item("rust".to_string(), "Rust", "facade and internal crates")
        .item(
            "typescript".to_string(),
            "TypeScript",
            "facade and internal packages",
        )
        .initial_values(vec!["rust".to_string()]);
    prompt.interact().map_err(prompt_error)
}

fn prompt_clients() -> Result<Vec<String>, RepoctlError> {
    let mut prompt = MultiSelect::new("Foundation clients")
        .item("rust".to_string(), "Rust", "Cargo client crate")
        .item("typescript".to_string(), "TypeScript", "Bun client package")
        .item("python".to_string(), "Python", "uv client package")
        .initial_values(vec!["rust".to_string()]);
    prompt.interact().map_err(prompt_error)
}

fn prompt_framework_facade() -> Result<bool, RepoctlError> {
    let mut prompt = Confirm::new("Include public facade and internal implementation areas?")
        .initial_value(true);
    prompt.interact().map_err(prompt_error)
}

fn prompt_iac_provider() -> Result<Option<IacProviderArg>, RepoctlError> {
    let mut prompt = Select::new("IaC provider")
        .item(None, "None", "skip infrastructure scaffold")
        .item(
            Some(IacProvider::Pulumi),
            "Pulumi",
            "generate Pulumi stacks",
        )
        .item(
            Some(IacProvider::Terraform),
            "Terraform",
            "generate Terraform stacks",
        )
        .item(
            Some(IacProvider::OpenTofu),
            "OpenTofu",
            "generate OpenTofu stacks",
        )
        .initial_value(None);
    let value = prompt.interact().map_err(prompt_error)?;
    Ok(value.map(|provider| match provider {
        IacProvider::Pulumi => IacProviderArg::Pulumi,
        IacProvider::Terraform => IacProviderArg::Terraform,
        IacProvider::OpenTofu => IacProviderArg::Opentofu,
    }))
}

fn prompt_proto_package() -> Result<Option<String>, RepoctlError> {
    let mut prompt = Input::new("Owned proto package")
        .placeholder("company.identity.v1")
        .required(false)
        .validate(|value: &String| {
            if value.trim().is_empty() {
                Ok(())
            } else {
                ProtoPackageName::new(value.trim().to_string())
                    .map(|_| ())
                    .map_err(|diagnostic| diagnostic.message.to_string())
            }
        });
    let value: String = prompt.interact().map_err(prompt_error)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn prompt_owner() -> Result<Option<String>, RepoctlError> {
    let mut prompt = Input::new("Owner handle")
        .placeholder("@platform")
        .required(false)
        .validate(|value: &String| {
            if value.trim().is_empty() {
                Ok(())
            } else {
                OwnerHandle::new(value.trim().to_string())
                    .map(|_| ())
                    .map_err(|diagnostic| diagnostic.message.to_string())
            }
        });
    let value: String = prompt.interact().map_err(prompt_error)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn project_root(kind: &ProjectKind) -> Result<&'static str, Diagnostic> {
    match kind {
        ProjectKind::App => Ok(APP_ROOT),
        ProjectKind::Framework => Ok(FRAMEWORK_ROOT),
        ProjectKind::FoundationService => Ok(FOUNDATION_ROOT),
        ProjectKind::ProtoRoot | ProjectKind::CoreInfra => Err(Diagnostic::error(
            "new.kind.unsupported",
            "new project supports app, framework, and foundation-service",
        )),
    }
}

fn project_kind_command(kind: &ProjectKind) -> &'static str {
    match kind {
        ProjectKind::App => "app",
        ProjectKind::Framework => "framework",
        ProjectKind::FoundationService => "foundation",
        ProjectKind::ProtoRoot | ProjectKind::CoreInfra => "project",
    }
}

fn project_kind_label(kind: &ProjectKind) -> &'static str {
    match kind {
        ProjectKind::App => "app",
        ProjectKind::Framework => "framework",
        ProjectKind::FoundationService => "foundation service",
        ProjectKind::ProtoRoot => "proto root",
        ProjectKind::CoreInfra => "core infrastructure",
    }
}

fn example_slug(kind: &ProjectKind) -> &'static str {
    match kind {
        ProjectKind::App => "catalog",
        ProjectKind::Framework => "core",
        ProjectKind::FoundationService => "identity",
        ProjectKind::ProtoRoot => "protos",
        ProjectKind::CoreInfra => "core-infra",
    }
}

fn root_to_command(root: &str) -> &'static str {
    match root {
        APP_ROOT => "app",
        FRAMEWORK_ROOT => "framework",
        FOUNDATION_ROOT => "foundation",
        _ => "project",
    }
}

fn missing_path_diagnostic(kind: &ProjectKind) -> Diagnostic {
    Diagnostic::error(
        "new.path.required",
        format!("missing {} name or path", project_kind_label(kind)),
    )
}

fn prompt_error(error: impl std::fmt::Display) -> RepoctlError {
    RepoctlError::diagnostic(Diagnostic::error(
        "cli.prompt.failed",
        format!("interactive prompt failed: {error}"),
    ))
}
