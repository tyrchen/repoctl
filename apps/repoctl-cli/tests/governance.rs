//! End-to-end regression tests using real argv execution and isolated repositories.
// Synchronous process fixtures are deliberate; these tests do not run inside Tokio.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::{
    error::Error,
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

mod regressions;

type TestResult = Result<(), Box<dyn Error>>;

fn fixture() -> Result<TempDir, Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("repo.yaml"),
        "schema: company.repo/v1\nname: review\nlayout: functional\ndefaults:\n  owner: \
         \"@platform\"\n",
    )?;
    Ok(temp)
}

fn write(root: &Path, relative: &str, content: &str) -> Result<(), Box<dyn Error>> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn app(root: &Path, tasks: &str) -> Result<(), Box<dyn Error>> {
    write(root, "apps/catalog/project.yaml", &format!("schema: company.project/v1\nname: apps.catalog\nkind: app\npath: apps/catalog\nworkspaces:\n  - name: api\n    language: python\n    toolchain: uv\n    root: .\n    manifest: pyproject.toml\n{tasks}"))
}

fn cli(root: &Path, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(args)
        .arg("--repo")
        .arg(root)
        .args(["--format", "json"])
        .output()?)
}

fn json(output: &Output) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn test_should_wait_for_all_prerequisite_commands_at_high_concurrency() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        "tasks:\n  prepare:\n    - workspace: api\n      command: python3 prepare.py first\n    - \
         workspace: api\n      command: python3 prepare.py second\n  check:\n    - workspace: \
         api\n      command: python3 check.py\n      depends_on:\n        - project: \
         apps.catalog\n          workspace: api\n          task: prepare\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/prepare.py",
        r"import pathlib,time,sys
time.sleep(0.05)
pathlib.Path(sys.argv[1]).write_text('ready')
",
    )?;
    write(
        temp.path(),
        "apps/catalog/check.py",
        "import pathlib,sys\nsys.exit(0 if pathlib.Path('first').exists() and \
         pathlib.Path('second').exists() else 7)\n",
    )?;
    let output = cli(temp.path(), &["run", "check", "--concurrency", "8"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    assert_eq!(data["outputs"].as_array().map(Vec::len), Some(3));
    Ok(())
}

#[test]
fn test_should_skip_failed_descendants_and_continue_independent_tasks() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        "tasks:\n  prepare:\n    - workspace: api\n      command: python3 fail.py\n  check:\n    \
         - workspace: api\n      command: python3 mark.py\n      depends_on:\n        - project: \
         apps.catalog\n          workspace: api\n          task: prepare\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/fail.py",
        "import sys\nsys.exit(7)\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/mark.py",
        "import pathlib\npathlib.Path('should-not-exist').write_text('bad')\n",
    )?;
    write(temp.path(), "apps/other/project.yaml", "schema: company.project/v1\nname: apps.other\nkind: app\npath: apps/other\nworkspaces:\n  - name: api\n    language: python\n    root: .\n    manifest: pyproject.toml\ntasks:\n  check:\n    - workspace: api\n      command: python3 ok.py\n")?;
    write(
        temp.path(),
        "apps/other/ok.py",
        "import pathlib\npathlib.Path('passed').write_text('ok')\n",
    )?;
    let output = cli(temp.path(), &["run", "check", "--concurrency", "2"])?;
    assert!(!output.status.success());
    assert!(temp.path().join("apps/other/passed").exists());
    assert!(!temp.path().join("apps/catalog/should-not-exist").exists());
    assert!(
        json(&output)?["diagnostics"]
            .as_array()
            .is_some_and(|d| d.iter().any(|d| d["code"] == "task.skipped"))
    );
    Ok(())
}

#[test]
fn test_should_fail_bounded_execution_on_deadline_and_output_overflow() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "tasks:\n  timeout:\n    - workspace: api\n      command: python3 slow.py\n      timeout_seconds: 1\n  overflow:\n    - workspace: api\n      command: python3 noisy.py\n      max_output_bytes: 32\n")?;
    write(
        temp.path(),
        "apps/catalog/slow.py",
        "import time\ntime.sleep(10)\n",
    )?;
    write(temp.path(), "apps/catalog/noisy.py", "print('x' * 4096)\n")?;
    for (task, status) in [("timeout", 124), ("overflow", 125)] {
        let output = cli(temp.path(), &["run", task])?;
        assert!(!output.status.success());
        assert_eq!(json(&output)?["outputs"][0]["output"]["status"], status);
    }
    Ok(())
}

