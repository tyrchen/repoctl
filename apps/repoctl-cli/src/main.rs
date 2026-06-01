#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::missing_errors_doc)]
// The CLI dispatcher intentionally keeps command routing in one place.
#![allow(clippy::too_many_lines)]

//! repoctl command-line frontend.

use std::{
    fmt::Write as FmtWrite,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand, ValueEnum};
use repoctl::{
    AffectedReport, AffectedRequest, BoundaryLintRequest, CiMatrixReport, CiMatrixRequest,
    Diagnostic, ExplainReport, ExplainRequest, GraphPrintReport, GraphPrintRequest,
    GraphValidateRequest, IacProvider, InitPlan, InitProfile, InitRequest, NewProjectRequest,
    OwnerHandle, ProjectKind, ProjectName, ProtoPackageName, RenderPlan, RepoLayout, RepoName,
    RepoRelativePath, Repoctl, RepoctlError, Severity, SkillsFacadeRequest, TaskName,
    TaskRunReport, TaskRunRequest, ValidationReport,
};

#[derive(Debug, Parser)]
#[command(name = "repoctl", version, about = "Monorepo control plane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize a functional monorepo.
    Init {
        /// Repository name.
        #[arg(long)]
        name: String,
        /// Initialization profile.
        #[arg(long, value_enum, default_value_t = InitProfileArg::Startup)]
        profile: InitProfileArg,
        /// Repository layout.
        #[arg(long, value_enum, default_value_t = LayoutArg::Functional)]
        layout: LayoutArg,
        /// Target repository root.
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Plan without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Create a project from built-in templates.
    New {
        #[command(subcommand)]
        command: NewCommand,
    },
    /// Inspect and validate the repository graph.
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },
    /// Explain a project or graph node.
    Explain {
        /// Project id or graph node id.
        selector: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Run boundary policy checks.
    LintBoundaries {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Changed file used for path-based policies.
        #[arg(long = "changed-file")]
        changed_files: Vec<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Compute affected projects and tasks.
    Affected {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Base git ref.
        #[arg(long)]
        base: Option<String>,
        /// Head git ref.
        #[arg(long)]
        head: Option<String>,
        /// Explicit changed file.
        #[arg(long = "changed-file")]
        changed_files: Vec<String>,
        /// Comma-separated task names.
        #[arg(long, value_delimiter = ',')]
        tasks: Vec<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Run repo tasks.
    Run {
        /// Task name.
        task: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Project selector.
        #[arg(long = "project")]
        projects: Vec<String>,
        /// Run only affected projects.
        #[arg(long)]
        affected: bool,
        /// Explicit changed file.
        #[arg(long = "changed-file")]
        changed_files: Vec<String>,
        /// Base git ref.
        #[arg(long)]
        base: Option<String>,
        /// Head git ref.
        #[arg(long)]
        head: Option<String>,
        /// Plan without running commands.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// CI helpers.
    Ci {
        #[command(subcommand)]
        command: CiCommand,
    },
    /// Skills helpers.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum NewCommand {
    /// Create an app project.
    App(NewProjectArgs),
    /// Create a framework project.
    Framework(NewProjectArgs),
    /// Create a foundation service project.
    Foundation(NewProjectArgs),
}

#[derive(Clone, Debug, Parser)]
struct NewProjectArgs {
    /// Project path, for example apps/catalog.
    path: String,
    /// Repository root or path inside the repo.
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Comma-separated stack entries.
    #[arg(long, value_delimiter = ',')]
    stack: Vec<String>,
    /// Comma-separated languages.
    #[arg(long, value_delimiter = ',')]
    languages: Vec<String>,
    /// Comma-separated clients.
    #[arg(long, value_delimiter = ',')]
    clients: Vec<String>,
    /// Foundation service language.
    #[arg(long = "service")]
    _service: Option<String>,
    /// Include framework facade and internal areas.
    #[arg(long, default_value_t = false, num_args = 0..=1, default_missing_value = "true")]
    facade: bool,
    /// `IaC` provider.
    #[arg(long, value_enum)]
    iac: Option<IacProviderArg>,
    /// Proto package.
    #[arg(long)]
    proto: Option<String>,
    /// Owner handle.
    #[arg(long)]
    owner: Option<String>,
    /// Plan without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
    format: OutputFormat,
}

#[derive(Debug, Subcommand)]
enum CiCommand {
    /// Build CI matrix.
    Matrix {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Comma-separated task names.
        #[arg(long, value_delimiter = ',')]
        tasks: Vec<String>,
        /// Explicit changed file.
        #[arg(long = "changed-file")]
        changed_files: Vec<String>,
        /// Base git ref.
        #[arg(long)]
        base: Option<String>,
        /// Head git ref.
        #[arg(long)]
        head: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
    },
    /// Summarize CI matrix.
    Summarize {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum SkillsCommand {
    /// Check generated skills.
    Check {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Synchronize generated skills.
    Sync {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum GraphCommand {
    /// Validate graph and boundary policies.
    Validate {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Changed file used for path-based policies.
        #[arg(long = "changed-file")]
        changed_files: Vec<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Print the graph.
    Print {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
    GithubActions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum InitProfileArg {
    Startup,
    Enterprise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LayoutArg {
    Functional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum IacProviderArg {
    Pulumi,
    Terraform,
    Opentofu,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => render_error(&error),
    }
}

fn run() -> Result<ExitCode, RepoctlError> {
    let cli = Cli::parse();
    let repoctl = Repoctl::with_default_adapters()?;
    match cli.command {
        Command::Init {
            name,
            profile,
            layout,
            repo,
            dry_run,
            format,
        } => {
            let plan = repoctl.init(InitRequest {
                repo_root: repo,
                name: RepoName::new(name).map_err(RepoctlError::diagnostic)?,
                profile: profile.into(),
                layout: layout.into(),
                dry_run,
            })?;
            render_init_plan(&plan, format)?;
            Ok(exit_for_diagnostics(&plan.warnings))
        }
        Command::New { command } => {
            let (kind, args) = match command {
                NewCommand::App(args) => (ProjectKind::App, args),
                NewCommand::Framework(args) => (ProjectKind::Framework, args),
                NewCommand::Foundation(args) => (ProjectKind::FoundationService, args),
            };
            let format = args.format;
            let plan = repoctl.new_project(NewProjectRequest {
                repo: args.repo,
                kind,
                path: RepoRelativePath::new(args.path).map_err(RepoctlError::diagnostic)?,
                stack: args.stack,
                languages: args.languages,
                clients: args.clients,
                facade: args.facade,
                iac: args.iac.map(Into::into),
                proto: args
                    .proto
                    .map(ProtoPackageName::new)
                    .transpose()
                    .map_err(RepoctlError::diagnostic)?,
                owner: args
                    .owner
                    .map(OwnerHandle::new)
                    .transpose()
                    .map_err(RepoctlError::diagnostic)?,
                dry_run: args.dry_run,
            })?;
            render_render_plan(&plan, format)?;
            Ok(exit_for_diagnostics(&plan.diagnostics))
        }
        Command::Graph { command } => match command {
            GraphCommand::Validate {
                repo,
                changed_files,
                format,
            } => {
                let changed_files = parse_changed_files(changed_files)?;
                let report = repoctl.validate_graph(GraphValidateRequest {
                    repo,
                    changed_files,
                })?;
                render_validation_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            GraphCommand::Print { repo, format } => {
                let report = repoctl.graph_print(GraphPrintRequest { repo })?;
                render_graph_print(&report, format)?;
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Explain {
            selector,
            repo,
            format,
        } => {
            let report = repoctl.explain(ExplainRequest { repo, selector })?;
            render_explain(&report, format)?;
            Ok(exit_for_diagnostics(&report.diagnostics))
        }
        Command::LintBoundaries {
            repo,
            changed_files,
            format,
        } => {
            let changed_files = parse_changed_files(changed_files)?;
            let report = repoctl.lint_boundaries(BoundaryLintRequest {
                repo,
                changed_files,
            })?;
            let validation = ValidationReport::new(report.diagnostics);
            render_validation_report(&validation, format)?;
            Ok(exit_for_diagnostics(&validation.diagnostics))
        }
        Command::Affected {
            repo,
            base,
            head,
            changed_files,
            tasks,
            format,
        } => {
            let report = repoctl.affected(AffectedRequest {
                repo,
                base,
                head,
                changed_files: parse_changed_files(changed_files)?,
                tasks: parse_tasks(tasks)?,
            })?;
            render_affected(&report, format)?;
            Ok(exit_for_diagnostics(&report.diagnostics))
        }
        Command::Run {
            task,
            repo,
            projects,
            affected,
            changed_files,
            base,
            head,
            dry_run,
            format,
        } => {
            let report = repoctl.run_task(TaskRunRequest {
                repo,
                tasks: vec![TaskName::new(task).map_err(RepoctlError::diagnostic)?],
                projects: parse_projects(projects)?,
                affected,
                changed_files: parse_changed_files(changed_files)?,
                base,
                head,
                concurrency: None,
                dry_run,
            })?;
            render_task_report(&report, format)?;
            Ok(exit_for_diagnostics(&report.diagnostics))
        }
        Command::Ci { command } => match command {
            CiCommand::Matrix {
                repo,
                tasks,
                changed_files,
                base,
                head,
                format,
            } => {
                let report = repoctl.ci_matrix(CiMatrixRequest {
                    repo,
                    tasks: parse_tasks(tasks)?,
                    changed_files: parse_changed_files(changed_files)?,
                    base,
                    head,
                })?;
                render_ci_matrix(&report, format)?;
                Ok(ExitCode::SUCCESS)
            }
            CiCommand::Summarize { repo, format } => {
                let report = repoctl.ci_matrix(CiMatrixRequest {
                    repo,
                    tasks: Vec::new(),
                    changed_files: Vec::new(),
                    base: None,
                    head: None,
                })?;
                render_ci_matrix(&report, format)?;
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Skills { command } => match command {
            SkillsCommand::Check { repo, format } => {
                let report = repoctl.skills().check(SkillsFacadeRequest {
                    repo,
                    sync: false,
                    dry_run: false,
                })?;
                let validation = ValidationReport::new(report.diagnostics);
                render_validation_report(&validation, format)?;
                Ok(exit_for_diagnostics(&validation.diagnostics))
            }
            SkillsCommand::Sync {
                repo,
                dry_run,
                format,
            } => {
                let report = repoctl.skills().sync(SkillsFacadeRequest {
                    repo,
                    sync: true,
                    dry_run,
                })?;
                let validation = ValidationReport::new(report.diagnostics);
                render_validation_report(&validation, format)?;
                Ok(exit_for_diagnostics(&validation.diagnostics))
            }
        },
    }
}

impl From<InitProfileArg> for InitProfile {
    fn from(value: InitProfileArg) -> Self {
        match value {
            InitProfileArg::Startup => Self::Startup,
            InitProfileArg::Enterprise => Self::Enterprise,
        }
    }
}

impl From<LayoutArg> for RepoLayout {
    fn from(value: LayoutArg) -> Self {
        match value {
            LayoutArg::Functional => Self::Functional,
        }
    }
}

impl From<IacProviderArg> for IacProvider {
    fn from(value: IacProviderArg) -> Self {
        match value {
            IacProviderArg::Pulumi => Self::Pulumi,
            IacProviderArg::Terraform => Self::Terraform,
            IacProviderArg::Opentofu => Self::OpenTofu,
        }
    }
}

fn parse_changed_files(values: Vec<String>) -> Result<Vec<RepoRelativePath>, RepoctlError> {
    values
        .into_iter()
        .map(|value| {
            RepoRelativePath::new(value.clone()).map_err(|diagnostic| {
                RepoctlError::diagnostic(
                    diagnostic
                        .with_path(value)
                        .with_help("changed files must be repo-relative paths without traversal"),
                )
            })
        })
        .collect()
}

fn parse_tasks(values: Vec<String>) -> Result<Vec<TaskName>, RepoctlError> {
    values
        .into_iter()
        .map(|value| TaskName::new(value).map_err(RepoctlError::diagnostic))
        .collect()
}

fn parse_projects(values: Vec<String>) -> Result<Vec<ProjectName>, RepoctlError> {
    values
        .into_iter()
        .map(|value| ProjectName::new(value).map_err(RepoctlError::diagnostic))
        .collect()
}

fn render_init_plan(plan: &InitPlan, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(plan),
        OutputFormat::Human => {
            let mut output = String::new();
            for operation in &plan.operations {
                let _ = writeln!(&mut output, "{} {}", operation.operation, operation.path);
            }
            render_optional_diagnostics(&mut output, &plan.warnings);
            write_stdout(&output)
        }
    }
}

fn render_render_plan(plan: &RenderPlan, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(plan),
        OutputFormat::Human => {
            let mut output = String::new();
            for operation in &plan.operations {
                let _ = writeln!(&mut output, "{} {}", operation.operation, operation.path);
            }
            render_optional_diagnostics(&mut output, &plan.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_validation_report(
    report: &ValidationReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            if report.diagnostics.is_empty() {
                write_stdout("OK: graph validation passed\n")
            } else {
                render_diagnostics(&report.diagnostics)
            }
        }
    }
}

fn render_graph_print(report: &GraphPrintReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            output.push_str("Nodes:\n");
            for node in &report.snapshot.graph.nodes {
                let _ = writeln!(&mut output, "  {} [{}]", node.id, node.label);
            }
            output.push_str("Edges:\n");
            for edge in &report.snapshot.graph.edges {
                let _ = writeln!(
                    &mut output,
                    "  {} -> {} ({:?})",
                    edge.from, edge.to, edge.kind
                );
            }
            write_stdout(&output)
        }
    }
}

fn render_explain(report: &ExplainReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            if !report.diagnostics.is_empty() {
                return render_diagnostics(&report.diagnostics);
            }
            let mut output = String::new();
            let _ = writeln!(&mut output, "Selector: {}", report.selector);
            output.push_str("Nodes:\n");
            for node in &report.nodes {
                let _ = writeln!(&mut output, "  {} [{}]", node.id, node.label);
            }
            output.push_str("Edges:\n");
            for edge in &report.edges {
                let _ = writeln!(
                    &mut output,
                    "  {} -> {} ({:?})",
                    edge.from, edge.to, edge.kind
                );
            }
            write_stdout(&output)
        }
    }
}

fn render_affected(report: &AffectedReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            output.push_str("Direct:\n");
            for project in &report.directly_affected {
                let _ = writeln!(&mut output, "  {project}");
            }
            output.push_str("Transitive:\n");
            for project in &report.transitively_affected {
                let _ = writeln!(&mut output, "  {project}");
            }
            output.push_str("Reasons:\n");
            for reason in &report.reasons {
                let _ = writeln!(
                    &mut output,
                    "  {} -> {}: {}",
                    reason.source, reason.target, reason.reason
                );
            }
            render_optional_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_task_report(report: &TaskRunReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            for command in &report.commands {
                let _ = writeln!(
                    &mut output,
                    "{} {}",
                    command.program,
                    command.args.join(" ")
                );
            }
            for output_item in &report.outputs {
                let _ = writeln!(
                    &mut output,
                    "{}:{}:{} status {}",
                    output_item.project,
                    output_item.workspace,
                    output_item.task,
                    output_item.output.status
                );
            }
            render_optional_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_ci_matrix(report: &CiMatrixReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::GithubActions => write_json(&report.github_actions),
        OutputFormat::Json => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            for entry in &report.entries {
                let _ = writeln!(&mut output, "{entry}");
            }
            write_stdout(&output)
        }
    }
}

fn render_optional_diagnostics(output: &mut String, diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        let _ = writeln!(
            output,
            "{} [{:?}]: {}",
            diagnostic.code, diagnostic.severity, diagnostic.message
        );
    }
}

fn render_diagnostics(diagnostics: &[Diagnostic]) -> Result<(), RepoctlError> {
    let mut output = String::new();
    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        let _ = writeln!(
            &mut output,
            "{} [{}]: {}",
            diagnostic.code, severity, diagnostic.message
        );
        if let Some(source) = &diagnostic.source {
            let _ = writeln!(&mut output, "  at {}", source.path);
        }
        if let Some(help) = &diagnostic.help {
            let _ = writeln!(&mut output, "  help: {help}");
        }
    }
    write_stdout(&output)
}

fn write_json<T: serde::Serialize>(value: &T) -> Result<(), RepoctlError> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)
        .map_err(|error| RepoctlError::Internal(format!("failed to render JSON: {error}")))?;
    stdout
        .write_all(b"\n")
        .map_err(|source| RepoctlError::io("<stdout>", source))
}

fn write_stdout(value: &str) -> Result<(), RepoctlError> {
    io::stdout()
        .lock()
        .write_all(value.as_bytes())
        .map_err(|source| RepoctlError::io("<stdout>", source))
}

fn render_error(error: &RepoctlError) -> ExitCode {
    let diagnostics = error.diagnostics();
    let _ = render_stderr(&diagnostics);
    match error {
        RepoctlError::Internal(_) => ExitCode::from(4),
        RepoctlError::Environment(_) | RepoctlError::Io { .. } => ExitCode::from(3),
        RepoctlError::Diagnostic { .. } | RepoctlError::Diagnostics { .. } => ExitCode::from(1),
    }
}

fn render_stderr(diagnostics: &[Diagnostic]) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    for diagnostic in diagnostics {
        writeln!(
            stderr,
            "{} [{:?}]: {}",
            diagnostic.code, diagnostic.severity, diagnostic.message
        )?;
        if let Some(source) = &diagnostic.source {
            writeln!(stderr, "  at {}", source.path)?;
        }
        if let Some(help) = &diagnostic.help {
            writeln!(stderr, "  help: {help}")?;
        }
    }
    Ok(())
}

fn exit_for_diagnostics(diagnostics: &[Diagnostic]) -> ExitCode {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
