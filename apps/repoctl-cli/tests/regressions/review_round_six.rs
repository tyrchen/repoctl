//! Repository task routers and Cargo ownership-aware migration regressions.

use std::{collections::BTreeSet, error::Error, fs, path::Path, process::Command};

use serde_json::Value;

use super::{
    super::{TestResult, app, cli, fixture, json, write},
    adoption_review::adoption_command,
};

#[test]
fn test_should_select_all_projects_for_root_task_routing_changes() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        "tasks:\n  check:\n    - workspace: api\n      command: python3 check.py\n",
    )?;
    write(temp.path(), "apps/other/project.yaml", "schema: company.project/v1\nname: apps.other\nkind: app\npath: apps/other\nworkspaces:\n  - name: api\n    language: python\n    root: .\n    manifest: pyproject.toml\ntasks:\n  check:\n    - workspace: api\n      command: python3 check.py\n")?;
    for router in [
        "Makefile",
        "makefile",
        "GNUmakefile",
        "Makefile.toml",
        "Justfile",
        "justfile",
        ".justfile",
        "Taskfile.yaml",
        "Taskfile.yml",
        ".cargo/config.toml",
        ".cargo/config",
        "pnpm-workspace.yaml",
    ] {
        let data = json(&cli(temp.path(), &["affected", "--changed-file", router])?)?;
        assert_eq!(data["directlyAffected"].as_array().map(Vec::len), Some(2));
        assert_eq!(data["tasks"].as_array().map(Vec::len), Some(2));
        let matrix = json(&cli(
            temp.path(),
            &["ci", "matrix", "--tasks", "check", "--changed-file", router],
        )?)?;
        assert_eq!(matrix["entries"].as_array().map(Vec::len), Some(2));
    }
    let scoped = json(&cli(
        temp.path(),
        &["affected", "--changed-file", "apps/catalog/Makefile"],
    )?)?;
    assert_eq!(
        scoped["directlyAffected"],
        serde_json::json!(["apps.catalog"])
    );
    Ok(())
}

#[test]
fn test_should_group_cargo_members_under_their_real_lock_and_verification_owner() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    cargo_sources(&source)?;
    let lock = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(source.join("rust"))
        .output()?;
    assert!(
        lock.status.success(),
        "{}",
        String::from_utf8_lossy(&lock.stderr)
    );
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
    let root_task = assert_cargo_owners(&data)?;
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
    for command in data["verification"]["commands"]
        .as_array()
        .ok_or("commands missing")?
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
    let matrix = json(&cli(
        temp.path(),
        &["ci", "matrix", "--tasks", "check", "--fallback", "all"],
    )?)?;
    assert_eq!(matrix["entries"].as_array().map(Vec::len), Some(3));
    assert_frozen_install(temp.path(), &root_task)?;
    Ok(())
}

fn cargo_sources(source: &Path) -> TestResult {
    write(source, "package.json", r#"{"name":"node-root"}"#)?;
    write(
        source,
        "rust/Cargo.toml",
        r"[workspace]
resolver='3'
members=['crates/*','../sibling']
exclude=['standalone','separate']
default-members=['crates/api']
",
    )?;
    for (path, name) in [
        ("rust/crates/api", "api"),
        ("rust/crates/web", "web"),
        ("rust/helper", "helper"),
        ("sibling", "sibling"),
        ("rust/standalone", "standalone"),
        ("rust/separate", "separate"),
    ] {
        let extra = match name {
            "api" => "[dependencies]\nhelper={path='../../helper'}\n",
            "separate" => "[workspace]\n",
            _ => "",
        };
        let workspace = if name == "sibling" {
            "workspace='../rust'\n"
        } else {
            ""
        };
        write(
            source,
            &format!("{path}/Cargo.toml"),
            &format!(
                "[package]\nname='{name}'\nversion='0.1.0'\nedition='2024'\n{workspace}{extra}"
            ),
        )?;
        write(
            source,
            &format!("{path}/src/lib.rs"),
            "pub fn value() -> u8 { 1 }\n",
        )?;
    }
    Ok(())
}

fn assert_cargo_owners(data: &Value) -> Result<String, Box<dyn Error>> {
    let content = data["manifestSyntheses"][0]["content"]
        .as_str()
        .ok_or("manifest missing")?;
    let manifest: Value = serde_norway::from_str(content)?;
    let workspaces = manifest["workspaces"]
        .as_array()
        .ok_or("workspaces missing")?;
    let roots = workspaces
        .iter()
        .filter(|workspace| workspace["language"] == "rust")
        .filter_map(|workspace| workspace["root"].as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        roots,
        BTreeSet::from(["rust", "rust/separate", "rust/standalone"])
    );
    let owner = workspaces
        .iter()
        .find(|workspace| workspace["root"] == "rust")
        .ok_or("Cargo owner missing")?;
    assert_eq!(owner["lockfile"], "rust/Cargo.lock");
    for workspace in workspaces.iter().filter(|workspace| {
        workspace["root"] == "rust/standalone" || workspace["root"] == "rust/separate"
    }) {
        assert!(workspace["lockfile"].is_null());
    }
    let commands = data["verification"]["commands"]
        .as_array()
        .ok_or("commands missing")?;
    assert_eq!(
        commands
            .iter()
            .filter(|command| command["program"] == "cargo")
            .count(),
        3
    );
    let check = commands
        .iter()
        .find(|command| command["cwd"] == "apps/source/rust")
        .ok_or("root check missing")?;
    assert!(
        check["args"]
            .as_array()
            .is_some_and(|args| args.contains(&Value::from("--locked")))
    );
    assert!(content.contains("cargo check --workspace"));
    Ok(owner["name"]
        .as_str()
        .ok_or("owner name missing")?
        .to_string())
}

fn assert_frozen_install(root: &Path, task: &str) -> TestResult {
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "ci",
            "run-step",
            "--project",
            "apps.source",
            "--workspace",
            task,
            "--task",
            "check",
            "--install",
            "--repo",
        ])
        .arg(root)
        .args(["--format", "json"])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Frozen installation must reject stale workspace locks rather than repair them.
    write(root, "apps/source/rust/Cargo.lock", "version = 4\n")?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "ci",
            "run-step",
            "--project",
            "apps.source",
            "--workspace",
            task,
            "--task",
            "check",
            "--install",
            "--repo",
        ])
        .arg(root)
        .args(["--format", "json"])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(root.join("apps/source/rust/Cargo.lock"))?,
        "version = 4\n"
    );
    Ok(())
}