#[test]
fn test_should_restore_cached_artifacts_and_invalidate_changed_inputs() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "tasks:\n  build:\n    - workspace: api\n      command: python3 build.py\n      cache:\n        enabled: true\n        inputs: [apps/catalog/src/**]\n        outputs: [apps/catalog/dist]\n")?;
    write(temp.path(), "apps/catalog/src/input", "first")?;
    write(
        temp.path(),
        "apps/catalog/build.py",
        "import pathlib\np=pathlib.Path('dist')\np.mkdir(exist_ok=True)\nc=p/'count'\nc.\
         write_text(str(int(c.read_text())+1) if c.exists() else \
         '1')\n(p/'artifact').write_text(pathlib.Path('src/input').read_text())\n",
    )?;
    let first = cli(temp.path(), &["run", "build"])?;
    assert!(
        first.status.success(),
        "{}",
        format!(
            "{} {}",
            String::from_utf8_lossy(&first.stderr),
            String::from_utf8_lossy(&first.stdout)
        )
    );
    fs::remove_dir_all(temp.path().join("apps/catalog/dist"))?;
    let hit = cli(temp.path(), &["run", "build"])?;
    assert!(hit.status.success());
    assert_eq!(json(&hit)?["outputs"][0]["output"]["cached"], true);
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/count"))?,
        "1"
    );
    write(temp.path(), "apps/catalog/src/input", "second")?;
    let miss = cli(temp.path(), &["run", "build"])?;
    assert_eq!(json(&miss)?["outputs"][0]["output"]["cached"], false);
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/artifact"))?,
        "second"
    );
    let bypass = cli(temp.path(), &["run", "build", "--no-cache"])?;
    assert!(bypass.status.success());
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/count"))?,
        "3"
    );
    Ok(())
}

#[test]
fn test_should_propagate_infrastructure_changes_with_reviewers_and_evidence() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "depends_on: [core-infra.network]\n")?;
    write(
        temp.path(),
        "core-infra/network/project.yaml",
        "schema: company.project/v1\nname: core-infra.network\nkind: core-infra-component\npath: \
         core-infra/network\nowners: [\"@network\"]\n",
    )?;
    let output = cli(
        temp.path(),
        &["affected", "--changed-file", "core-infra/network/main.tf"],
    )?;
    assert!(output.status.success());
    let data = json(&output)?;
    assert_eq!(data["transitivelyAffected"][0], "apps.catalog");
    assert!(
        data["suggestedReviewers"]
            .as_array()
            .is_some_and(|r| r.contains(&Value::String("@platform".to_string())))
    );
    assert!(
        data["reasons"].as_array().is_some_and(|r| r
            .iter()
            .any(|r| r["edgeKind"] == "uses-core-infra-module"
                && r["evidence"] == "core-infra.network"))
    );
    Ok(())
}

