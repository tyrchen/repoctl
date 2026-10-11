//! Pinned toolchains, local Cargo closures and polyglot adoption regressions.

use std::{collections::BTreeSet, error::Error, fs, process::Command};

use serde_json::Value;

use super::{
    super::{TestResult, cli, fixture, json, write},
    adoption_review::adoption_command,
};

#[test]
fn test_should_select_every_workspace_for_root_toolchain_changes() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        r"schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
workspaces:
  - name: api
    language: rust
    root: api
    manifest: api/Cargo.toml
  - name: web
    language: typescript
    root: web
    manifest: web/package.json
tasks:
  check:
    - workspace: api
      command: cargo check
    - workspace: web
      command: npm run check
",
    )?;
    write(
        temp.path(),
        "apps/catalog/api/Cargo.toml",
        "[package]\nname='api'\nversion='0.1.0'\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/web/package.json",
        r#"{"name":"web"}"#,
    )?;
    for toolchain in ["rust-toolchain.toml", "rust-toolchain"] {
        let data = json(&cli(
            temp.path(),
            &["affected", "--changed-file", toolchain],
        )?)?;
        assert_eq!(data["tasks"].as_array().map(Vec::len), Some(2));
        assert!(data["diagnostics"].as_array().is_some_and(Vec::is_empty));
        let data = json(&cli(
            temp.path(),
            &[
                "ci",
                "matrix",
                "--tasks",
                "check",
                "--changed-file",
                toolchain,
                "--changed-file",
                "apps/catalog/api/lib.rs",
            ],
        )?)?;
        assert_eq!(data["entries"].as_array().map(Vec::len), Some(2));
    }
    Ok(())
}

#[test]
fn test_should_follow_local_path_packages_without_a_cargo_workspace_table() -> TestResult {
    check_local_cargo_closure(".", "helper")?;
    check_local_cargo_closure("backend", "../helper")?;
    Ok(())
}

fn check_local_cargo_closure(root: &str, helper_path: &str) -> TestResult {
    let temp = fixture()?;
    let manifest = if root == "." {
        "Cargo.toml".to_string()
    } else {
        format!("{root}/Cargo.toml")
    };
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        &format!(
            "schema: company.project/v1\nname: apps.catalog\nkind: app\npath: \
             apps/catalog\nworkspaces:\n  - name: rust\n    language: rust\n    root: {root}\n    \
             manifest: {manifest}\ntasks:\n  check:\n    - workspace: rust\n      command: cargo \
             check\n"
        ),
    )?;
    write(
        temp.path(),
        &format!("apps/catalog/{manifest}"),
        &format!(
            "[package]\nname='catalog'\nversion='0.1.0'\n[dependencies]\nhelper={{path='\
             {helper_path}'}}\n"
        ),
    )?;
    write(
        temp.path(),
        "apps/catalog/helper/Cargo.toml",
        r"[package]
name='helper'
version='0.1.0'
[target.'cfg(unix)'.dev-dependencies]
shared={path='../../../frameworks/shared'}
",
    )?;
    write(
        temp.path(),
        "frameworks/shared/project.yaml",
        "schema: company.project/v1\nname: frameworks.shared\nkind: framework\npath: \
         frameworks/shared\npublic_facades:\n  rust: [.]\n",
    )?;
    write(
        temp.path(),
        "frameworks/shared/Cargo.toml",
        "[package]\nname='shared'\nversion='0.1.0'\n",
    )?;
    let output = cli(temp.path(), &["graph", "print"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    assert!(
        data["snapshot"]["graph"]["edges"]
            .as_array()
            .is_some_and(|edges| edges
                .iter()
                .any(|edge| edge["from"] == "project:apps.catalog"
                    && edge["to"] == "project:frameworks.shared"))
    );
    let data = json(&cli(
        temp.path(),
        &["affected", "--changed-file", "frameworks/shared/src/lib.rs"],
    )?)?;
    assert!(
        data["transitivelyAffected"]
            .as_array()
            .is_some_and(|projects| projects.contains(&Value::from("apps.catalog")))
    );
    // The same hidden dependency must also participate in boundary validation.
    write(
        temp.path(),
        "frameworks/shared/project.yaml",
        "schema: company.project/v1\nname: frameworks.shared\nkind: framework\npath: \
         frameworks/shared\npublic_facades:\n  rust: [public]\n",
    )?;
    assert!(
        !cli(temp.path(), &["graph", "validate", "--mode", "metadata"])?
            .status
            .success()
    );
    Ok(())
}

#[test]
fn test_should_adopt_nested_rust_and_assign_unique_polyglot_workspace_names() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    for package in [
        "package.json",
        "frontend/package.json",
        "client/one/package.json",
        "client/two/package.json",
        "python/package.json",
        "rust-backend/package.json",
    ] {
        write(
            &source,
            package,
            &format!(
                r#"{{"name":"{}","scripts":{{"check":"echo ok"}}}}"#,
                package.replace('/', "-")
            ),
        )?;
    }
    for package in ["backend", "other"] {
        write(
            &source,
            &format!("{package}/Cargo.toml"),
            &format!("[package]\nname='{package}'\nversion='0.1.0'\nedition='2024'\n"),
        )?;
        write(
            &source,
            &format!("{package}/src/lib.rs"),
            "pub fn value() -> u8 { 1 }\n",
        )?;
    }
    write(
        &source,
        "pyproject.toml",
        "[project]\nname='python-root'\nversion='0.1.0'\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", "plan", "--source"])
        .arg(&source)
        .arg("--dest")
        .arg(temp.path())
        .args([
            "--ci",
            "update",
            "--verification",
            "full",
            "--map",
            "source=apps/source",
            "--format",
            "json",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    assert_polyglot_plan(&data)?;
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    assert!(
        adoption_command(temp.path(), "verify", &plan_path)?
            .status
            .success()
    );
    let matrix = json(&cli(
        temp.path(),
        &["ci", "matrix", "--tasks", "check", "--fallback", "all"],
    )?)?;
    assert_eq!(matrix["entries"].as_array().map(Vec::len), Some(9));
    let cargo_commands = data["verification"]["commands"]
        .as_array()
        .ok_or("verification missing")?;
    for command in cargo_commands
        .iter()
        .filter(|command| command["program"] == "cargo")
    {
        let cwd = command["cwd"].as_str().ok_or("cwd missing")?;
        let args = command["args"].as_array().ok_or("args missing")?;
        let output = Command::new("cargo")
            .args(args.iter().filter_map(Value::as_str))
            .arg("--offline")
            .current_dir(temp.path().join(cwd))
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn assert_polyglot_plan(data: &Value) -> Result<(), Box<dyn Error>> {
    let content = data["manifestSyntheses"][0]["content"]
        .as_str()
        .ok_or("synthesis missing")?;
    let manifest: Value = serde_norway::from_str(content)?;
    let workspaces = manifest["workspaces"]
        .as_array()
        .ok_or("workspaces missing")?;
    assert_eq!(workspaces.len(), 9);
    let names = workspaces
        .iter()
        .filter_map(|workspace| workspace["name"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names.len(), workspaces.len());
    for root in ["backend", "other"] {
        assert!(
            workspaces
                .iter()
                .any(|workspace| workspace["root"] == root && workspace["language"] == "rust")
        );
    }
    let commands = data["verification"]["commands"]
        .as_array()
        .ok_or("verification missing")?;
    let roots = commands
        .iter()
        .filter(|command| command["program"] == "cargo")
        .filter_map(|command| command["cwd"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        roots,
        BTreeSet::from(["apps/source/backend", "apps/source/other"])
    );
    Ok(())
}
