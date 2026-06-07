#![forbid(unsafe_code)]
#![warn(rust_2024_compatibility, missing_docs, missing_debug_implementations)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
// The inspector is synchronous CLI/facade work and does not run inside async request handlers.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! Syntax-aware code-size inspection for repoctl.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{DirEntry, WalkBuilder};
use memchr::memchr_iter;
use rayon::prelude::*;
use repoctl_core::{
    AffectedRequest, CodeLanguage, CodeSizeConfig, CodeSizeFinding, CodeSizeInspectionReport,
    CodeSizeInspectionRequest, CodeSizeInspectionSummary, CodeSizeResolvedConfigSummary,
    CodeSizeRuleConfig, CodeSizeRuleConfigPatch, CodeSizeRuleKind, CodeSizeScope,
    CodeSizeSkippedReason, Diagnostic, DiscoverRequest, GeneratedCodeInspectionMode,
    ProjectManifest, RepoGlob, RepoRelativePath, RepoRoot, RepoSnapshot, RepoctlError,
};
use repoctl_engine::RepoctlEngine;
use repoctl_runner::RunnerService;
use tree_sitter::{Language, Node, Parser};

const BINARY_PREFIX_BYTES: usize = 8 * 1024;
const HEAVY_DIR_NAMES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    ".next",
    ".turbo",
    ".pnpm-store",
    ".yarn",
    ".cache",
    "build",
    "coverage",
];
const GENERATED_PATTERNS: &[&str] = &[
    "**/generated/**",
    "**/*.pb.rs",
    "**/*.pb.go",
    "**/*_pb.py",
    "**/*.generated.ts",
    "**/*.generated.tsx",
    "**/openapi/**",
];

/// Code-size inspector service.
#[derive(Clone, Debug)]
pub struct InspectorService {
    engine: RepoctlEngine,
    runner: RunnerService,
}

impl Default for InspectorService {
    fn default() -> Self {
        Self {
            engine: RepoctlEngine::with_default_adapters(),
            runner: RunnerService::with_default_adapters(),
        }
    }
}

impl InspectorService {
    /// Creates an inspector with default local adapters.
    #[must_use]
    pub fn with_default_adapters() -> Self {
        Self::default()
    }

    /// Creates an inspector from existing services.
    #[must_use]
    pub fn new(engine: RepoctlEngine, runner: RunnerService) -> Self {
        Self { engine, runner }
    }

    /// Runs code-size inspection.
    #[allow(clippy::too_many_lines)]
    pub fn inspect_size(
        &self,
        request: &CodeSizeInspectionRequest,
    ) -> Result<CodeSizeInspectionReport, RepoctlError> {
        let started = Instant::now();
        let snapshot = self.engine.discovery().discover(&DiscoverRequest {
            repo: request.repo.clone(),
        })?;
        let config = snapshot.repo_manifest.inspection.code_size.clone();
        let mut diagnostics = Vec::new();
        let mut skipped = BTreeMap::<String, u64>::new();

        if !config.enabled {
            return Ok(report(
                request,
                &config,
                CodeSizeInspectionSummary::default(),
                Vec::new(),
                diagnostics,
                skipped,
                started,
            ));
        }

        let selected = self.select_files(request, &snapshot, &config, &mut diagnostics)?;
        let files_considered = selected.len() as u64;
        let excludes = compile_globs(&config.excludes)?;
        let generated = compile_generated_globs(&snapshot)?;
        let filtered = selected
            .into_iter()
            .filter_map(|file| {
                if excludes.is_match(file.path.as_str()) {
                    increment_skip(&mut skipped, "excluded_by_config");
                    return None;
                }
                if config.generated_code == GeneratedCodeInspectionMode::Skip
                    && generated.is_match(file.path.as_str())
                {
                    increment_skip(&mut skipped, "generated_code");
                    return None;
                }
                Some(file)
            })
            .collect::<Vec<_>>();
        if filtered.len() > config.max_files.get() {
            diagnostics.push(Diagnostic::error(
                "inspect.code_size.too_many_files",
                format!(
                    "code-size inspection selected {} files, limit {}",
                    filtered.len(),
                    config.max_files
                ),
            ));
            return Ok(report(
                request,
                &config,
                CodeSizeInspectionSummary {
                    files_considered,
                    files_skipped: files_considered,
                    ..CodeSizeInspectionSummary::default()
                },
                Vec::new(),
                diagnostics,
                skipped,
                started,
            ));
        }

        let scan_results = filtered
            .par_iter()
            .map(|file| scan_file(file, request, &snapshot, &config))
            .collect::<Vec<_>>();

        let mut findings = Vec::new();
        let mut files_scanned = 0_u64;
        let mut files_errored = 0_u64;
        for result in scan_results {
            match result {
                ScanResult::Scanned {
                    findings: file_findings,
                    diagnostics: file_diagnostics,
                } => {
                    files_scanned += 1;
                    findings.extend(file_findings);
                    diagnostics.extend(file_diagnostics);
                }
                ScanResult::Skipped { reason } => increment_skip(&mut skipped, &reason),
                ScanResult::Errored { diagnostic, reason } => {
                    files_errored += 1;
                    diagnostics.push(diagnostic);
                    increment_skip(&mut skipped, &reason);
                }
            }
        }
        sort_findings(&mut findings);
        let files_with_findings = findings
            .iter()
            .map(|finding| finding.path.clone())
            .collect::<BTreeSet<_>>()
            .len() as u64;
        let files_skipped = skipped.values().sum();
        Ok(report(
            request,
            &config,
            CodeSizeInspectionSummary {
                files_considered,
                files_scanned,
                files_skipped,
                files_errored,
                finding_count: findings.len() as u64,
                files_with_findings,
                duration_millis: 0,
            },
            findings,
            diagnostics,
            skipped,
            started,
        ))
    }