#[test]
fn test_should_discover_node_and_normalized_python_dependencies_offline() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        "schema: company.project/v1\nname: apps.catalog\nkind: app\npath: \
         apps/catalog\nworkspaces:\n  - name: web\n    language: typescript\n    root: web\n    \
         manifest: web/package.json\n  - name: jobs\n    language: python\n    root: jobs\n    \
         manifest: jobs/pyproject.toml\n",
    )?;
    write(temp.path(), "frameworks/shared/project.yaml", "schema: company.project/v1\nname: frameworks.shared\nkind: framework\npath: frameworks/shared\nworkspaces:\n  - name: web\n    language: typescript\n    root: web\n    manifest: web/package.json\n  - name: jobs\n    language: python\n    root: jobs\n    manifest: jobs/pyproject.toml\npublic_facades:\n  typescript: [web]\n  python: [jobs]\n")?;
    write(
        temp.path(),
        "apps/catalog/web/package.json",
        r#"{"name":"catalog","dependencies":{"@company/shared":"workspace:*"}}"#,
    )?;
    write(
        temp.path(),
        "frameworks/shared/web/package.json",
        r#"{"name":"@company/shared"}"#,
    )?;
    write(
        temp.path(),
        "apps/catalog/jobs/pyproject.toml",
        "[project]\nname='catalog-jobs'\ndependencies=['Shared_Lib>=1']\n",
    )?;
    write(
        temp.path(),
        "frameworks/shared/jobs/pyproject.toml",
        "[project]\nname='shared-lib'\n",
    )?;
    let output = cli(temp.path(), &["graph", "print"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    let edges = data["snapshot"]["graph"]["edges"]
        .as_array()
        .ok_or("missing graph edges")?;
    assert_eq!(
        edges
            .iter()
            .filter(
                |e| e["from"] == "project:apps.catalog" && e["to"] == "project:frameworks.shared"
            )
            .count(),
        2
    );
    Ok(())
}

#[test]
fn test_should_generate_standalone_executable_ci_with_a_real_matrix() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        "tasks:\n  check:\n    - workspace: api\n      command: python3 check.py\n",
    )?;
    write(temp.path(), "apps/catalog/check.py", "print('checked')\n")?;
    let output = cli(temp.path(), &["ci", "workflow"])?;
    assert!(output.status.success());
    let report = json(&output)?;
    let text = report["content"]
        .as_str()
        .ok_or("workflow content missing")?;
    let workflow: Value = serde_norway::from_str(text)?;
    assert!(text.contains("cargo install repoctl-cli --version '=0.7.0' --locked"));
    assert!(!text.contains("--dry-run"));
    assert!(!text.contains("--path apps/repoctl-cli"));
    assert_eq!(
        workflow["jobs"]["tasks"]["strategy"]["matrix"],
        "${{ fromJson(needs.graph.outputs.matrix) }}"
    );
    assert!(text.contains("test \"$TASKS\" = success"));
    let matrix = cli(temp.path(), &["ci", "matrix", "--fallback", "all"])?;
    assert_eq!(json(&matrix)?["entries"][0]["needsPython"], true);
    let step = cli(
        temp.path(),
        &[
            "ci",
            "run-step",
            "--project",
            "apps.catalog",
            "--workspace",
            "api",
            "--task",
            "check",
        ],
    )?;
    assert!(step.status.success());
    assert!(
        json(&step)?["outputs"][0]["output"]["stdout"]
            .as_str()
            .is_some_and(|s| s.contains("checked"))
    );
    Ok(())
}

#[test]
fn test_should_bound_context_and_exclude_protected_source() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        "ai:\n  docs: [README.md]\n  do_not_edit: [private/**]\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/README.md",
        "Project documentation",
    )?;
    write(
        temp.path(),
        "apps/catalog/source.py",
        &"中文内容\n".repeat(2000),
    )?;
    write(
        temp.path(),
        "apps/catalog/private/secret.py",
        "never include this",
    )?;
    let output = cli(
        temp.path(),
        &[
            "context",
            "apps.catalog",
            "--changed-file",
            "apps/catalog/source.py",
            "--changed-file",
            "apps/catalog/private/secret.py",
            "--include-docs",
            "--max-bytes",
            "4096",
        ],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 4097);
    let data = json(&output)?;
    assert!(!String::from_utf8_lossy(&output.stdout).contains("never include this"));
    assert!(
        data["payload"]["snippets"]
            .as_array()
            .or_else(|| data["snippets"].as_array())
            .is_some_and(|s| !s.is_empty())
    );
    Ok(())
}

#[test]
fn test_should_reject_cycles_and_missing_prerequisites() -> TestResult {
    let temp = fixture()?;
    for dependency in ["check", "missing"] {
        app(
            temp.path(),
            &format!(
                "tasks:\n  check:\n    - workspace: api\n      command: python3 check.py\n      \
                 depends_on:\n        - project: apps.catalog\n          workspace: api\n          \
                 task: {dependency}\n"
            ),
        )?;
        let output = cli(temp.path(), &["run", "check", "--dry-run"])?;
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(if dependency == "check" {
            "task.dependency_cycle"
        } else {
            "task.dependency_task_missing"
        }));
    }
    Ok(())
}

