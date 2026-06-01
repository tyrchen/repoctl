#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::missing_errors_doc)]

//! repoctl command-line frontend.

use std::{
    fmt::Write as FmtWrite,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use clap::{Parser, Subcommand, ValueEnum};
use repoctl::{
    BoundaryLintRequest, Diagnostic, ExplainReport, ExplainRequest, GraphPrintReport,
    GraphPrintRequest, GraphValidateRequest, RepoRelativePath, Repoctl, RepoctlError, Severity,
    ValidationReport,
};

#[derive(Debug, Parser)]
#[command(name = "repoctl", version, about = "Monorepo control plane")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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

fn render_validation_report(
    report: &ValidationReport,
    format: OutputFormat,
) -> Result<(), RepoctlError> {
    match format {
        OutputFormat::Json => write_json(report),
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
        OutputFormat::Json => write_json(report),
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
        OutputFormat::Json => write_json(report),
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
