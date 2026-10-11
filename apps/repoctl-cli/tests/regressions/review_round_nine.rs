//! Project-scoped impact, disabled cache contracts and saved-plan boundary tests.

use std::{
    fs::{self, File},
    path::Path,
};

use serde_json::{Value, json as value};

use super::{
    super::{TestResult, app, cli, fixture, json, write},
    adoption_review::{adoption_command, plan},
};

#[test]
fn test_should_include_outputs_of_disabled_caches_in_fingerprints() -> TestResult {
    check_declared_output_inputs(false)?;
    check_declared_output_inputs(true)?;
    Ok(())
}

fn check_declared_output_inputs(producer_cache: bool) -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        &format!(
            r"tasks:
  producer:
    - workspace: api
      command: python3 unused.py
      cache:
        enabled: {producer_cache}
        inputs: [apps/catalog/producer/**]
        outputs: [apps/catalog/source]
  build:
    - workspace: api
      command: python3 build.py
      cache:
        enabled: true
        inputs: [apps/catalog/source/**]
        outputs: [apps/catalog/out]
"
        ),
    )?;
    write(
        temp.path(),
        "apps/catalog/build.py",
        "import pathlib\np=pathlib.Path('out')\np.mkdir(exist_ok=True)\n(p/'value').\
         write_text(pathlib.Path('source/value').read_text())\n",
    )?;
    for (text, cached) in [("one", false), ("one", true), ("two", false), ("two", true)] {
        write(temp.path(), "apps/catalog/source/value", text)?;
        let output = cli(temp.path(), &["run", "build"])?;
        assert!(output.status.success());
        assert_eq!(json(&output)?["outputs"][0]["output"]["cached"], cached);
        assert_eq!(
            fs::read_to_string(temp.path().join("apps/catalog/out/value"))?,
            text
        );
    }
    Ok(())
}

#[test]
fn test_should_check_sibling_workspace_consumers_of_same_project_dependencies() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        r"schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
workspaces:
  - name: consumer
    language: typescript
    root: consumer
    manifest: consumer/package.json
  - name: shared
    language: typescript
    root: shared
    manifest: shared/package.json
tasks:
  check:
    - workspace: consumer
      command: node check.js
    - workspace: shared
      command: node check.js
",
    )?;
    write(
        temp.path(),
        "apps/catalog/consumer/package.json",
        r#"{"name":"consumer","dependencies":{"shared":"file:../shared"}}"#,
    )?;
    write(
        temp.path(),
        "apps/catalog/shared/package.json",
        r#"{"name":"shared"}"#,
    )?;
    write(
        temp.path(),
        "apps/other/project.yaml",
        "schema: company.project/v1\nname: apps.other\nkind: app\npath: apps/other\n",
    )?;
    let data = json(&cli(
        temp.path(),
        &["affected", "--changed-file", "apps/catalog/shared/index.js"],
    )?)?;
    assert_eq!(data["directlyAffected"], value!(["apps.catalog"]));
    assert_eq!(
        data["workspaces"],
        value!(["apps.catalog:consumer", "apps.catalog:shared"])
    );
    assert_eq!(data["tasks"].as_array().map(Vec::len), Some(2));
    let matrix = json(&cli(
        temp.path(),
        &[
            "ci",
            "matrix",
            "--tasks",
            "check",
            "--changed-file",
            "apps/catalog/shared/index.js",
        ],
    )?)?;
    assert_eq!(matrix["entries"].as_array().map(Vec::len), Some(2));
    Ok(())
}

#[test]
fn test_should_adopt_a_readme_only_root_as_a_documentation_tool() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "README.md", "# Tool specifications\n")?;
    write(&source, "specs/design.md", "documented behavior")?;
    let output = plan(&source, temp.path(), &["--map", "source=tools/source"])?;
    assert!(output.status.success());
    let data = json(&output)?;
    assert_eq!(data["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(data["sources"][0]["name"], "source");
    assert_eq!(data["sources"][0]["inferredKind"], "tool");
    let content = data["manifestSyntheses"][0]["content"]
        .as_str()
        .ok_or("manifest missing")?;
    assert!(!content.contains("\ntasks:\n"));
    let path = temp.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &path)?
            .status
            .success()
    );
    assert!(
        adoption_command(temp.path(), "verify", &path)?
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("tools/source/specs/design.md"))?,
        "documented behavior"
    );
    Ok(())
}

#[test]
fn test_should_include_documents_when_checkout_ancestors_have_excluded_names() -> TestResult {
    for name in ["target", ".git", "node_modules", ".repoctl"] {
        let temp = fixture()?;
        let root = temp.path().join(name).join("checkout");
        write(
            &root,
            "repo.yaml",
            &fs::read_to_string(temp.path().join("repo.yaml"))?,
        )?;
        app(&root, "ai:\n  docs: [docs]\n")?;
        write(&root, "apps/catalog/docs/guide.md", "visible documentation")?;
        write(
            &root,
            "apps/catalog/docs/secrets/hidden.md",
            "NEVER_INCLUDE_SECRET",
        )?;
        let output = cli(
            &root,
            &[
                "context",
                "apps.catalog",
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
        let text = String::from_utf8(output.stdout)?;
        assert!(text.contains("visible documentation"));
        assert!(!text.contains("NEVER_INCLUDE_SECRET"));
    }
    Ok(())
}

#[test]
fn test_should_bound_saved_plan_bytes_and_every_top_level_collection() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "README.md", "# Tool\n")?;
    let original = json(&plan(
        &source,
        temp.path(),
        &["--map", "source=tools/source"],
    )?)?;
    let path = temp.path().join("plan.json");
    File::create(&path)?.set_len(67_108_865)?;
    assert_plan_rejected(temp.path(), &path, "adoption.plan.limit.bytes")?;
    for (pointer, count, template, code) in oversized_collections(&original) {
        let mut data = original.clone();
        *data.pointer_mut(pointer).ok_or("missing plan collection")? =
            Value::Array(vec![template; count]);
        fs::write(&path, serde_json::to_vec(&data)?)?;
        assert_plan_rejected(temp.path(), &path, code)?;
    }
    assert!(!temp.path().join("tools").exists());
    Ok(())
}

fn assert_plan_rejected(root: &Path, path: &Path, code: &str) -> TestResult {
    for command in ["apply", "verify"] {
        let output = adoption_command(root, command, path)?;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn oversized_collections(original: &Value) -> Vec<(&'static str, usize, Value, &'static str)> {
    let command = original["verification"]["commands"][0].clone();
    vec![
        (
            "/sources",
            257,
            original["sources"][0].clone(),
            "adoption.plan.limit.sources",
        ),
        (
            "/operations",
            20_001,
            original["operations"][0].clone(),
            "adoption.plan.limit.operations",
        ),
        (
            "/dependencyRewrites",
            20_001,
            value!({"file":"README.md","package":"p","from":"1","to":"2","surface":"unspecified","ownerProject":"tools.source"}),
            "adoption.plan.limit.dependency_rewrites",
        ),
        (
            "/lockfileRefreshes",
            20_001,
            value!({"lockfile":"package-lock.json","command":command}),
            "adoption.plan.limit.lockfile_refreshes",
        ),
        (
            "/manifestSyntheses",
            257,
            original["manifestSyntheses"][0].clone(),
            "adoption.plan.limit.manifest_syntheses",
        ),
        (
            "/ciOperations",
            2,
            value!({"path":".github/workflows/repoctl.yml","operation":"report-file","content":"preview"}),
            "adoption.plan.limit.ci_operations",
        ),
        (
            "/verification/prerequisites",
            257,
            value!({"tool":"cargo","reason":"required"}),
            "adoption.plan.limit.prerequisites",
        ),
        (
            "/verification/commands",
            20_001,
            command,
            "adoption.plan.limit.commands",
        ),
        (
            "/diagnostics",
            20_001,
            value!({"code":"test","message":"test","severity":"warning"}),
            "adoption.plan.limit.diagnostics",
        ),
        (
            "/sources/0/inventory/manifests",
            20_001,
            Value::from("file"),
            "adoption.plan.limit.inventory.manifests",
        ),
        (
            "/sources/0/reasons",
            257,
            Value::from("reason"),
            "adoption.plan.limit.source.reasons",
        ),
        (
            "/verification/commands/0/args",
            257,
            Value::from("arg"),
            "adoption.plan.limit.command.args",
        ),
    ]
}