    fn select_files(
        &self,
        request: &CodeSizeInspectionRequest,
        snapshot: &RepoSnapshot,
        config: &CodeSizeConfig,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<Vec<SelectedSourceFile>, RepoctlError> {
        let raw_paths = match request.scope {
            CodeSizeScope::All => walk_roots(&snapshot.root, &[RepoRelativePath::root()])?,
            CodeSizeScope::Changed => changed_files(request, &snapshot.root, diagnostics)?,
            CodeSizeScope::Affected => {
                let changed = changed_files(request, &snapshot.root, diagnostics)?;
                if changed.is_empty() {
                    diagnostics.push(Diagnostic::warning(
                        "inspect.code_size.no_changed_files",
                        "affected code-size inspection found no changed files",
                    ));
                    Vec::new()
                } else {
                    let affected = self.runner.affected(&AffectedRequest {
                        repo: request.repo.clone(),
                        base: request.base.clone(),
                        head: request.head.clone(),
                        changed_files: changed,
                        tasks: Vec::new(),
                    })?;
                    let names = affected_project_names(request, &affected);
                    let roots = snapshot
                        .projects
                        .iter()
                        .filter(|project| names.contains(project.name.as_str()))
                        .map(|project| project.path.clone())
                        .collect::<Vec<_>>();
                    walk_roots(&snapshot.root, &roots)?
                }
            }
        };

        let mut files = Vec::new();
        let language_filter = request.languages.iter().copied().collect::<BTreeSet<_>>();
        for path in raw_paths {
            let Some(language) = detect_language(&path) else {
                continue;
            };
            if !language_filter.is_empty() && !language_filter.contains(&language) {
                continue;
            }
            if !config
                .languages
                .get(&language)
                .is_some_and(|language_config| language_config.enabled)
            {
                continue;
            }
            let absolute = snapshot.root.join(&path);
            if !absolute.is_file() {
                continue;
            }
            files.push(SelectedSourceFile {
                path,
                absolute: absolute.as_std_path().to_path_buf(),
                language,
            });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        files.dedup_by(|left, right| left.path == right.path);
        Ok(files)
    }
}

#[derive(Clone, Debug)]
struct SelectedSourceFile {
    path: RepoRelativePath,
    absolute: PathBuf,
    language: CodeLanguage,
}

#[derive(Debug)]
enum ScanResult {
    Scanned {
        findings: Vec<CodeSizeFinding>,
        diagnostics: Vec<Diagnostic>,
    },
    Skipped {
        reason: String,
    },
    Errored {
        diagnostic: Diagnostic,
        reason: String,
    },
}

#[derive(Clone, Debug)]
struct SyntaxSpan {
    kind: SyntaxSpanKind,
    node_kind: String,
    symbol: Option<String>,
    start_line: NonZeroU32,
    end_line: NonZeroU32,
    start_byte: usize,
    end_byte: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyntaxSpanKind {
    Function,
    Block,
    Comment,
    Test,
}

#[allow(clippy::too_many_lines)]
fn scan_file(
    file: &SelectedSourceFile,
    request: &CodeSizeInspectionRequest,
    snapshot: &RepoSnapshot,
    config: &CodeSizeConfig,
) -> ScanResult {
    let metadata = match fs::metadata(&file.absolute) {
        Ok(metadata) => metadata,
        Err(source) => {
            return ScanResult::Errored {
                diagnostic: Diagnostic::warning(
                    "inspect.code_size.file_read",
                    format!("failed to read metadata: {source}"),
                )
                .with_path(file.path.to_string()),
                reason: "read_error".to_string(),
            };
        }
    };
    if metadata.len() > config.max_file_bytes.get() as u64 {
        return ScanResult::Skipped {
            reason: "file_too_large_to_scan".to_string(),
        };
    }
    let bytes = match fs::read(&file.absolute) {
        Ok(bytes) => bytes,
        Err(source) => {
            return ScanResult::Errored {
                diagnostic: Diagnostic::warning(
                    "inspect.code_size.file_read",
                    format!("failed to read file: {source}"),
                )
                .with_path(file.path.to_string()),
                reason: "read_error".to_string(),
            };
        }
    };
    if is_binary(&bytes) {
        return ScanResult::Skipped {
            reason: "binary_file_skipped".to_string(),
        };
    }
    let line_index = match LineIndex::new(&bytes) {
        Ok(index) => index,
        Err(diagnostic) => {
            return ScanResult::Errored {
                diagnostic: diagnostic.with_path(file.path.to_string()),
                reason: "line_count_overflow".to_string(),
            };
        }
    };
    let test_path = is_test_path(&file.path, file.language);
    let mut parser = Parser::new();
    if let Err(source) = parser.set_language(&tree_sitter_language(file.language, &file.path)) {
        return ScanResult::Errored {
            diagnostic: Diagnostic::warning(
                "inspect.code_size.parser_init",
                format!("failed to initialize parser: {source}"),
            )
            .with_path(file.path.to_string()),
            reason: "parser_init_error".to_string(),
        };
    }
    let Some(tree) = parser.parse(&bytes, None) else {
        return ScanResult::Errored {
            diagnostic: Diagnostic::warning(
                "inspect.code_size.syntax_error",
                "parser did not produce a syntax tree",
            )
            .with_path(file.path.to_string()),
            reason: "parse_error".to_string(),
        };
    };
    let mut diagnostics = Vec::new();
    let has_error = tree.root_node().has_error();
    let mut spans = Vec::new();
    collect_spans(
        tree.root_node(),
        &bytes,
        file.language,
        test_path,
        &line_index,
        &mut spans,
    );
    if has_error {
        diagnostics.push(
            Diagnostic::warning(
                "inspect.code_size.syntax_error",
                "source contains syntax errors; skipped function and block findings",
            )
            .with_path(file.path.to_string()),
        );
    }
    let findings = evaluate_rules(RuleEvaluation {
        file,
        request,
        snapshot,
        config,
        bytes: &bytes,
        line_index: &line_index,
        test_path,
        has_error,
        spans: &spans,
    });
    ScanResult::Scanned {
        findings,
        diagnostics,
    }
}

#[derive(Clone, Copy, Debug)]
struct RuleEvaluation<'a> {
    file: &'a SelectedSourceFile,
    request: &'a CodeSizeInspectionRequest,
    snapshot: &'a RepoSnapshot,
    config: &'a CodeSizeConfig,
    bytes: &'a [u8],
    line_index: &'a LineIndex,
    test_path: bool,
    has_error: bool,
    spans: &'a [SyntaxSpan],
}

fn evaluate_rules(input: RuleEvaluation<'_>) -> Vec<CodeSizeFinding> {
    let mut findings = Vec::new();
    let rule_filter = input.request.rules.iter().copied().collect::<BTreeSet<_>>();
    let project = project_for_path(input.snapshot, &input.file.path);
    if rule_requested(&rule_filter, CodeSizeRuleKind::File) {
        let rule = resolved_rule(input.config, &input.file.path, CodeSizeRuleKind::File);
        if rule.enabled && (rule.include_tests || !input.test_path) {
            let excluded = input
                .spans
                .iter()
                .filter(|span| {
                    span.kind == SyntaxSpanKind::Comment
                        || (!rule.include_tests && span.kind == SyntaxSpanKind::Test)
                })
                .map(|span| (span.start_byte, span.end_byte))
                .collect::<Vec<_>>();
            if let Some(effective_lines) = effective_lines(input.bytes, &excluded)
                && effective_lines > rule.max_lines.get()
            {
                let measured = nonzero_u32(effective_lines);
                findings.push(CodeSizeFinding {
                    rule: CodeSizeRuleKind::File,
                    severity: rule.severity.clone(),
                    path: input.file.path.clone(),
                    project: project.clone(),
                    language: input.file.language,
                    symbol: None,
                    node_kind: Some("source_file".to_string()),
                    start_line: NonZeroU32::MIN,
                    end_line: nonzero_u32(input.line_index.physical_lines),
                    measured_lines: measured,
                    physical_lines: Some(nonzero_u32(input.line_index.physical_lines)),
                    limit: rule.max_lines,
                    message: format!(
                        "file has {effective_lines} effective LOC, limit {}",
                        rule.max_lines
                    ),
                });
            }
        }
    }
    if input.has_error {
        return findings;
    }
    for span in input.spans {
        let rule_kind = match span.kind {
            SyntaxSpanKind::Function => CodeSizeRuleKind::Function,
            SyntaxSpanKind::Block => CodeSizeRuleKind::Block,
            SyntaxSpanKind::Comment | SyntaxSpanKind::Test => continue,
        };
        if !rule_requested(&rule_filter, rule_kind) {
            continue;
        }
        let rule = resolved_rule(input.config, &input.file.path, rule_kind);
        if !rule.enabled {
            continue;
        }
        let in_test = input.test_path || contained_in_test_range(span, input.spans);
        if in_test && !rule.include_tests {
            continue;
        }
        let measured = span.end_line.get().saturating_sub(span.start_line.get()) + 1;
        if measured <= rule.max_lines.get() {
            continue;
        }
        findings.push(CodeSizeFinding {
            rule: rule_kind,
            severity: rule.severity.clone(),
            path: input.file.path.clone(),
            project: project.clone(),
            language: input.file.language,
            symbol: span.symbol.clone(),
            node_kind: Some(span.node_kind.clone()),
            start_line: span.start_line,
            end_line: span.end_line,
            measured_lines: nonzero_u32(measured),
            physical_lines: None,
            limit: rule.max_lines,
            message: finding_message(rule_kind, span, measured, &rule),
        });
    }
    findings
}

fn collect_spans(
    node: Node<'_>,
    bytes: &[u8],
    language: CodeLanguage,
    test_path: bool,
    line_index: &LineIndex,
    spans: &mut Vec<SyntaxSpan>,
) {
    let kind = node.kind();
    if is_comment_node(kind) {
        spans.push(span_from_node(
            node,
            SyntaxSpanKind::Comment,
            None,
            line_index,
        ));
    }
    if is_test_node(node, bytes, language, test_path) {
        spans.push(span_from_node(node, SyntaxSpanKind::Test, None, line_index));
    }
    if is_function_node(node, bytes, language) {
        let span_node = function_span_node(node, language);
        spans.push(span_from_node(
            span_node,
            SyntaxSpanKind::Function,
            symbol_for_node(node, bytes, language),
            line_index,
        ));
    } else if is_block_node(node, language) && !is_direct_function_body(node, language) {
        spans.push(span_from_node(
            node,
            SyntaxSpanKind::Block,
            enclosing_symbol(node, bytes, language),
            line_index,
        ));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_spans(child, bytes, language, test_path, line_index, spans);
    }
}

fn span_from_node(
    node: Node<'_>,
    kind: SyntaxSpanKind,
    symbol: Option<String>,
    line_index: &LineIndex,
) -> SyntaxSpan {
    let start_line = line_index.line_for_start(node.start_byte());
    let end_line = line_index.line_for_end(node.end_byte());
    SyntaxSpan {
        kind,
        node_kind: node.kind().to_string(),
        symbol,
        start_line,
        end_line,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
    }
}

fn function_span_node(node: Node<'_>, language: CodeLanguage) -> Node<'_> {
    if language == CodeLanguage::Python
        && let Some(parent) = node.parent()
        && parent.kind() == "decorated_definition"
    {
        return parent;
    }
    node
}

fn is_function_node(node: Node<'_>, bytes: &[u8], language: CodeLanguage) -> bool {
    match language {
        CodeLanguage::Rust => match node.kind() {
            "function_item" => true,
            "closure_expression" => child_kind(node, "block"),
            _ => false,
        },
        CodeLanguage::TypeScript => match node.kind() {
            "function_declaration"
            | "method_definition"
            | "function_expression"
            | "generator_function_declaration"
            | "generator_function"
            | "constructor_type" => true,
            "arrow_function" => arrow_has_block_body(node, bytes),
            _ => false,
        },
        CodeLanguage::Python => node.kind() == "function_definition",
    }
}

fn is_block_node(node: Node<'_>, language: CodeLanguage) -> bool {
    match language {
        CodeLanguage::Rust => node.kind() == "block" && parent_kind_is_rust_block_owner(node),
        CodeLanguage::TypeScript => matches!(
            node.kind(),
            "statement_block" | "switch_body" | "class_static_block"
        ),
        CodeLanguage::Python => node.kind() == "block" && parent_kind_is_python_block_owner(node),
    }
}

fn is_comment_node(kind: &str) -> bool {
    kind.contains("comment")
}

fn is_test_node(node: Node<'_>, bytes: &[u8], language: CodeLanguage, test_path: bool) -> bool {
    match language {
        CodeLanguage::Rust => {
            let text = node_text(node, bytes);
            (node.kind() == "mod_item" && text.contains("cfg(test)"))
                || (node.kind() == "function_item" && text.contains("#[test]"))
        }
        CodeLanguage::TypeScript => {
            test_path
                && node.kind() == "call_expression"
                && call_name(node, bytes).is_some_and(|name| {
                    matches!(name.as_str(), "describe" | "it" | "test" | "expect")
                })
        }
        CodeLanguage::Python => {
            if node.kind() == "class_definition" {
                return field_text(node, "name", bytes)
                    .is_some_and(|name| name.starts_with("Test"));
            }
            node.kind() == "function_definition"
                && field_text(node, "name", bytes).is_some_and(|name| name.starts_with("test_"))
        }
    }
}

fn is_direct_function_body(node: Node<'_>, language: CodeLanguage) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match language {
        CodeLanguage::Rust => {
            parent.kind() == "function_item" || parent.kind() == "closure_expression"
        }
        CodeLanguage::TypeScript => matches!(
            parent.kind(),
            "function_declaration"
                | "method_definition"
                | "function_expression"
                | "generator_function_declaration"
                | "generator_function"
                | "arrow_function"
        ),
        CodeLanguage::Python => parent.kind() == "function_definition",
    }
}

fn parent_kind_is_rust_block_owner(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "if_expression"
                | "else_clause"
                | "for_expression"
                | "while_expression"
                | "loop_expression"
                | "match_arm"
                | "async_block"
                | "unsafe_block"
        )
    })
}

