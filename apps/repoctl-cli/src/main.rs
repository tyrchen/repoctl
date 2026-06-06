#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::missing_errors_doc)]
// The CLI dispatcher intentionally keeps command routing in one place.
#![allow(clippy::too_many_lines)]

//! repoctl command-line frontend.

use std::{
    collections::BTreeMap,
    fmt::Write as FmtWrite,
    io::{self, Write},
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand, ValueEnum};
use repoctl::{
    AdoptionApplyRequest, AdoptionCiMode, AdoptionOutputFormat, AdoptionPlan, AdoptionPlanRequest,
    AdoptionVerifyRequest, AffectedReport, AffectedRequest, AiContext, AiContextRequest,
    BoundaryLintRequest, CiFallback, CiMatrixReport, CiMatrixRequest, CiProvider, CiWorkflowReport,
    CiWorkflowRequest, CodegenCheckReport, CodegenCheckRequest, DependencyRewriteMode, Diagnostic,
    ExplainReport, ExplainRequest, FileOperation, GraphPrintReport, GraphPrintRequest,
    GraphValidateRequest, HygieneCheckRequest, HygieneCleanRequest, HygieneReport, IacFacadeReport,
    IacFacadeRequest, IacProvider, InitPlan, InitProfile, InitRequest, NewProjectRequest,
    OpsJournalAction, OpsJournalReport, OpsJournalRequest, OpsPlan, OpsPlanRequest,
    OpsReconcileReport, OpsReconcileRequest, OpsVerifyReport, OpsVerifyRequest, OwnerHandle,
    PrSummary, PrSummaryRequest, ProcessCommand, ProjectKind, ProjectName, ProtoFacadeReport,
    ProtoFacadeRequest, ProtoOperation, ProtoPackageName, ProviderCapabilityReport,
    ProviderCapabilityRequest, RenderPlan, RepoLayout, RepoName, RepoRelativePath, Repoctl,
    RepoctlError, Severity, SkillsFacadeRequest, TaskName, TaskRunReport, TaskRunRequest,
    TemplateListReport, TemplateListRequest, TemplateRenderRequest, TemplateSource, ValidationMode,
    ValidationReport, WorkspaceName,
};

mod interactive;

use interactive::{InteractiveArgs, NewProjectPromptContext, normalize_new_project_path};

#[derive(Debug, Parser)]
#[command(
    name = "repoctl",
    version,
    about = "Manage graph-aware monorepos",
    long_about = "repoctl manages graph-aware monorepos: scaffolding, validation, affected-task \
                  analysis, CI matrices, templates, skills, proto ownership, and IaC planning."
)]
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
        /// Explicit repository root. Defaults to a new directory named by --name.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Scaffold a project from built-in conventions.
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
    /// Check boundary policies.
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
    /// Plan or run repository tasks.
    Run {
        /// Task name.
        task: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Project selector.
        #[arg(long = "project")]
        projects: Vec<String>,
        /// Workspace selector in project:workspace form.
        #[arg(long = "workspace")]
        workspaces: Vec<String>,
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
        /// Maximum task concurrency.
        #[arg(long)]
        concurrency: Option<NonZeroU32>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Generate CI data.
    Ci {
        #[command(subcommand)]
        command: CiCommand,
    },
    /// Adopt existing repositories into a functional monorepo.
    Adopt {
        #[command(subcommand)]
        command: AdoptCommand,
    },
    /// Check and clean generated artifact leakage.
    Hygiene {
        #[command(subcommand)]
        command: HygieneCommand,
    },
    /// List and render templates.
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    /// Check generated-code policy.
    Codegen {
        #[command(subcommand)]
        command: CodegenCommand,
    },
    /// Proto ownership and compatibility helpers.
    Proto {
        #[command(subcommand)]
        command: ProtoCommand,
    },
    /// Build AI agent context.
    Context {
        /// Project id.
        project: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Context audience.
        #[arg(long = "for", default_value = "ai")]
        for_target: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
        format: OutputFormat,
    },
    /// Build pull request summaries.
    Pr {
        #[command(subcommand)]
        command: PrCommand,
    },
    /// Plan infrastructure commands.
    Iac {
        #[command(subcommand)]
        command: IacCommand,
    },
    /// Plan and verify operational infrastructure sessions.
    Ops {
        #[command(subcommand)]
        command: OpsCommand,
    },
    /// Inspect provider package capabilities.
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// Check and synchronize generated skills.
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
    /// Create a tool project.
    Tool(NewProjectArgs),
}

