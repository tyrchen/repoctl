//! Stable diagnostics and error types.

use std::fmt::{Display, Formatter};

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Severity level for a repoctl diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Informational diagnostic that does not affect success.
    Info,
    /// Warning diagnostic that should be reviewed but does not fail validation.
    Warning,
    /// Error diagnostic that fails the requested operation.
    Error,
}

/// One-based source span for diagnostics when line and column are known.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSpan {
    /// One-based line number.
    pub line: usize,
    /// One-based column number.
    pub column: usize,
}

/// File and optional span associated with a diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSource {
    /// Repo-relative or absolute path depending on the caller boundary.
    pub path: Box<str>,
    /// Optional line and column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

/// Stable, actionable diagnostic returned by repoctl operations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// Stable machine-readable code.
    pub code: Box<str>,
    /// Severity for exit-code and renderer decisions.
    pub severity: Severity,
    /// Human-readable summary.
    pub message: Box<str>,
    /// Source location if the diagnostic can be tied to one file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Box<DiagnosticSource>>,
    /// Project context when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<Box<str>>,
    /// Workspace context when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Box<str>>,
    /// Remediation hint when a clear next step exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<Box<str>>,
}

impl Diagnostic {
    /// Creates an error diagnostic.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into().into_boxed_str(),
            severity: Severity::Error,
            message: message.into().into_boxed_str(),
            source: None,
            project: None,
            workspace: None,
            help: None,
        }
    }

    /// Creates a warning diagnostic.
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into().into_boxed_str(),
            severity: Severity::Warning,
            message: message.into().into_boxed_str(),
            source: None,
            project: None,
            workspace: None,
            help: None,
        }
    }

    /// Adds source path context.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.source = Some(Box::new(DiagnosticSource {
            path: path.into().into_boxed_str(),
            span: None,
        }));
        self
    }

    /// Adds project context.
    #[must_use]
    pub fn with_project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into().into_boxed_str());
        self
    }

    /// Adds workspace context.
    #[must_use]
    pub fn with_workspace(mut self, workspace: impl Into<String>) -> Self {
        self.workspace = Some(workspace.into().into_boxed_str());
        self
    }

    /// Adds remediation help.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into().into_boxed_str());
        self
    }
}

impl Display for Diagnostic {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if let Some(source) = &self.source {
            write!(
                f,
                "{} [{}] {}: {}",
                self.code,
                severity_label(&self.severity),
                source.path,
                self.message
            )
        } else {
            write!(
                f,
                "{} [{}] {}",
                self.code,
                severity_label(&self.severity),
                self.message
            )
        }
    }
}

/// A report containing zero or more diagnostics.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    /// Diagnostics produced by the operation.
    pub diagnostics: Vec<Diagnostic>,
}

impl ValidationReport {
    /// Creates a report from diagnostics.
    pub fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }

    /// Returns true when no error-severity diagnostics are present.
    pub fn is_success(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    /// Adds one diagnostic.
    pub fn push(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

/// Error type used by repoctl facade and capability services.
#[derive(Debug, Error)]
pub enum RepoctlError {
    /// A single controlled diagnostic.
    #[error("{diagnostic}")]
    Diagnostic {
        /// Diagnostic payload.
        diagnostic: Box<Diagnostic>,
    },
    /// Multiple controlled diagnostics.
    #[error("repoctl produced {} diagnostics", diagnostics.len())]
    Diagnostics {
        /// Diagnostic payloads.
        diagnostics: Vec<Diagnostic>,
    },
    /// Filesystem operation failed.
    #[error("I/O error at {path}: {source}")]
    Io {
        /// Path being accessed.
        path: Utf8PathBuf,
        /// Source error.
        #[source]
        source: std::io::Error,
    },
    /// External environment or toolchain is unavailable.
    #[error("environment error: {0}")]
    Environment(String),
    /// Internal bug surfaced as a controlled error.
    #[error("internal error: {0}")]
    Internal(String),
}

impl RepoctlError {
    /// Creates a diagnostic error.
    pub fn diagnostic(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic {
            diagnostic: Box::new(diagnostic),
        }
    }

    /// Creates an I/O error with path context.
    pub fn io(path: impl Into<Utf8PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    /// Returns diagnostics embedded in the error, if any.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        match self {
            Self::Diagnostic { diagnostic } => vec![(**diagnostic).clone()],
            Self::Diagnostics { diagnostics } => diagnostics.clone(),
            Self::Io { path, source } => vec![Diagnostic::error(
                "repoctl.io",
                format!("I/O error at {path}: {source}"),
            )],
            Self::Environment(message) => {
                vec![Diagnostic::error("repoctl.environment", message.clone())]
            }
            Self::Internal(message) => vec![Diagnostic::error("repoctl.internal", message.clone())],
        }
    }
}

fn severity_label(severity: &Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}