fn parent_kind_is_python_block_owner(node: Node<'_>) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "if_statement"
                | "elif_clause"
                | "else_clause"
                | "for_statement"
                | "while_statement"
                | "with_statement"
                | "try_statement"
                | "except_clause"
                | "finally_clause"
                | "match_statement"
                | "case_clause"
                | "class_definition"
        )
    })
}

fn child_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|child| child.kind() == kind)
}

fn arrow_has_block_body(node: Node<'_>, _bytes: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "statement_block")
}

fn symbol_for_node(node: Node<'_>, bytes: &[u8], language: CodeLanguage) -> Option<String> {
    match language {
        CodeLanguage::Rust | CodeLanguage::Python => field_text(node, "name", bytes),
        CodeLanguage::TypeScript => {
            field_text(node, "name", bytes).or_else(|| parent_name_for_expression(node, bytes))
        }
    }
}

fn enclosing_symbol(node: Node<'_>, bytes: &[u8], language: CodeLanguage) -> Option<String> {
    let mut current = node.parent();
    while let Some(parent) = current {
        if is_function_node(parent, bytes, language) {
            return symbol_for_node(parent, bytes, language);
        }
        current = parent.parent();
    }
    None
}

fn parent_name_for_expression(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    field_text(parent, "name", bytes).or_else(|| {
        let grandparent = parent.parent()?;
        field_text(grandparent, "name", bytes)
    })
}