#[derive(Clone, Debug, Parser)]
struct NewProjectArgs {
    /// Project name or path. Names are placed under the selected project root.
    path: Option<String>,
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
    /// Render a maintained workflow.
    Workflow {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// CI provider.
        #[arg(long, value_enum, default_value_t = CiProviderArg::GithubActions)]
        provider: CiProviderArg,
        /// Write the workflow file.
        #[arg(long)]
        write: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
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
        /// Fallback when affected-file detection cannot select entries.
        #[arg(long, value_enum, default_value_t = CiFallbackArg::None)]
        fallback: CiFallbackArg,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Execute one CI matrix step locally.
    RunStep {
        /// Task name.
        #[arg(long)]
        task: String,
        /// Project selector.
        #[arg(long)]
        project: String,
        /// Workspace selector.
        #[arg(long)]
        workspace: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan without running commands.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
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
enum AdoptCommand {
    /// Inspect sources and render a reviewed adoption plan.
    Plan {
        /// Directory containing source repos or a single source repo.
        #[arg(long)]
        source: PathBuf,
        /// Destination initialized functional monorepo.
        #[arg(long)]
        dest: PathBuf,
        /// Source repo names to include.
        #[arg(long)]
        include: Vec<String>,
        /// Source repo names to exclude.
        #[arg(long)]
        exclude: Vec<String>,
        /// Placement override SOURCE=DEST.
        #[arg(long = "map")]
        map: Vec<String>,
        /// Kind override SOURCE=KIND.
        #[arg(long = "kind")]
        kind: Vec<String>,
        /// Owner override SOURCE=@owner.
        #[arg(long = "owner")]
        owner: Vec<String>,
        /// Dependency rewrite mode.
        #[arg(long, value_enum, default_value_t = DependencyRewriteArg::Auto)]
        rewrite_deps: DependencyRewriteArg,
        /// CI behavior.
        #[arg(long, value_enum, default_value_t = AdoptionCiArg::Update)]
        ci: AdoptionCiArg,
        /// Verification depth.
        #[arg(long, value_enum, default_value_t = ValidationModeArg::Metadata)]
        verification: ValidationModeArg,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Apply a reviewed adoption plan.
    Apply {
        /// Plan JSON file.
        #[arg(long)]
        plan: PathBuf,
        /// Refuse stale inference by requiring a fresh plan first.
        #[arg(long)]
        refresh: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Verify a reviewed adoption plan without copying.
    Verify {
        /// Plan JSON file.
        #[arg(long)]
        plan: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum HygieneCommand {
    /// Check for generated artifacts and nested repository metadata.
    Check {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Plan or clean generated artifacts.
    Clean {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan without deleting generated artifacts.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum TemplateCommand {
    /// List available templates.
    List {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Render a template source.
    Render {
        /// Template source, for example builtin:app or local:templates/app.
        source: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Input as key=value. Repeat as needed.
        #[arg(long = "input")]
        inputs: Vec<String>,
        /// Plan without writing files.
        #[arg(long)]
        dry_run: bool,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum CodegenCommand {
    /// Check generated-code direct edits.
    Check {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
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
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum ProtoCommand {
    /// Find owner projects for a proto path.
    Owners {
        /// Proto source path or package name.
        selector: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Find consumer projects for a proto path or package.
    Consumers {
        /// Proto source path or package name.
        selector: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Check proto toolchain and generated-code policy.
    Check {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
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
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum PrCommand {
    /// Build a PR summary.
    Summary {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
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
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum IacCommand {
    /// Plan provider commands.
    #[command(alias = "preview")]
    Plan {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan affected projects only.
        #[arg(long)]
        affected: bool,
        /// Project selector.
        #[arg(long)]
        project: Option<String>,
        /// Environment or stack.
        #[arg(long)]
        env: Option<String>,
        /// Plan core infrastructure.
        #[arg(long)]
        core: bool,
        /// Plan without executing provider commands.
        #[arg(long)]
        dry_run: bool,
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
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum OpsCommand {
    /// Build an ordered operations plan.
    Plan {
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
        /// Target environment or stack.
        #[arg(long = "env")]
        environments: Vec<String>,
        /// Comma-separated task names.
        #[arg(long, value_delimiter = ',')]
        tasks: Vec<String>,
        /// Write JSON plan to this path.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Plan non-mutating verification commands.
    Verify {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan JSON path.
        #[arg(long)]
        plan: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Manage a local operations session journal.
    Journal {
        #[command(subcommand)]
        command: OpsJournalCommand,
    },
    /// Review manual state recorded in a plan.
    Reconcile {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Plan JSON path.
        #[arg(long)]
        plan: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum OpsJournalCommand {
    /// Start a session journal.
    Start {
        /// Session name.
        #[arg(long)]
        name: String,
        /// Optional plan id.
        #[arg(long)]
        plan_id: Option<String>,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Add a command evidence entry.
    AddCommand {
        /// Session id or name.
        #[arg(long)]
        session: String,
        /// Optional exit status.
        #[arg(long)]
        exit_status: Option<i32>,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Command and arguments after `--`.
        command: Vec<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Add a note evidence entry.
    AddNote {
        /// Session id or name.
        #[arg(long)]
        session: String,
        /// Note kind.
        #[arg(long = "kind")]
        note_kind: String,
        /// Note message.
        #[arg(long)]
        message: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
    /// Render a Markdown session summary.
    Summary {
        /// Session id or name.
        #[arg(long)]
        session: String,
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Human)]
        format: OutputFormat,
    },
}

#[derive(Debug, Subcommand)]
enum ProviderCommand {
    /// Inspect provider package capabilities.
    Capabilities {
        /// Repository root or path inside the repo.
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Workspace selector in project:workspace or repo-relative path form.
        #[arg(long)]
        workspace: Option<String>,
        /// Base git ref.
        #[arg(long)]
        base: Option<String>,
        /// Head git ref.
        #[arg(long)]
        head: Option<String>,
        /// Explicit changed file.
        #[arg(long = "changed-file")]
        changed_files: Vec<String>,
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
        /// Validation depth.
        #[arg(long, value_enum, default_value_t = ValidationModeArg::Structural)]
        mode: ValidationModeArg,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ValidationModeArg {
    Structural,
    Metadata,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CiFallbackArg {
    All,
    None,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CiProviderArg {
    GithubActions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DependencyRewriteArg {
    Auto,
    Off,
    ReportOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum AdoptionCiArg {
    Update,
    Off,
    ReportOnly,
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
            let name = RepoName::new(name).map_err(RepoctlError::diagnostic)?;
            let repo_root = init_repo_root(repo.as_deref(), &name);
            let plan = repoctl.init(InitRequest {
                repo_root,
                name,
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
                NewCommand::Tool(args) => (ProjectKind::Tool, args),
            };
            let format = args.format;
            let args = args.complete_interactively(NewProjectPromptContext {
                kind: &kind,
                format,
            })?;
            let path = normalize_new_project_path(&kind, args.path.as_deref())
                .map_err(RepoctlError::diagnostic)?;
            let plan = repoctl.new_project(NewProjectRequest {
                repo: args.repo,
                kind,
                path,
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
                mode,
                format,
            } => {
                let changed_files = parse_changed_files(changed_files)?;
                let report = repoctl.validate_graph(GraphValidateRequest {
                    repo,
                    changed_files,
                    mode: mode.into(),
                })?;
                render_validation_report(&report, format, "Graph validation passed.")?;
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
            render_validation_report(&validation, format, "No boundary violations found.")?;
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
            workspaces,
            affected,
            changed_files,
            base,
            head,
            dry_run,
            concurrency,
            format,
        } => {
            let report = repoctl.run_task(TaskRunRequest {
                repo,
                tasks: vec![TaskName::new(task).map_err(RepoctlError::diagnostic)?],
                projects: parse_projects(projects)?,
                workspaces: parse_workspaces(workspaces)?,
                affected,
                changed_files: parse_changed_files(changed_files)?,
                base,
                head,
                concurrency,
                dry_run,
            })?;
            render_task_report(&report, format)?;
            Ok(exit_for_diagnostics(&report.diagnostics))
        }
        Command::Ci { command } => match command {
            CiCommand::Workflow {
                repo,
                provider,
                write,
                format,
            } => {
                let report = repoctl.ci_workflow(CiWorkflowRequest {
                    repo,
                    provider: provider.into(),
                    write,
                })?;
                render_ci_workflow(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            CiCommand::Matrix {
                repo,
                tasks,
                changed_files,
                base,
                head,
                fallback,
                format,
            } => {
                let report = repoctl.ci_matrix(CiMatrixRequest {
                    repo,
                    tasks: parse_tasks(tasks)?,
                    changed_files: parse_changed_files(changed_files)?,
                    base,
                    head,
                    fallback: fallback.into(),
                })?;
                render_ci_matrix(&report, format)?;
                Ok(ExitCode::SUCCESS)
            }
            CiCommand::RunStep {
                task,
                project,
                workspace,
                repo,
                dry_run,
                format,
            } => {
                let report = repoctl.run_task(TaskRunRequest {
                    repo,
                    tasks: vec![TaskName::new(task).map_err(RepoctlError::diagnostic)?],
                    projects: vec![
                        ProjectName::new(project.clone()).map_err(RepoctlError::diagnostic)?,
                    ],
                    workspaces: vec![format!("{project}:{workspace}")],
                    affected: false,
                    changed_files: Vec::new(),
                    base: None,
                    head: None,
                    concurrency: None,
                    dry_run,
                })?;
                render_task_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            CiCommand::Summarize { repo, format } => {
                let report = repoctl.ci_matrix(CiMatrixRequest {
                    repo,
                    tasks: Vec::new(),
                    changed_files: Vec::new(),
                    base: None,
                    head: None,
                    fallback: CiFallback::All,
                })?;
                render_ci_matrix(&report, format)?;
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Adopt { command } => match command {
            AdoptCommand::Plan {
                source,
                dest,
                include,
                exclude,
                map,
                kind,
                owner,
                rewrite_deps,
                ci,
                verification,
                format,
            } => {
                let request = AdoptionPlanRequest {
                    source,
                    dest,
                    include,
                    exclude,
                    map: parse_path_overrides(map)?,
                    kind: parse_kind_overrides(kind)?,
                    owner: parse_owner_overrides(owner)?,
                    rewrite_deps: rewrite_deps.into(),
                    ci: ci.into(),
                    verification: verification.into(),
                    format: adoption_output_format(format),
                };
                let plan = repoctl.adopt_plan(request)?;
                render_adoption_plan(&plan, format)?;
                Ok(exit_for_diagnostics(&plan.diagnostics))
            }
            AdoptCommand::Apply {
                plan,
                refresh,
                format,
            } => {
                let applied = repoctl.adopt_apply(AdoptionApplyRequest { plan, refresh })?;
                render_adoption_plan(&applied, format)?;
                Ok(ExitCode::SUCCESS)
            }
            AdoptCommand::Verify { plan, format } => {
                let report = repoctl.adopt_verify(AdoptionVerifyRequest { plan })?;
                render_validation_report(&report, format, "Adoption verification passed.")?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
        },
        Command::Hygiene { command } => match command {
            HygieneCommand::Check { repo, format } => {
                let report = repoctl.hygiene_check(HygieneCheckRequest { repo })?;
                render_hygiene_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            HygieneCommand::Clean {
                repo,
                dry_run,
                format,
            } => {
                let report = repoctl.hygiene_clean(HygieneCleanRequest { repo, dry_run })?;
                render_hygiene_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
        },
        Command::Template { command } => match command {
            TemplateCommand::List { repo, format } => {
                let report = repoctl.template_list(TemplateListRequest { repo })?;
                render_template_list(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            TemplateCommand::Render {
                source,
                repo,
                inputs,
                dry_run,
                format,
            } => {
                let plan = repoctl.template_render(TemplateRenderRequest {
                    repo,
                    source: parse_template_source(&source)?,
                    inputs: parse_template_inputs(inputs)?,
                    dry_run,
                })?;
                render_render_plan(&plan, format)?;
                Ok(exit_for_diagnostics(&plan.diagnostics))
            }
        },
        Command::Codegen { command } => match command {
            CodegenCommand::Check {
                repo,
                changed_files,
                base,
                head,
                format,
            } => {
                let report = repoctl.codegen_check(CodegenCheckRequest {
                    repo,
                    base,
                    head,
                    changed_files: parse_changed_files(changed_files)?,
                })?;
                render_codegen_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
        },
        Command::Proto { command } => match command {
            ProtoCommand::Owners {
                selector,
                repo,
                format,
            } => {
                let report = repoctl.proto().owners(ProtoFacadeRequest {
                    repo,
                    operation: ProtoOperation::Owners,
                    selector: Some(selector),
                    base: None,
                    head: None,
                    changed_files: Vec::new(),
                })?;
                render_proto_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            ProtoCommand::Consumers {
                selector,
                repo,
                format,
            } => {
                let report = repoctl.proto().consumers(ProtoFacadeRequest {
                    repo,
                    operation: ProtoOperation::Consumers,
                    selector: Some(selector),
                    base: None,
                    head: None,
                    changed_files: Vec::new(),
                })?;
                render_proto_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            ProtoCommand::Check {
                repo,
                changed_files,
                base,
                head,
                format,
            } => {
                let report = repoctl.proto().check(ProtoFacadeRequest {
                    repo,
                    operation: ProtoOperation::Check,
                    selector: None,
                    base,
                    head,
                    changed_files: parse_changed_files(changed_files)?,
                })?;
                render_proto_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
        },
        Command::Context {
            project,
            repo,
            for_target,
            format,
        } => {
            let report = repoctl.ai_context(AiContextRequest {
                repo,
                project: ProjectName::new(project).map_err(RepoctlError::diagnostic)?,
                audience: for_target,
            })?;
            render_ai_context(&report, format)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Pr { command } => match command {
            PrCommand::Summary {
                repo,
                changed_files,
                base,
                head,
                format,
            } => {
                let report = repoctl.pr_summary(PrSummaryRequest {
                    repo,
                    base,
                    head,
                    changed_files: parse_changed_files(changed_files)?,
                })?;
                render_pr_summary(&report, format)?;
                Ok(ExitCode::SUCCESS)
            }
        },
        Command::Iac { command } => match command {
            IacCommand::Plan {
                repo,
                affected,
                project,
                env,
                core,
                dry_run,
                changed_files,
                base,
                head,
                format,
            } => {
                let report = repoctl.iac().plan(IacFacadeRequest {
                    repo,
                    affected,
                    project: project
                        .map(ProjectName::new)
                        .transpose()
                        .map_err(RepoctlError::diagnostic)?,
                    env,
                    core,
                    base,
                    head,
                    changed_files: parse_changed_files(changed_files)?,
                    dry_run,
                })?;
                render_iac_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
        },
        Command::Ops { command } => match command {
            OpsCommand::Plan {
                repo,
                base,
                head,
                changed_files,
                environments,
                tasks,
                output,
                format,
            } => {
                let report = repoctl.ops_plan(OpsPlanRequest {
                    repo,
                    base,
                    head,
                    changed_files: parse_changed_files(changed_files)?,
                    environments,
                    tasks: parse_tasks(tasks)?,
                    output,
                })?;
                render_ops_plan(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            OpsCommand::Verify { repo, plan, format } => {
                let report = repoctl.ops_verify(OpsVerifyRequest { repo, plan })?;
                render_ops_verify_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            OpsCommand::Journal { command } => {
                let (repo, action, format) = match command {
                    OpsJournalCommand::Start {
                        name,
                        plan_id,
                        repo,
                        format,
                    } => (repo, OpsJournalAction::Start { name, plan_id }, format),
                    OpsJournalCommand::AddCommand {
                        session,
                        exit_status,
                        repo,
                        command,
                        format,
                    } => (
                        repo,
                        OpsJournalAction::AddCommand {
                            session,
                            command: command.join(" "),
                            exit_status,
                        },
                        format,
                    ),
                    OpsJournalCommand::AddNote {
                        session,
                        note_kind,
                        message,
                        repo,
                        format,
                    } => (
                        repo,
                        OpsJournalAction::AddNote {
                            session,
                            note_kind,
                            message,
                        },
                        format,
                    ),
                    OpsJournalCommand::Summary {
                        session,
                        repo,
                        format,
                    } => (repo, OpsJournalAction::Summary { session }, format),
                };
                let report = repoctl.ops_journal(OpsJournalRequest { repo, action })?;
                render_ops_journal_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
            OpsCommand::Reconcile { repo, plan, format } => {
                let report = repoctl.ops_reconcile(OpsReconcileRequest { repo, plan })?;
                render_ops_reconcile_report(&report, format)?;
                Ok(exit_for_diagnostics(&report.diagnostics))
            }
        },
        Command::Provider { command } => match command {
            ProviderCommand::Capabilities {
                repo,
                workspace,
                base,
                head,
                changed_files,
                format,
            } => {
                let report = repoctl.provider_capabilities(ProviderCapabilityRequest {
                    repo,
                    workspace,
                    base,
                    head,
                    changed_files: parse_changed_files(changed_files)?,
                })?;
                render_provider_capabilities(&report, format)?;
                Ok(exit_for_provider_reports(&report))
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
                render_validation_report(&validation, format, "Skills are in sync.")?;
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
                render_validation_report(&validation, format, "Skills are in sync.")?;
                Ok(exit_for_diagnostics(&validation.diagnostics))
            }
        },
    }
}

fn init_repo_root(explicit_root: Option<&Path>, name: &RepoName) -> PathBuf {
    explicit_root.map_or_else(|| PathBuf::from(name.as_str()), Path::to_path_buf)
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

impl From<ValidationModeArg> for ValidationMode {
    fn from(value: ValidationModeArg) -> Self {
        match value {
            ValidationModeArg::Structural => Self::Structural,
            ValidationModeArg::Metadata => Self::Metadata,
            ValidationModeArg::Full => Self::Full,
        }
    }
}

impl From<CiFallbackArg> for CiFallback {
    fn from(value: CiFallbackArg) -> Self {
        match value {
            CiFallbackArg::All => Self::All,
            CiFallbackArg::None => Self::None,
            CiFallbackArg::Error => Self::Error,
        }
    }
}

impl From<CiProviderArg> for CiProvider {
    fn from(value: CiProviderArg) -> Self {
        match value {
            CiProviderArg::GithubActions => Self::GitHubActions,
        }
    }
}

impl From<DependencyRewriteArg> for DependencyRewriteMode {
    fn from(value: DependencyRewriteArg) -> Self {
        match value {
            DependencyRewriteArg::Auto => Self::Auto,
            DependencyRewriteArg::Off => Self::Off,
            DependencyRewriteArg::ReportOnly => Self::ReportOnly,
        }
    }
}

impl From<AdoptionCiArg> for AdoptionCiMode {
    fn from(value: AdoptionCiArg) -> Self {
        match value {
            AdoptionCiArg::Update => Self::Update,
            AdoptionCiArg::Off => Self::Off,
            AdoptionCiArg::ReportOnly => Self::ReportOnly,
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

fn parse_workspaces(values: Vec<String>) -> Result<Vec<String>, RepoctlError> {
    for value in &values {
        let Some((project, workspace)) = value.split_once(':') else {
            return Err(RepoctlError::diagnostic(
                Diagnostic::error(
                    "task.workspace.invalid",
                    "workspace selectors must use project:workspace syntax",
                )
                .with_help("example: --workspace apps.catalog:api"),
            ));
        };
        ProjectName::new(project.to_string()).map_err(RepoctlError::diagnostic)?;
        WorkspaceName::new(workspace.to_string()).map_err(RepoctlError::diagnostic)?;
    }
    Ok(values)
}

fn parse_path_overrides(
    values: Vec<String>,
) -> Result<BTreeMap<String, RepoRelativePath>, RepoctlError> {
    values
        .into_iter()
        .map(|value| {
            let (source, target) = parse_assignment(
                &value,
                "adoption.map.invalid",
                "use SOURCE=DEST, for example operon=frameworks/operon",
            )?;
            let path =
                RepoRelativePath::new(target.to_string()).map_err(RepoctlError::diagnostic)?;
            Ok((source.to_string(), path))
        })
        .collect()
}

fn parse_kind_overrides(
    values: Vec<String>,
) -> Result<BTreeMap<String, ProjectKind>, RepoctlError> {
    values
        .into_iter()
        .map(|value| {
            let (source, kind) = parse_assignment(
                &value,
                "adoption.kind.invalid",
                "use SOURCE=KIND, for example skills=tool",
            )?;
            Ok((source.to_string(), parse_project_kind_arg(kind)?))
        })
        .collect()
}

fn parse_owner_overrides(
    values: Vec<String>,
) -> Result<BTreeMap<String, OwnerHandle>, RepoctlError> {
    values
        .into_iter()
        .map(|value| {
            let (source, owner) = parse_assignment(
                &value,
                "adoption.owner.invalid",
                "use SOURCE=@owner, for example operon=@platform",
            )?;
            Ok((
                source.to_string(),
                OwnerHandle::new(owner.to_string()).map_err(RepoctlError::diagnostic)?,
            ))
        })
        .collect()
}

fn parse_assignment<'a>(
    value: &'a str,
    code: &'static str,
    help: &'static str,
) -> Result<(&'a str, &'a str), RepoctlError> {
    let Some((left, right)) = value.split_once('=') else {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(code, format!("invalid assignment `{value}`")).with_help(help),
        ));
    };
    if left.is_empty() || right.is_empty() {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(code, format!("invalid assignment `{value}`")).with_help(help),
        ));
    }
    Ok((left, right))
}

fn parse_project_kind_arg(value: &str) -> Result<ProjectKind, RepoctlError> {
    match value {
        "app" => Ok(ProjectKind::App),
        "framework" => Ok(ProjectKind::Framework),
        "foundation-service" => Ok(ProjectKind::FoundationService),
        "proto-root" => Ok(ProjectKind::ProtoRoot),
        "core-infra" => Ok(ProjectKind::CoreInfra),
        "core-infra-component" => Ok(ProjectKind::CoreInfraComponent),
        "tool" => Ok(ProjectKind::Tool),
        _ => Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.kind.invalid",
                format!("unsupported project kind `{value}`"),
            )
            .with_help(
                "valid kinds: app, framework, foundation-service, core-infra-component, tool",
            ),
        )),
    }
}

fn adoption_output_format(format: OutputFormat) -> AdoptionOutputFormat {
    match format {
        OutputFormat::Human => AdoptionOutputFormat::Human,
        OutputFormat::Json => AdoptionOutputFormat::Json,
        OutputFormat::GithubActions => AdoptionOutputFormat::GitHubActions,
    }
}

fn parse_template_source(value: &str) -> Result<TemplateSource, RepoctlError> {
    if let Some(name) = value.strip_prefix("builtin:") {
        return Ok(TemplateSource::Builtin {
            name: name.to_string(),
        });
    }
    if let Some(root) = value.strip_prefix("local:") {
        return Ok(TemplateSource::Local {
            root: RepoRelativePath::new(root.to_string()).map_err(RepoctlError::diagnostic)?,
        });
    }
    Err(RepoctlError::diagnostic(
        Diagnostic::error(
            "template.source.invalid",
            "template source must use builtin:<name> or local:<repo-relative-path>",
        )
        .with_help("example: repoctl template render builtin:app --input name=catalog"),
    ))
}

fn parse_template_inputs(values: Vec<String>) -> Result<serde_json::Value, RepoctlError> {
    let mut map = serde_json::Map::new();
    for value in values {
        let Some((key, raw_value)) = value.split_once('=') else {
            return Err(RepoctlError::diagnostic(
                Diagnostic::error(
                    "template.input.invalid",
                    "template input must use key=value syntax",
                )
                .with_help("example: --input name=catalog"),
            ));
        };
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "template.input.invalid_key",
                "template input keys may only contain ASCII letters, numbers, and underscore",
            )));
        }
        map.insert(key.to_string(), parse_template_input_value(raw_value));
    }
    Ok(serde_json::Value::Object(map))
}

fn parse_template_input_value(value: &str) -> serde_json::Value {
    match value {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        _ => value.parse::<i64>().map_or_else(
            |_| serde_json::Value::String(value.to_string()),
            |number| serde_json::Value::Number(number.into()),
        ),
    }
}

fn render_init_plan(plan: &InitPlan, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(plan),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Repository initialization plan");
            append_operation_table(&mut output, "Files and directories", &plan.operations);
            append_diagnostics(&mut output, &plan.warnings);
            append_numbered_section(&mut output, "Next steps", &plan.next_steps);
            write_stdout(&output)
        }
    }
}

fn render_render_plan(plan: &RenderPlan, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(plan),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Planned changes");
            append_operation_table(&mut output, "Files and directories", &plan.operations);
            append_diagnostics(&mut output, &plan.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_validation_report(
    report: &ValidationReport,
    format: OutputFormat,
    success_message: &str,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            if report.diagnostics.is_empty() {
                write_stdout(&format!("{success_message}\n"))
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
            append_title(&mut output, "Repository graph");
            append_table(
                &mut output,
                &format!("Nodes ({})", report.snapshot.graph.nodes.len()),
                &["Id", "Label"],
                report
                    .snapshot
                    .graph
                    .nodes
                    .iter()
                    .map(|node| vec![node.id.clone(), node.label.clone()])
                    .collect(),
            );
            append_table(
                &mut output,
                &format!("Edges ({})", report.snapshot.graph.edges.len()),
                &["From", "To", "Kind"],
                report
                    .snapshot
                    .graph
                    .edges
                    .iter()
                    .map(|edge| {
                        vec![
                            edge.from.clone(),
                            edge.to.clone(),
                            format!("{:?}", edge.kind),
                        ]
                    })
                    .collect(),
            );
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
            let _ = writeln!(&mut output, "Explanation for `{}`\n", report.selector);
            append_table(
                &mut output,
                &format!("Nodes ({})", report.nodes.len()),
                &["Id", "Label"],
                report
                    .nodes
                    .iter()
                    .map(|node| vec![node.id.clone(), node.label.clone()])
                    .collect(),
            );
            append_table(
                &mut output,
                &format!("Edges ({})", report.edges.len()),
                &["From", "To", "Kind"],
                report
                    .edges
                    .iter()
                    .map(|edge| {
                        vec![
                            edge.from.clone(),
                            edge.to.clone(),
                            format!("{:?}", edge.kind),
                        ]
                    })
                    .collect(),
            );
            write_stdout(&output)
        }
    }
}

fn render_affected(report: &AffectedReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Affected summary");
            append_list_section(
                &mut output,
                "Direct projects",
                report
                    .directly_affected
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            );
            append_list_section(
                &mut output,
                "Transitive projects",
                report
                    .transitively_affected
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            );
            append_list_section(&mut output, "Workspaces", report.workspaces.clone());
            append_list_section(&mut output, "Tasks", report.tasks.clone());
            append_list_section(&mut output, "Risk flags", report.risk_flags.clone());
            append_list_section(
                &mut output,
                "Suggested reviewers",
                report
                    .suggested_reviewers
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            );
            append_table(
                &mut output,
                &format!("Reasons ({})", report.reasons.len()),
                &["Source", "Target", "Reason"],
                report
                    .reasons
                    .iter()
                    .map(|reason| {
                        vec![
                            reason.source.clone(),
                            reason.target.clone(),
                            reason.reason.clone(),
                        ]
                    })
                    .collect(),
            );
            append_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_task_report(report: &TaskRunReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Task report");
            append_command_table(&mut output, "Planned commands", &report.commands);
            append_table(
                &mut output,
                &format!("Results ({})", report.outputs.len()),
                &["Project", "Workspace", "Task", "Status"],
                report
                    .outputs
                    .iter()
                    .map(|output_item| {
                        vec![
                            output_item.project.to_string(),
                            output_item.workspace.to_string(),
                            output_item.task.to_string(),
                            output_item.output.status.to_string(),
                        ]
                    })
                    .collect(),
            );
            append_failed_output(&mut output, report);
            append_diagnostics(&mut output, &report.diagnostics);
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
            append_title(&mut output, "CI matrix");
            append_table(
                &mut output,
                &format!("Entries ({})", report.entries.len()),
                &["Project", "Workspace", "Task"],
                report
                    .entries
                    .iter()
                    .map(|entry| {
                        vec![
                            json_field(entry, "project"),
                            json_field(entry, "workspace"),
                            json_field(entry, "task"),
                        ]
                    })
                    .collect(),
            );
            write_stdout(&output)
        }
    }
}

fn render_ci_workflow(report: &CiWorkflowReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "CI workflow");
            let _ = writeln!(output, "Path: {}", report.path);
            let _ = writeln!(output, "Bytes: {}", report.content.len());
            append_operation_table(&mut output, "Operations", &report.operations);
            append_diagnostics(&mut output, &report.diagnostics);
            if report
                .operations
                .iter()
                .any(|operation| operation.operation == "plan-file")
            {
                output.push_str(
                    "\nWrite it with:\n  repoctl ci workflow --provider github-actions --write\n",
                );
            }
            write_stdout(&output)
        }
    }
}

fn render_adoption_plan(plan: &AdoptionPlan, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(plan),
        OutputFormat::Human => {
            let mut output = String::new();
            let selected = plan.sources.iter().filter(|source| !source.skipped).count();
            let skipped = plan.sources.len().saturating_sub(selected);
            append_title(&mut output, "Adoption plan");
            let _ = writeln!(
                output,
                "{} source repos, {} selected, {} skipped",
                plan.sources.len(),
                selected,
                skipped
            );
            append_table(
                &mut output,
                "Placement",
                &["Source", "Destination", "Kind", "Confidence", "Decision"],
                plan.sources
                    .iter()
                    .map(|source| {
                        vec![
                            source.name.clone(),
                            if source.skipped {
                                "skipped".to_string()
                            } else {
                                source.destination_path.to_string()
                            },
                            project_kind_label(&source.inferred_kind).to_string(),
                            format!("{:.2}", source.confidence),
                            if source.override_applied {
                                "override".to_string()
                            } else {
                                source.reasons.join("; ")
                            },
                        ]
                    })
                    .collect(),
            );
            append_table(
                &mut output,
                "Dependency rewrites",
                &["File", "Package", "To"],
                plan.dependency_rewrites
                    .iter()
                    .map(|rewrite| {
                        vec![
                            rewrite.file.to_string(),
                            rewrite.package.clone(),
                            rewrite.to.clone(),
                        ]
                    })
                    .collect(),
            );
            append_table(
                &mut output,
                "Copy operations",
                &["Operation", "Destination"],
                plan.operations
                    .iter()
                    .map(|operation| {
                        vec![
                            operation.operation.clone(),
                            operation.destination_path.to_string(),
                        ]
                    })
                    .collect(),
            );
            append_operation_table(&mut output, "CI operations", &plan.ci_operations);
            append_command_table(&mut output, "Verification", &plan.verification.commands);
            if !plan.verification.prerequisites.is_empty() {
                append_table(
                    &mut output,
                    "Prerequisites",
                    &["Tool", "Reason"],
                    plan.verification
                        .prerequisites
                        .iter()
                        .map(|tool| vec![tool.tool.clone(), tool.reason.clone()])
                        .collect(),
                );
            }
            append_diagnostics(&mut output, &plan.diagnostics);
            output.push_str("\nNext commands:\n");
            output.push_str(
                "  repoctl adopt plan --source <source> --dest <dest> --format json > \
                 target/repoctl/adopt-plan.json\n",
            );
            output.push_str("  repoctl adopt apply --plan target/repoctl/adopt-plan.json\n");
            output.push_str("  repoctl adopt verify --plan target/repoctl/adopt-plan.json\n");
            write_stdout(&output)
        }
    }
}

fn render_hygiene_report(report: &HygieneReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Hygiene");
            append_diagnostics(&mut output, &report.diagnostics);
            append_operation_table(&mut output, "Cleanable operations", &report.operations);
            if report.diagnostics.is_empty() {
                output.push_str("No generated artifact leakage found.\n");
            }
            write_stdout(&output)
        }
    }
}

fn render_template_list(
    report: &TemplateListReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => write_stdout(&format_template_list_human(report)),
    }
}

fn render_codegen_report(
    report: &CodegenCheckReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    let validation = ValidationReport::new(report.diagnostics.clone());
    render_validation_report(&validation, format, "Generated-code check passed.")
}

fn render_proto_report(
    report: &ProtoFacadeReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Proto report");
            append_list_section(
                &mut output,
                "Owners",
                report.owners.iter().map(ToString::to_string).collect(),
            );
            append_list_section(
                &mut output,
                "Consumers",
                report.consumers.iter().map(ToString::to_string).collect(),
            );
            append_command_table(&mut output, "Commands", &report.commands);
            append_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_ai_context(report: &AiContext, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => write_json(&report.payload),
    }
}

fn render_pr_summary(report: &PrSummary, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => write_stdout(&report.markdown),
    }
}

fn render_iac_report(report: &IacFacadeReport, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "IaC plan");
            append_command_table(&mut output, "Commands", &report.commands);
            append_list_section(&mut output, "Risk flags", report.risk_flags.clone());
            append_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_ops_plan(plan: &OpsPlan, format: OutputFormat) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(plan),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Operations plan");
            let _ = writeln!(output, "Id: {}", plan.id);
            append_list_section(&mut output, "Environments", plan.environments.clone());
            append_list_section(
                &mut output,
                "Affected projects",
                plan.affected
                    .directly_affected
                    .iter()
                    .chain(plan.affected.transitively_affected.iter())
                    .map(ToString::to_string)
                    .collect(),
            );
            append_command_table(&mut output, "Task dry-run", &plan.task_plan.commands);
            append_table(
                &mut output,
                &format!("IaC previews ({})", plan.iac.len()),
                &["Project", "Workspace", "Stack", "Command"],
                plan.iac
                    .iter()
                    .map(|operation| {
                        vec![
                            operation
                                .project
                                .as_ref()
                                .map_or_else(|| "core-infra".to_string(), ToString::to_string),
                            operation.workspace.clone(),
                            operation.stack.clone(),
                            command_line(&operation.preview_command),
                        ]
                    })
                    .collect(),
            );
            append_table(
                &mut output,
                &format!("DNS ({})", plan.dns.len()),
                &["Record", "Target", "Proxied"],
                plan.dns
                    .iter()
                    .map(|operation| {
                        vec![
                            operation.record.clone(),
                            operation.expected_target.clone(),
                            operation
                                .expected_proxied
                                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
                        ]
                    })
                    .collect(),
            );
            append_table(
                &mut output,
                &format!("CDN ({})", plan.cdn.len()),
                &["Provider", "Alias", "Headers"],
                plan.cdn
                    .iter()
                    .map(|check| {
                        vec![
                            check.provider.clone(),
                            check.alias.clone(),
                            check.expected_response_headers.join(", "),
                        ]
                    })
                    .collect(),
            );
            append_table(
                &mut output,
                &format!(
                    "Provider capabilities ({})",
                    plan.provider_capabilities.len()
                ),
                &["Workspace", "Package", "Version", "Status"],
                plan.provider_capabilities
                    .iter()
                    .map(|report| {
                        vec![
                            report.workspace.clone(),
                            report.package.clone(),
                            report.version.clone(),
                            report.status.clone(),
                        ]
                    })
                    .collect(),
            );
            append_table(
                &mut output,
                &format!("Probes ({})", plan.probes.len()),
                &["Name", "Method", "URL"],
                plan.probes
                    .iter()
                    .map(|probe| vec![probe.name.clone(), probe.method.clone(), probe.url.clone()])
                    .collect(),
            );
            append_list_section(&mut output, "Required env", plan.required_env.clone());
            append_list_section(&mut output, "Production gaps", plan.production_gaps.clone());
            append_diagnostics(&mut output, &plan.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_ops_verify_report(
    report: &OpsVerifyReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Operations verification");
            append_command_table(&mut output, "Non-mutating commands", &report.commands);
            append_command_table(
                &mut output,
                "Skipped mutating commands",
                &report.skipped_mutating_commands,
            );
            append_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_ops_reconcile_report(
    report: &OpsReconcileReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Manual-state reconciliation");
            append_table(
                &mut output,
                &format!("Records ({})", report.records.len()),
                &["Kind", "Resource", "Status", "Managed equivalent"],
                report
                    .records
                    .iter()
                    .map(|record| {
                        vec![
                            record.kind.clone(),
                            record.resource.clone(),
                            record.status.clone(),
                            record.managed_equivalent.clone().unwrap_or_default(),
                        ]
                    })
                    .collect(),
            );
            append_command_table(&mut output, "Cleanup commands", &report.cleanup_commands);
            append_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_ops_journal_report(
    report: &OpsJournalReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            if let Some(markdown) = &report.markdown {
                return write_stdout(markdown);
            }
            let mut output = String::new();
            append_title(&mut output, "Operations journal");
            if let Some(path) = &report.path {
                let _ = writeln!(output, "Path: {}", path.display());
            }
            if let Some(journal) = &report.journal {
                let _ = writeln!(output, "Session: {} ({})", journal.name, journal.id);
                let _ = writeln!(output, "Entries: {}", journal.entries.len());
            }
            append_diagnostics(&mut output, &report.diagnostics);
            write_stdout(&output)
        }
    }
}

fn render_provider_capabilities(
    report: &[ProviderCapabilityReport],
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json | OutputFormat::GithubActions => write_json(report),
        OutputFormat::Human => {
            let mut output = String::new();
            append_title(&mut output, "Provider capabilities");
            append_table(
                &mut output,
                &format!("Reports ({})", report.len()),
                &[
                    "Workspace",
                    "Package",
                    "Version",
                    "Field",
                    "Status",
                    "Advice",
                ],
                report
                    .iter()
                    .map(|item| {
                        vec![
                            item.workspace.clone(),
                            item.package.clone(),
                            item.version.clone(),
                            item.field.clone(),
                            item.status.clone(),
                            item.advice.clone(),
                        ]
                    })
                    .collect(),
            );
            let diagnostics = report
                .iter()
                .flat_map(|item| item.diagnostics.clone())
                .collect::<Vec<_>>();
            append_diagnostics(&mut output, &diagnostics);
            write_stdout(&output)
        }
    }
}

fn append_diagnostics(output: &mut String, diagnostics: &[Diagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    if !output.ends_with("\n\n") {
        output.push('\n');
    }
    let _ = writeln!(output, "Diagnostics ({})", diagnostics.len());
    for diagnostic in diagnostics {
        append_diagnostic(output, diagnostic);
    }
}

fn render_diagnostics(diagnostics: &[Diagnostic]) -> Result<(), RepoctlError> {
    let mut output = String::new();
    for diagnostic in diagnostics {
        append_diagnostic(&mut output, diagnostic);
    }
    write_stdout(&output)
}

fn format_template_list_human(report: &TemplateListReport) -> String {
    let mut output = String::new();
    append_title(&mut output, "Available templates");
    append_table(
        &mut output,
        &format!("Templates ({})", report.templates.len()),
        &["Source", "Kind", "Name"],
        report
            .templates
            .iter()
            .map(|template| {
                vec![
                    template.source.clone(),
                    template.kind.clone(),
                    template.name.clone(),
                ]
            })
            .collect(),
    );
    if !report.templates.is_empty() {
        output.push_str("\nRender a template with:\n");
        output.push_str("  repoctl template render <source> --input name=<name>\n");
    }
    append_diagnostics(&mut output, &report.diagnostics);
    output
}

fn append_title(output: &mut String, title: &str) {
    let _ = writeln!(output, "{title}");
    let _ = writeln!(output, "{}", "-".repeat(title.len()));
    output.push('\n');
}

fn append_operation_table(output: &mut String, title: &str, operations: &[FileOperation]) {
    append_table(
        output,
        &format!("{title} ({})", operations.len()),
        &["Operation", "Path"],
        operations
            .iter()
            .map(|operation| vec![operation.operation.clone(), operation.path.to_string()])
            .collect(),
    );
}

fn append_command_table(output: &mut String, title: &str, commands: &[ProcessCommand]) {
    append_table(
        output,
        &format!("{title} ({})", commands.len()),
        &["Scope", "Directory", "Command"],
        commands
            .iter()
            .map(|command| {
                vec![
                    command_scope(command),
                    command.cwd.to_string(),
                    command_line(command),
                ]
            })
            .collect(),
    );
}

fn append_table(output: &mut String, title: &str, headers: &[&str], rows: Vec<Vec<String>>) {
    if !output.ends_with("\n\n") {
        output.push('\n');
    }
    let _ = writeln!(output, "{title}");
    if rows.is_empty() {
        output.push_str("  (none)\n");
        return;
    }

    let mut widths = headers
        .iter()
        .map(|header| header.len())
        .collect::<Vec<_>>();
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.len());
            } else {
                widths.push(cell.len());
            }
        }
    }

    let header_cells = headers
        .iter()
        .map(|header| (*header).to_string())
        .collect::<Vec<_>>();
    append_table_row(output, &widths, &header_cells);
    let separators = widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>();
    append_table_row(output, &widths, &separators);
    for row in rows {
        append_table_row(output, &widths, &row);
    }
}

fn append_table_row(output: &mut String, widths: &[usize], cells: &[String]) {
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            output.push_str("  ");
        }
        if index + 1 == cells.len() {
            output.push_str(cell);
        } else {
            let width = widths.get(index).copied().unwrap_or(cell.len());
            let _ = write!(output, "{cell:<width$}");
        }
    }
    output.push('\n');
}

fn append_list_section(output: &mut String, title: &str, items: Vec<String>) {
    if !output.ends_with("\n\n") {
        output.push('\n');
    }
    let _ = writeln!(output, "{title} ({})", items.len());
    if items.is_empty() {
        output.push_str("  (none)\n");
        return;
    }
    for item in items {
        let _ = writeln!(output, "  {item}");
    }
}

fn append_numbered_section(output: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    if !output.ends_with("\n\n") {
        output.push('\n');
    }
    let _ = writeln!(output, "{title}");
    for (index, item) in items.iter().enumerate() {
        let number = index + 1;
        let _ = writeln!(output, "  {number}. {item}");
    }
}

fn append_diagnostic(output: &mut String, diagnostic: &Diagnostic) {
    let _ = writeln!(
        output,
        "{}: {}",
        severity_label(&diagnostic.severity),
        diagnostic.message
    );
    let _ = writeln!(output, "  code: {}", diagnostic.code);
    if let Some(source) = &diagnostic.source {
        let _ = writeln!(output, "  path: {}", source.path);
        if let Some(span) = &source.span {
            let _ = writeln!(output, "  span: {}:{}", span.line, span.column);
        }
    }
    if let Some(project) = &diagnostic.project {
        let _ = writeln!(output, "  project: {project}");
    }
    if let Some(workspace) = &diagnostic.workspace {
        let _ = writeln!(output, "  workspace: {workspace}");
    }
    if let Some(help) = &diagnostic.help {
        let _ = writeln!(output, "  help: {help}");
    }
}

fn append_failed_output(output: &mut String, report: &TaskRunReport) {
    let failed = report
        .outputs
        .iter()
        .filter(|output_item| output_item.output.status != 0)
        .collect::<Vec<_>>();
    if failed.is_empty() {
        return;
    }
    if !output.ends_with("\n\n") {
        output.push('\n');
    }
    let _ = writeln!(output, "Failure output ({})", failed.len());
    for output_item in failed {
        let _ = writeln!(
            output,
            "{}:{}:{}",
            output_item.project, output_item.workspace, output_item.task
        );
        append_stream_snippet(output, "stdout", &output_item.output.stdout);
        append_stream_snippet(output, "stderr", &output_item.output.stderr);
    }
}

fn append_stream_snippet(output: &mut String, label: &str, value: &str) {
    if value.trim().is_empty() {
        return;
    }
    let _ = writeln!(output, "  {label}:");
    for line in value.lines().take(20) {
        let _ = writeln!(output, "    {line}");
    }
}

fn command_scope(command: &ProcessCommand) -> String {
    match (&command.project, &command.workspace, &command.task) {
        (Some(project), Some(workspace), Some(task)) => {
            format!("{project}:{workspace}:{task}")
        }
        (Some(project), Some(workspace), None) => format!("{project}:{workspace}"),
        (Some(project), None, Some(task)) => format!("{project}:{task}"),
        (Some(project), None, None) => project.to_string(),
        (None, None, Some(task)) => task.to_string(),
        _ => "-".to_string(),
    }
}

fn command_line(command: &ProcessCommand) -> String {
    std::iter::once(command.program.as_str())
        .chain(command.args.iter().map(String::as_str))
        .map(shell_display_arg)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_display_arg(value: &str) -> String {
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn json_field(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn severity_label(severity: &Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

fn project_kind_label(kind: &ProjectKind) -> &'static str {
    match kind {
        ProjectKind::App => "app",
        ProjectKind::Framework => "framework",
        ProjectKind::FoundationService => "foundation-service",
        ProjectKind::ProtoRoot => "proto-root",
        ProjectKind::CoreInfra => "core-infra",
        ProjectKind::CoreInfraComponent => "core-infra-component",
        ProjectKind::Tool => "tool",
    }
}

fn write_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<(), RepoctlError> {
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
    let mut output = String::new();
    for diagnostic in diagnostics {
        append_diagnostic(&mut output, diagnostic);
    }
    stderr.write_all(output.as_bytes())
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

fn exit_for_provider_reports(reports: &[ProviderCapabilityReport]) -> ExitCode {
    if reports.iter().any(|report| {
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use repoctl::TemplateSummary;

    use super::*;

    #[test]
    fn test_should_initialize_named_repo_under_parent_directory() -> Result<(), Diagnostic> {
        let name = RepoName::new("universe")?;
        let root = init_repo_root(None, &name);

        assert_eq!(root, PathBuf::from("universe"));
        Ok(())
    }

    #[test]
    fn test_should_use_explicit_init_repo_root_when_provided() -> Result<(), Diagnostic> {
        let name = RepoName::new("universe")?;
        let root = init_repo_root(Some(Path::new(".")), &name);

        assert_eq!(root, PathBuf::from("."));
        Ok(())
    }

    #[test]
    fn test_should_render_template_list_as_readable_table() {
        let report = TemplateListReport {
            templates: vec![TemplateSummary {
                source: "builtin:app".to_string(),
                name: "app".to_string(),
                kind: "scaffold".to_string(),
            }],
            diagnostics: Vec::new(),
        };

        let output = format_template_list_human(&report);

        assert!(output.contains("Available templates"));
        assert!(output.contains("Source       Kind      Name"));
        assert!(output.contains("builtin:app  scaffold  app"));
        assert!(!output.contains('\t'));
        assert!(output.contains("repoctl template render <source> --input name=<name>"));
    }

    #[test]
    fn test_should_render_diagnostics_with_actionable_fields() {
        let diagnostic = Diagnostic::error("template.source.invalid", "invalid source")
            .with_path("templates/app/template.yaml")
            .with_help("use builtin:<name> or local:<path>");
        let mut output = String::new();

        append_diagnostic(&mut output, &diagnostic);

        assert!(output.contains("error: invalid source"));
        assert!(output.contains("code: template.source.invalid"));
        assert!(output.contains("path: templates/app/template.yaml"));
        assert!(output.contains("help: use builtin:<name> or local:<path>"));
    }

    #[test]
    fn test_should_place_bare_framework_name_under_frameworks_root() -> Result<(), Diagnostic> {
        let path = interactive::normalize_new_project_path(&ProjectKind::Framework, Some("core"))?;

        assert_eq!(path.as_str(), "frameworks/core");
        Ok(())
    }

    #[test]
    fn test_should_keep_explicit_framework_path() -> Result<(), Diagnostic> {
        let path = interactive::normalize_new_project_path(
            &ProjectKind::Framework,
            Some("frameworks/core"),
        )?;

        assert_eq!(path.as_str(), "frameworks/core");
        Ok(())
    }

    #[test]
    fn test_should_reject_framework_path_under_app_root() {
        let error =
            interactive::normalize_new_project_path(&ProjectKind::Framework, Some("apps/core"))
                .expect_err("kind mismatch");

        assert_eq!(error.code.as_ref(), "new.path.kind_mismatch");
    }

    #[test]
    fn test_should_require_project_name_below_kind_root() {
        let error =
            interactive::normalize_new_project_path(&ProjectKind::Framework, Some("frameworks"))
                .expect_err("missing slug");

        assert_eq!(error.code.as_ref(), "new.path.missing_slug");
    }

    #[test]
    fn test_should_reject_separator_only_project_path() {
        let error = interactive::normalize_new_project_path(&ProjectKind::Framework, Some("/"))
            .expect_err("missing path");

        assert_eq!(error.code.as_ref(), "new.path.required");
    }
}