#[test]
fn test_should_run_independent_ready_nodes_concurrently() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        "schema: company.project/v1\nname: apps.catalog\nkind: app\npath: \
         apps/catalog\nworkspaces:\n  - name: one\n    language: python\n    root: one\n    \
         manifest: one/pyproject.toml\n  - name: two\n    language: python\n    root: two\n    \
         manifest: two/pyproject.toml\ntasks:\n  check:\n    - workspace: one\n      command: \
         python3 ../barrier.py one two\n    - workspace: two\n      command: python3 \
         ../barrier.py two one\n",
    )?;
    fs::create_dir_all(temp.path().join("apps/catalog/one"))?;
    fs::create_dir_all(temp.path().join("apps/catalog/two"))?;
    write(
        temp.path(),
        "apps/catalog/barrier.py",
        "import pathlib,sys,time\nr=pathlib.Path(__file__).parent\n(r/sys.argv[1]/'ready').\
         touch()\nfor _ in range(100):\n if (r/sys.argv[2]/'ready').exists(): sys.exit(0)\n \
         time.sleep(0.01)\nsys.exit(9)\n",
    )?;
    let output = cli(temp.path(), &["run", "check", "--concurrency", "2"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    Ok(())
}

#[test]
fn test_should_treat_corrupt_cache_entries_as_misses_without_restoring_escaped_paths() -> TestResult
{
    let temp = fixture()?;
    app(temp.path(), "tasks:\n  build:\n    - workspace: api\n      command: python3 build.py\n      cache:\n        enabled: true\n        inputs: [apps/catalog/build.py]\n        outputs: [apps/catalog/dist]\n")?;
    write(
        temp.path(),
        "apps/catalog/build.py",
        "import pathlib\np=pathlib.Path('dist')\np.mkdir(exist_ok=True)\n(p/'result').write_text('\
         safe')\n",
    )?;
    assert!(cli(temp.path(), &["run", "build"])?.status.success());
    let path = fs::read_dir(temp.path().join(".repoctl/cache/tasks"))?
        .next()
        .ok_or("cache entry missing")??
        .path();
    let mut entry: Value = serde_json::from_slice(&fs::read(&path)?)?;
    entry["artifacts"][0]["path"] = Value::String("apps/catalog/dist/../../../escaped".to_string());
    fs::write(&path, serde_json::to_vec(&entry)?)?;
    fs::remove_dir_all(temp.path().join("apps/catalog/dist"))?;
    let output = cli(temp.path(), &["run", "build"])?;
    assert!(output.status.success());
    assert_eq!(json(&output)?["outputs"][0]["output"]["cached"], false);
    assert!(!temp.path().join("escaped").exists());
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/result"))?,
        "safe"
    );
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Repoctl Test",
            "-c",
            "user.email=repoctl-test@example.invalid",
        ])
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string().into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[test]
fn test_should_select_both_rename_owners_and_local_untracked_files() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "")?;
    write(
        temp.path(),
        "apps/other/project.yaml",
        "schema: company.project/v1\nname: apps.other\nkind: app\npath: apps/other\n",
    )?;
    write(temp.path(), "apps/catalog/old.py", "old source")?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=main"])?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "fixture"])?;
    let base = git(temp.path(), &["rev-parse", "HEAD"])?;
    git(
        temp.path(),
        &["mv", "apps/catalog/old.py", "apps/other/new.py"],
    )?;
    let local = cli(temp.path(), &["affected"])?;
    assert_eq!(
        json(&local)?["directlyAffected"].as_array().map(Vec::len),
        Some(2)
    );
    git(temp.path(), &["commit", "--quiet", "-m", "rename"])?;
    let range = cli(
        temp.path(),
        &["affected", "--base", &base, "--head", "HEAD"],
    )?;
    assert_eq!(
        json(&range)?["directlyAffected"].as_array().map(Vec::len),
        Some(2)
    );
    write(temp.path(), "apps/catalog/untracked.py", "new source")?;
    let untracked = cli(temp.path(), &["affected"])?;
    assert_eq!(json(&untracked)?["directlyAffected"][0], "apps.catalog");
    fs::remove_file(temp.path().join("apps/other/new.py"))?;
    let deleted = cli(temp.path(), &["affected"])?;
    assert_eq!(
        json(&deleted)?["directlyAffected"],
        serde_json::json!(["apps.catalog", "apps.other"])
    );
    Ok(())
}