fn field_text(node: Node<'_>, field: &str, bytes: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    node_text(child, bytes).trim().to_owned().into()
}

fn call_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let function = node.child_by_field_name("function")?;
    Some(node_text(function, bytes).trim().to_string())
}

fn node_text<'a>(node: Node<'_>, bytes: &'a [u8]) -> &'a str {
    let start = node.start_byte();
    let end = node.end_byte();
    if start >= end || end > bytes.len() {
        return "";
    }
    std::str::from_utf8(&bytes[start..end]).unwrap_or("")
}

fn tree_sitter_language(language: CodeLanguage, path: &RepoRelativePath) -> Language {
    match language {
        CodeLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
        CodeLanguage::TypeScript => {
            if path_extension(path.as_str()).is_some_and(|extension| extension == "tsx") {
                tree_sitter_typescript::LANGUAGE_TSX.into()
            } else {
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
            }
        }
        CodeLanguage::Python => tree_sitter_python::LANGUAGE.into(),
    }
}

fn detect_language(path: &RepoRelativePath) -> Option<CodeLanguage> {
    match path_extension(path.as_str()).as_deref() {
        Some("rs") => Some(CodeLanguage::Rust),
        Some("ts" | "tsx" | "mts" | "cts") => Some(CodeLanguage::TypeScript),
        Some("py" | "pyi") => Some(CodeLanguage::Python),
        Some(_) | None => None,
    }
}

fn path_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

fn is_test_path(path: &RepoRelativePath, language: CodeLanguage) -> bool {
    let value = path.as_str();
    match language {
        CodeLanguage::Rust => {
            value.starts_with("tests/")
                || value.contains("/tests/")
                || value.starts_with("benches/")
                || value.contains("/benches/")
                || value.ends_with("_test.rs")
        }
        CodeLanguage::TypeScript => {
            value.starts_with("__tests__/")
                || value.contains("/__tests__/")
                || value.ends_with(".test.ts")
                || value.ends_with(".test.tsx")
                || value.ends_with(".spec.ts")
                || value.ends_with(".spec.tsx")
        }
        CodeLanguage::Python => {
            value.starts_with("tests/")
                || value.contains("/tests/")
                || value
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with("test_") || name.ends_with("_test.py"))
        }
    }
}

