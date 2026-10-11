//! CI tool requirements, Poetry groups and uv lockfile ownership regressions.

use std::{fs, process::Command};

use serde_json::Value;

use super::{
    super::{TestResult, cli, fixture, json, write},
    adoption_review::{adoption_command, plan},
};

#[test]
fn test_should_detect_member_dependencies_and_configured_build_scripts_needing_protoc() -> TestResult
{
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        r"schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
workspaces:
  - name: rust
    language: rust
    root: .
    manifest: Cargo.toml
tasks:
  check:
    - workspace: rust
      command: cargo check
",
    )?;
    write(
        temp.path(),
        "apps/catalog/Cargo.toml",
        "[workspace]\nmembers=['crates/*']\nexclude=['crates/ignored']\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/crates/ignored/Cargo.toml",
        "[package]\nname='ignored'\nversion='0.1.0'\n[build-dependencies]\nprost-build='0.14'\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/crates/member/build.rs",
        "fn main() {}\n",
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
    write(
        temp.path(),
        "frameworks/shared/build.rs",
        "fn main() { std::process::Command::new(\"protoc\"); }\n",
    )?;
    for (extra, script, needs_protoc) in [
        (
            "\n[dependencies]\nshared={path='../../../../frameworks/shared'}",
            "fn main() {}",
            true,
        ),
        ("", "fn main() {}", false),
        (
            "\n[build-dependencies]\nprost-build='0.14'",
            "fn main() {}",
            true,
        ),
        (
            "\n[target.'cfg(unix)'.build-dependencies]\nrenamed={package='tonic-build',version='0.\
             14'}",
            "fn main() {}",
            true,
        ),
        (
            "",
            "fn main() { std::process::Command::new(\"protoc\"); }",
            true,
        ),
        ("build='scripts/generate.rs'", "fn main() {}", true),
        (
            "build=false",
            "fn main() { std::process::Command::new(\"protoc\"); }",
            false,
        ),
    ] {
        write(
            temp.path(),
            "apps/catalog/crates/member/Cargo.toml",
            &format!("[package]\nname='member'\nversion='0.1.0'\n{extra}\n"),
        )?;
        write(temp.path(), "apps/catalog/crates/member/build.rs", script)?;
        write(
            temp.path(),
            "apps/catalog/crates/member/scripts/generate.rs",
            "fn main() { prost_build::compile_protos(&[], &[]); }\n",
        )?;
        let output = cli(
            temp.path(),
            &["ci", "matrix", "--tasks", "check", "--fallback", "all"],
        )?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let data = json(&output)?;
        assert_eq!(
            data["entries"][0]["needsProtoc"], needs_protoc,
            "{extra}/{script}"
        );
    }
    Ok(())
}

#[test]
fn test_should_discover_poetry_dependency_groups_and_propagate_affected_consumers() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        r"schema: company.project/v1
name: apps.catalog
kind: app
path: apps/catalog
workspaces:
  - name: python
    language: python
    root: .
    manifest: pyproject.toml
",
    )?;
    write(
        temp.path(),
        "frameworks/shared/project.yaml",
        r"schema: company.project/v1
name: frameworks.shared
kind: framework
path: frameworks/shared
workspaces:
  - name: python
    language: python
    root: .
    manifest: pyproject.toml
public_facades:
  python: [.]
",
    )?;
    write(
        temp.path(),
        "frameworks/shared/pyproject.toml",
        "[tool.poetry]\nname='Shared_Pkg'\nversion='0.1.0'\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/pyproject.toml",
        r"[tool.poetry]
name='consumer'
version='0.1.0'
[tool.poetry.group.dev.dependencies]
shared-pkg={path='../../frameworks/shared'}
[tool.poetry.group.qa]
optional=true
[tool.poetry.group.qa.dependencies]
'shared.pkg'='^0.1'
",
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
        .ok_or("edges missing")?;
    assert_eq!(
        edges
            .iter()
            .filter(|edge| edge["to"] == "project:frameworks.shared")
            .count(),
        2
    );
    let affected = json(&cli(
        temp.path(),
        &[
            "affected",
            "--changed-file",
            "frameworks/shared/src/main.py",
        ],
    )?)?;
    assert!(
        affected["transitivelyAffected"]
            .as_array()
            .is_some_and(|values| values.contains(&Value::from("apps.catalog")))
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_should_inherit_nearest_uv_lock_and_install_from_its_owner() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "pyproject.toml",
        "[project]\nname='root'\nversion='0.1.0'\n[tool.uv.workspace]\nmembers=['packages/*']\n",
    )?;
    write(&source, "uv.lock", "version=1\n")?;
    write(
        &source,
        "packages/child/pyproject.toml",
        "[project]\nname='child'\nversion='0.1.0'\n",
    )?;
    write(
        &source,
        "packages/standalone/pyproject.toml",
        "[project]\nname='standalone'\nversion='0.1.0'\n",
    )?;
    write(&source, "packages/standalone/uv.lock", "version=1\n")?;
    let output = plan(&source, temp.path(), &["--map", "source=apps/source"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    let manifest: Value = serde_norway::from_str(
        data["manifestSyntheses"][0]["content"]
            .as_str()
            .ok_or("manifest missing")?,
    )?;
    let workspaces = manifest["workspaces"]
        .as_array()
        .ok_or("workspaces missing")?;
    for (root, expected) in [
        (".", "uv.lock"),
        ("packages/child", "uv.lock"),
        ("packages/standalone", "packages/standalone/uv.lock"),
    ] {
        let workspace = workspaces
            .iter()
            .find(|workspace| workspace["root"] == root)
            .ok_or("workspace missing")?;
        assert_eq!(workspace["lockfile"], expected);
    }
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    write(
        temp.path(),
        "bin/uv",
        r"#!/usr/bin/env python3
import pathlib,sys
with pathlib.Path('calls').open('a') as f:
 f.write(' '.join(sys.argv[1:])+'\n')
",
    )?;
    fs::set_permissions(
        temp.path().join("bin/uv"),
        fs::Permissions::from_mode(0o755),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "ci",
            "run-step",
            "--project",
            "apps.source",
            "--workspace",
            "python-packages-child",
            "--task",
            "check",
            "--install",
            "--format",
            "json",
            "--repo",
        ])
        .arg(temp.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                temp.path().join("bin").display(),
                std::env::var("PATH")?
            ),
        )
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/source/calls"))?,
        "sync --frozen\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/source/packages/child/calls"))?,
        "run python -m compileall -q .\n"
    );
    Ok(())
}