#[test]
fn test_should_compare_merge_base_instead_of_unrelated_base_branch_changes() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "")?;
    write(
        temp.path(),
        "apps/other/project.yaml",
        "schema: company.project/v1\nname: apps.other\nkind: app\npath: apps/other\n",
    )?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=main"])?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "fixture"])?;
    let common = git(temp.path(), &["rev-parse", "HEAD"])?;
    write(temp.path(), "apps/catalog/main.py", "base branch only")?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "base-only"])?;
    git(
        temp.path(),
        &["checkout", "--quiet", "-b", "feature", &common],
    )?;
    write(temp.path(), "apps/other/feature.py", "feature branch only")?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "feature-only"])?;
    let output = cli(
        temp.path(),
        &["affected", "--base", "main", "--head", "HEAD"],
    )?;
    assert_eq!(
        json(&output)?["directlyAffected"],
        serde_json::json!(["apps.other"])
    );
    Ok(())
}

#[test]
fn test_should_migrate_generic_packages_and_preserve_alias_and_target_options() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "shared/Cargo.toml",
        "[package]\nname='shared'\nversion='0.1.0'\nedition='2024'\n[features]\nserde=[]\n",
    )?;
    write(&source, "shared/src/lib.rs", "pub fn value() {}\n")?;
    write(
        &source,
        "consumer/Cargo.toml",
        r"[package]
name='consumer'
version='0.1.0'
edition='2024'
[dependencies]
renamed={package='shared',version='0.1',features=['serde'],default-features=false,optional=true}
[target.'cfg(unix)'.dependencies]
shared={version='0.1',features=['serde']}
",
    )?;
    write(&source, "consumer/src/lib.rs", "pub fn value() {}\n")?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", "plan", "--source"])
        .arg(&source)
        .arg("--dest")
        .arg(temp.path())
        .args([
            "--ci",
            "off",
            "--verification",
            "structural",
            "--format",
            "json",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan = json(&output)?;
    assert_eq!(plan["dependencyRewrites"].as_array().map(Vec::len), Some(2));
    let operations = plan["operations"].as_array().ok_or("operations missing")?;
    let content = operations
        .iter()
        .find(|op| {
            op["destinationPath"]
                .as_str()
                .is_some_and(|p| p.ends_with("consumer/Cargo.toml"))
        })
        .and_then(|op| op["content"].as_str())
        .ok_or("rewritten Cargo manifest missing")?;
    assert!(content.contains("package='shared'") || content.contains("package = 'shared'"));
    assert!(content.contains("optional=true") || content.contains("optional = true"));
    assert!(
        content.contains("default-features=false") || content.contains("default-features = false")
    );
    assert!(content.contains("features=['serde']") || content.contains("features = ['serde']"));
    assert!(content.contains("path"));
    Ok(())
}

#[test]
fn test_should_plan_buf_breaking_with_comparison_as_one_argument() -> TestResult {
    let temp = fixture()?;
    write(temp.path(), "protos/buf.yaml", "version: v2\n")?;
    let against = ".git#branch=main,subdir=protos";
    let output = cli(temp.path(), &["proto", "check", "--against", against])?;
    assert!(output.status.success());
    let data = json(&output)?;
    assert_eq!(
        data["commands"][1]["args"],
        serde_json::json!(["breaking", "protos", "--against", against])
    );
    assert_eq!(data["outputs"], serde_json::json!([]));
    Ok(())
}

#[test]
fn test_should_export_deterministic_impact_diagrams_with_selected_tasks() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        "tasks:\n  check:\n    - workspace: api\n      command: python3 check.py\n",
    )?;
    let args = [
        "affected",
        "--changed-file",
        "apps/catalog/check.py",
        "--diagram",
        "mermaid",
    ];
    let first = cli(temp.path(), &args)?;
    let second = cli(temp.path(), &args)?;
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    let text = String::from_utf8(first.stdout)?;
    assert!(text.starts_with("flowchart LR"));
    assert!(text.contains("selected task"));
    assert!(text.contains("apps.catalog:api:check"));
    Ok(())
}