fn changed_files(
    request: &CodeSizeInspectionRequest,
    root: &RepoRoot,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<RepoRelativePath>, RepoctlError> {
    if !request.changed_files.is_empty() {
        return Ok(request.changed_files.clone());
    }
    let (Some(base), Some(head)) = (&request.base, &request.head) else {
        diagnostics.push(Diagnostic::warning(
            "inspect.code_size.no_base_head",
            "changed or affected code-size inspection requires --base and --head or --changed-file",
        ));
        return Ok(Vec::new());
    };
    let output = Command::new("git")
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=ACMR")
        .arg(base)
        .arg(head)
        .current_dir(root.absolute.as_std_path())
        .output()
        .map_err(|source| {
            RepoctlError::Environment(format!("failed to execute git diff: {source}"))
        })?;
    if !output.status.success() {
        return Err(RepoctlError::Environment(format!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| RepoRelativePath::new(line.to_string()).map_err(RepoctlError::diagnostic))
        .collect()
}

fn affected_project_names(
    request: &CodeSizeInspectionRequest,
    affected: &repoctl_core::AffectedReport,
) -> BTreeSet<String> {
    let mut names = affected
        .directly_affected
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    if request.include_transitive {
        names.extend(
            affected
                .transitively_affected
                .iter()
                .map(ToString::to_string),
        );
    }
    names
}

fn walk_roots(
    root: &RepoRoot,
    prefixes: &[RepoRelativePath],
) -> Result<Vec<RepoRelativePath>, RepoctlError> {
    let mut files = Vec::new();
    for prefix in prefixes {
        let start = root.join(prefix);
        if !start.exists() {
            continue;
        }
        let mut builder = WalkBuilder::new(start.as_std_path());
        builder
            .hidden(false)
            .parents(true)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .filter_entry(|entry| !is_excluded_entry(entry));
        for entry in builder.build() {
            let entry = entry.map_err(|error| {
                RepoctlError::Environment(format!("failed to walk repository: {error}"))
            })?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            let relative = strip_repo_prefix(root, entry.path())?;
            if is_heavy_path(&relative) {
                continue;
            }
            files.push(relative);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn is_excluded_entry(entry: &DirEntry) -> bool {
    entry
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| HEAVY_DIR_NAMES.contains(&name))
}

fn is_heavy_path(path: &RepoRelativePath) -> bool {
    path.as_str()
        .split('/')
        .any(|part| HEAVY_DIR_NAMES.contains(&part))
}

fn strip_repo_prefix(root: &RepoRoot, path: &Path) -> Result<RepoRelativePath, RepoctlError> {
    let relative = path
        .strip_prefix(root.absolute.as_std_path())
        .map_err(|error| {
            RepoctlError::Environment(format!("walked path escaped repository root: {error}"))
        })?;
    RepoRelativePath::new(relative.to_string_lossy().to_string()).map_err(RepoctlError::diagnostic)
}

fn compile_globs(globs: &[RepoGlob]) -> Result<GlobSet, RepoctlError> {
    let mut builder = GlobSetBuilder::new();
    for glob in globs {
        builder.add(Glob::new(glob.as_str()).map_err(|source| {
            RepoctlError::diagnostic(Diagnostic::error(
                "inspect.code_size.config_invalid",
                format!("invalid inspection glob `{}`: {source}", glob.as_str()),
            ))
        })?);
    }
    builder.build().map_err(|source| {
        RepoctlError::diagnostic(Diagnostic::error(
            "inspect.code_size.config_invalid",
            format!("failed to compile inspection globs: {source}"),
        ))
    })
}

fn compile_generated_globs(snapshot: &RepoSnapshot) -> Result<GlobSet, RepoctlError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in GENERATED_PATTERNS {
        builder.add(Glob::new(pattern).map_err(|source| {
            RepoctlError::diagnostic(Diagnostic::error(
                "inspect.code_size.config_invalid",
                format!("invalid built-in generated-code glob `{pattern}`: {source}"),
            ))
        })?);
    }
    for glob in snapshot.projects.iter().flat_map(|project| {
        project
            .ai
            .do_not_edit
            .iter()
            .map(|glob| project_generated_pattern(project, glob))
    }) {
        builder.add(Glob::new(&glob).map_err(|source| {
            RepoctlError::diagnostic(Diagnostic::error(
                "inspect.code_size.config_invalid",
                format!("invalid generated-code glob `{glob}`: {source}"),
            ))
        })?);
    }
    builder.build().map_err(|source| {
        RepoctlError::diagnostic(Diagnostic::error(
            "inspect.code_size.config_invalid",
            format!("failed to compile generated-code globs: {source}"),
        ))
    })
}

fn project_generated_pattern(project: &ProjectManifest, glob: &RepoGlob) -> String {
    if glob.as_str().starts_with("**/") || project.path.as_str() == "." {
        glob.as_str().to_string()
    } else {
        format!("{}/{}", project.path, glob.as_str())
    }
}

fn resolved_rule(
    config: &CodeSizeConfig,
    path: &RepoRelativePath,
    rule: CodeSizeRuleKind,
) -> CodeSizeRuleConfig {
    let mut resolved = config.rules.get(rule).clone();
    for override_config in &config.overrides {
        let matches =
            compile_globs(&override_config.paths).is_ok_and(|set| set.is_match(path.as_str()));
        if matches && let Some(patch) = override_config.rules.get(rule) {
            apply_rule_patch(&mut resolved, patch);
        }
    }
    resolved
}

fn apply_rule_patch(rule: &mut CodeSizeRuleConfig, patch: &CodeSizeRuleConfigPatch) {
    if let Some(enabled) = patch.enabled {
        rule.enabled = enabled;
    }
    if let Some(max_lines) = patch.max_lines {
        rule.max_lines = max_lines;
    }
    if let Some(severity) = &patch.severity {
        rule.severity = severity.clone();
    }
    if let Some(include_tests) = patch.include_tests {
        rule.include_tests = include_tests;
    }
}

fn rule_requested(filter: &BTreeSet<CodeSizeRuleKind>, rule: CodeSizeRuleKind) -> bool {
    filter.is_empty() || filter.contains(&rule)
}

fn project_for_path(
    snapshot: &RepoSnapshot,
    path: &RepoRelativePath,
) -> Option<repoctl_core::ProjectName> {
    snapshot
        .projects
        .iter()
        .find(|project| project.contains_path(path))
        .map(|project| project.name.clone())
}

fn contained_in_test_range(span: &SyntaxSpan, spans: &[SyntaxSpan]) -> bool {
    spans.iter().any(|test| {
        test.kind == SyntaxSpanKind::Test
            && span.start_byte >= test.start_byte
            && span.end_byte <= test.end_byte
    })
}

fn finding_message(
    rule: CodeSizeRuleKind,
    span: &SyntaxSpan,
    measured: u32,
    config: &CodeSizeRuleConfig,
) -> String {
    match rule {
        CodeSizeRuleKind::File => {
            format!(
                "file has {measured} effective LOC, limit {}",
                config.max_lines
            )
        }
        CodeSizeRuleKind::Function => span.symbol.as_ref().map_or_else(
            || {
                format!(
                    "function-like `{}` spans {measured} lines, limit {}",
                    span.node_kind, config.max_lines
                )
            },
            |symbol| {
                format!(
                    "function {symbol} spans {measured} lines, limit {}",
                    config.max_lines
                )
            },
        ),
        CodeSizeRuleKind::Block => span.symbol.as_ref().map_or_else(
            || {
                format!(
                    "block `{}` spans {measured} lines, limit {}",
                    span.node_kind, config.max_lines
                )
            },
            |symbol| {
                format!(
                    "block in {symbol} spans {measured} lines, limit {}",
                    config.max_lines
                )
            },
        ),
    }
}

#[derive(Clone, Debug)]
struct LineIndex {
    starts: Vec<usize>,
    physical_lines: u32,
}

impl LineIndex {
    fn new(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let mut starts = Vec::with_capacity(memchr_iter(b'\n', bytes).count().saturating_add(1));
        starts.push(0);
        for newline in memchr_iter(b'\n', bytes) {
            let next = newline.saturating_add(1);
            if next < bytes.len() {
                starts.push(next);
            }
        }
        let physical_lines = if bytes.is_empty() {
            0
        } else {
            u32::try_from(starts.len()).map_err(|_| {
                Diagnostic::warning(
                    "inspect.code_size.line_count_overflow",
                    "file line count exceeds supported range",
                )
            })?
        };
        Ok(Self {
            starts,
            physical_lines,
        })
    }

    fn line_for_start(&self, byte: usize) -> NonZeroU32 {
        self.line_for_byte(byte)
    }

    fn line_for_end(&self, byte: usize) -> NonZeroU32 {
        if byte == 0 {
            return NonZeroU32::MIN;
        }
        self.line_for_byte(byte.saturating_sub(1))
    }

    fn line_for_byte(&self, byte: usize) -> NonZeroU32 {
        let index = match self.starts.binary_search(&byte) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        nonzero_u32(u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX))
    }
}

fn effective_lines(bytes: &[u8], excluded_ranges: &[(usize, usize)]) -> Option<u32> {
    if bytes.is_empty() {
        return Some(0);
    }
    let ranges = normalized_ranges(excluded_ranges, bytes.len());
    let mut range_index = 0_usize;
    let mut count = 0_u32;
    let mut line_has_code = false;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            if line_has_code {
                count = count.checked_add(1)?;
            }
            line_has_code = false;
            continue;
        }
        while ranges
            .get(range_index)
            .is_some_and(|(_start, end)| index >= *end)
        {
            range_index = range_index.saturating_add(1);
        }
        let excluded = ranges
            .get(range_index)
            .is_some_and(|(start, end)| index >= *start && index < *end);
        if !excluded && !byte.is_ascii_whitespace() {
            line_has_code = true;
        }
    }
    if line_has_code {
        count = count.checked_add(1)?;
    }
    Some(count)
}

fn normalized_ranges(ranges: &[(usize, usize)], len: usize) -> Vec<(usize, usize)> {
    let mut normalized = ranges
        .iter()
        .filter_map(|(start, end)| {
            let start = (*start).min(len);
            let end = (*end).min(len);
            (start < end).then_some((start, end))
        })
        .collect::<Vec<_>>();
    normalized.sort_unstable();
    let mut merged = Vec::<(usize, usize)>::new();
    for (start, end) in normalized {
        if let Some((_last_start, last_end)) = merged.last_mut()
            && start <= *last_end
        {
            *last_end = (*last_end).max(end);
            continue;
        }
        merged.push((start, end));
    }
    merged
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .take(BINARY_PREFIX_BYTES)
        .any(|byte| *byte == b'\0')
}

fn sort_findings(findings: &mut [CodeSizeFinding]) {
    findings.sort_by(|left, right| {
        (
            left.path.as_str(),
            left.start_line,
            rule_rank(left.rule),
            left.symbol.as_deref().unwrap_or(""),
            left.node_kind.as_deref().unwrap_or(""),
        )
            .cmp(&(
                right.path.as_str(),
                right.start_line,
                rule_rank(right.rule),
                right.symbol.as_deref().unwrap_or(""),
                right.node_kind.as_deref().unwrap_or(""),
            ))
    });
}

fn rule_rank(rule: CodeSizeRuleKind) -> u8 {
    match rule {
        CodeSizeRuleKind::File => 0,
        CodeSizeRuleKind::Function => 1,
        CodeSizeRuleKind::Block => 2,
    }
}

fn report(
    request: &CodeSizeInspectionRequest,
    config: &CodeSizeConfig,
    mut summary: CodeSizeInspectionSummary,
    findings: Vec<CodeSizeFinding>,
    diagnostics: Vec<Diagnostic>,
    skipped: BTreeMap<String, u64>,
    started: Instant,
) -> CodeSizeInspectionReport {
    summary.duration_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    CodeSizeInspectionReport {
        scope: request.scope,
        base: request.base.clone(),
        head: request.head.clone(),
        summary,
        config: CodeSizeResolvedConfigSummary {
            enabled: config.enabled,
            generated_code: config.generated_code,
            max_files: config.max_files,
            max_file_bytes: config.max_file_bytes,
            rules: config.rules.clone(),
        },
        findings,
        diagnostics,
        skipped: skipped
            .into_iter()
            .map(|(reason, count)| CodeSizeSkippedReason { reason, count })
            .collect(),
    }
}

fn increment_skip(skipped: &mut BTreeMap<String, u64>, reason: &str) {
    let count = skipped.entry(reason.to_string()).or_insert(0);
    *count = count.saturating_add(1);
}

fn nonzero_u32(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap_or(NonZeroU32::MIN)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path};

    use repoctl_core::{
        CodeLanguage, CodeSizeInspectionRequest, CodeSizeRuleKind, CodeSizeScope, ProjectAiSpec,
        ProjectAreas, ProjectDnsSpec, ProjectKind, ProjectManifest, ProjectName, ProjectOpsSpec,
        ProjectProtoSpec, RepoGlob, RepoRelativePath, SchemaId, Visibility,
    };
    use tree_sitter::Parser;

    use super::{
        InspectorService, LineIndex, SyntaxSpan, SyntaxSpanKind, collect_spans, effective_lines,
        is_binary, is_test_path, project_generated_pattern, tree_sitter_language,
    };

    #[test]
    fn test_should_count_effective_lines_without_comments() {
        let bytes = b"fn main() {\n  // comment\n  println!(\"hi\");\n}\n";
        let effective = effective_lines(bytes, &[(14, 24)]);
        assert_eq!(effective, Some(3));
    }

    #[test]
    fn test_should_merge_excluded_ranges_for_effective_lines() {
        let bytes = b"alpha\nbeta\ngamma\n";
        let effective = effective_lines(bytes, &[(0, 3), (2, 7), (11, 16)]);
        assert_eq!(effective, Some(1));
    }

    #[test]
    fn test_should_count_single_line_without_final_newline() {
        let index = LineIndex::new(b"let a = 1;");
        assert!(index.is_ok());
        assert_eq!(index.ok().map(|value| value.physical_lines), Some(1));
    }

    #[test]
    fn test_should_detect_binary_prefix() {
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary(b"abc\ndef"));
    }

    #[test]
    fn test_should_collect_rust_function_and_nested_block() {
        let source = b"fn main() {\nif true {\nprintln!(\"hi\");\n}\n}\n";
        let spans = spans_for(
            source,
            CodeLanguage::Rust,
            &RepoRelativePath::new("src/main.rs").ok(),
        );
        assert!(spans.iter().any(|span| {
            span.kind == SyntaxSpanKind::Function && span.symbol.as_deref() == Some("main")
        }));
        assert!(spans.iter().any(|span| {
            span.kind == SyntaxSpanKind::Block && span.symbol.as_deref() == Some("main")
        }));
    }

    #[test]
    fn test_should_parse_tsx_with_tsx_grammar() {
        let path = RepoRelativePath::new("apps/web/src/component.tsx").ok();
        let source = b"export function Card() {\nreturn <div />;\n}\n";
        let spans = spans_for(source, CodeLanguage::TypeScript, &path);
        assert!(spans.iter().any(|span| {
            span.kind == SyntaxSpanKind::Function && span.symbol.as_deref() == Some("Card")
        }));
    }

    #[test]
    fn test_should_collect_typescript_expect_test_range() {
        let path = RepoRelativePath::new("apps/web/src/component.test.ts").ok();
        let source = b"expect(() => {\n  throw new Error('x');\n}).toThrow();\n";
        let spans = spans_for(source, CodeLanguage::TypeScript, &path);
        assert!(spans.iter().any(|span| span.kind == SyntaxSpanKind::Test));
    }

    #[test]
    fn test_should_include_python_decorators_in_function_span() {
        let path = RepoRelativePath::new("service/app.py").ok();
        let source = b"@route('/x')\n@auth\nasync def handler():\n    return True\n";
        let spans = spans_for(source, CodeLanguage::Python, &path);
        let handler = spans
            .iter()
            .find(|span| {
                span.kind == SyntaxSpanKind::Function && span.symbol.as_deref() == Some("handler")
            })
            .expect("handler function span exists");
        assert_eq!(handler.start_line.get(), 1);
        assert_eq!(handler.end_line.get(), 4);
    }

    #[test]
    fn test_should_collect_python_test_ranges() {
        let source = b"def test_should_work():\n    assert True\n";
        let path = RepoRelativePath::new("tests/test_app.py").ok();
        let spans = spans_for(source, CodeLanguage::Python, &path);
        assert!(spans.iter().any(|span| span.kind == SyntaxSpanKind::Test));
        assert!(
            path.as_ref()
                .is_some_and(|value| { is_test_path(value, CodeLanguage::Python) })
        );
    }

    #[test]
    fn test_should_prefix_project_local_generated_globs() {
        let project = project_with_do_not_edit("apps/catalog", "deploy/prod/**");
        assert_eq!(
            project_generated_pattern(&project, &project.ai.do_not_edit[0]),
            "apps/catalog/deploy/prod/**"
        );
    }

    #[test]
    fn test_should_keep_global_generated_globs() {
        let project = project_with_do_not_edit("apps/catalog", "**/generated/**");
        assert_eq!(
            project_generated_pattern(&project, &project.ai.do_not_edit[0]),
            "**/generated/**"
        );
    }

    #[test]
    fn test_should_scan_temp_repo_and_apply_override() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(
            temp.path(),
            r#"
inspection:
  code_size:
    rules:
      function:
        max_lines: 2
    overrides:
      - paths:
          - "apps/catalog/src/lib.rs"
        rules:
          function:
            max_lines: 20
        reason: "fixture override"
"#,
        );
        write_project(temp.path(), "apps/catalog", "");
        write_file(
            temp.path(),
            "apps/catalog/src/lib.rs",
            "pub fn allowed() {\nlet a = 1;\nlet b = 2;\nlet _c = a + b;\n}\n",
        );

        let report = inspect_temp_repo(temp.path(), CodeSizeScope::All, Vec::new());

        assert!(report.diagnostics.is_empty());
        assert!(report.findings.is_empty());
        assert_eq!(report.summary.files_scanned, 1);
    }

    #[test]
    fn test_should_skip_project_generated_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(temp.path(), "");
        write_project(
            temp.path(),
            "apps/catalog",
            r#"
ai:
  do_not_edit:
    - "generated/**"
"#,
        );
        write_file(
            temp.path(),
            "apps/catalog/generated/lib.rs",
            "pub fn generated() {\nlet a = 1;\nlet b = 2;\nlet _c = a + b;\n}\n",
        );

        let report = inspect_temp_repo(temp.path(), CodeSizeScope::All, Vec::new());

        assert!(report.findings.is_empty());
        assert_eq!(report.summary.files_scanned, 0);
        assert!(
            report
                .skipped
                .iter()
                .any(|item| { item.reason == "generated_code" && item.count == 1 })
        );
    }

    #[test]
    fn test_should_report_file_size_when_parse_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(
            temp.path(),
            r#"
inspection:
  code_size:
    rules:
      file:
        max_lines: 1
"#,
        );
        write_project(temp.path(), "apps/catalog", "");
        write_file(
            temp.path(),
            "apps/catalog/src/lib.rs",
            "pub fn broken( {\nlet x = 1;\n",
        );

        let report = inspect_temp_repo(temp.path(), CodeSizeScope::All, Vec::new());

        assert!(report.findings.iter().any(|finding| {
            finding.rule == CodeSizeRuleKind::File
                && finding.path.as_str() == "apps/catalog/src/lib.rs"
        }));
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code.as_ref() == "inspect.code_size.syntax_error" })
        );
        assert!(!report.findings.iter().any(|finding| {
            matches!(
                finding.rule,
                CodeSizeRuleKind::Function | CodeSizeRuleKind::Block
            )
        }));
    }

    #[test]
    fn test_should_scan_explicit_changed_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_repo_manifest(
            temp.path(),
            r#"
inspection:
  code_size:
    rules:
      function:
        max_lines: 2
"#,
        );
        write_project(temp.path(), "apps/catalog", "");
        write_file(
            temp.path(),
            "apps/catalog/src/lib.rs",
            "pub fn changed() {\nlet a = 1;\nlet b = 2;\nlet _c = a + b;\n}\n",
        );

        let report = inspect_temp_repo(
            temp.path(),
            CodeSizeScope::Changed,
            vec![RepoRelativePath::new("apps/catalog/src/lib.rs").expect("changed file")],
        );

        assert!(report.findings.iter().any(|finding| {
            finding.rule == CodeSizeRuleKind::Function
                && finding.symbol.as_deref() == Some("changed")
        }));
        assert_eq!(report.summary.files_scanned, 1);
    }

    fn spans_for(
        source: &[u8],
        language: CodeLanguage,
        path: &Option<RepoRelativePath>,
    ) -> Vec<SyntaxSpan> {
        let fallback = RepoRelativePath::new("src/file.rs").ok();
        let path = path.as_ref().or(fallback.as_ref());
        let mut parser = Parser::new();
        if let Some(path) = path {
            let language_impl = tree_sitter_language(language, path);
            assert!(parser.set_language(&language_impl).is_ok());
        }
        let tree = parser.parse(source, None);
        let index = LineIndex::new(source);
        let mut spans = Vec::new();
        if let (Some(tree), Ok(index), Some(path)) = (tree, index, path) {
            collect_spans(
                tree.root_node(),
                source,
                language,
                is_test_path(path, language),
                &index,
                &mut spans,
            );
        }
        spans
    }

    fn project_with_do_not_edit(path: &str, glob: &str) -> ProjectManifest {
        ProjectManifest {
            schema: SchemaId::new("company.project/v1").expect("schema"),
            name: ProjectName::new("apps.catalog").expect("project"),
            kind: ProjectKind::App,
            path: RepoRelativePath::new(path).expect("path"),
            owners: Vec::new(),
            visibility: Visibility::Internal,
            workspaces: Vec::new(),
            depends_on: Vec::new(),
            tasks: BTreeMap::new(),
            iac: None,
            deploy: None,
            dns: ProjectDnsSpec::default(),
            cdn: None,
            ops: ProjectOpsSpec::default(),
            protos: ProjectProtoSpec::default(),
            ai: ProjectAiSpec {
                editable: Vec::new(),
                do_not_edit: vec![RepoGlob::new(glob).expect("glob")],
                docs: Vec::new(),
            },
            areas: ProjectAreas::default(),
            policies: BTreeMap::new(),
            source: RepoRelativePath::new("apps/catalog/project.yaml").expect("source"),
        }
    }

    fn inspect_temp_repo(
        root: &Path,
        scope: CodeSizeScope,
        changed_files: Vec<RepoRelativePath>,
    ) -> repoctl_core::CodeSizeInspectionReport {
        InspectorService::with_default_adapters()
            .inspect_size(&CodeSizeInspectionRequest {
                repo: Some(root.to_path_buf()),
                scope,
                changed_files,
                ..CodeSizeInspectionRequest::default()
            })
            .expect("inspect succeeds")
    }

    fn write_repo_manifest(root: &Path, inspection: &str) {
        fs::write(
            root.join("repo.yaml"),
            format!(
                r#"
schema: company.repo/v1
name: acme
layout: functional
defaults:
  owner: "@platform"
{inspection}
"#
            ),
        )
        .expect("repo manifest");
    }

    fn write_project(root: &Path, relative: &str, extra: &str) {
        let dir = root.join(relative);
        fs::create_dir_all(&dir).expect("project dir");
        fs::write(
            dir.join("project.yaml"),
            format!(
                r#"
schema: company.project/v1
name: apps.catalog
kind: app
path: {relative}
owners:
  - "@catalog"
workspaces:
  - name: api
    language: rust
    root: .
    manifest: Cargo.toml
{extra}
"#
            ),
        )
        .expect("project manifest");
    }

    fn write_file(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("source dir");
        }
        fs::write(path, content).expect("source file");
    }
}
