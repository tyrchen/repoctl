//! Failure, cache-contract and offline-discovery regressions.

mod adoption_execution;
mod adoption_review;
mod buf_contract;
mod cache_dependencies;
mod contracts;
mod review;
mod review_round_two;

use std::{fs, process::Command};

use super::{TestResult, app, cli, fixture, json, write};

#[test]
fn test_should_invalidate_cache_on_selected_environment_and_runtime_changes() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        r"tasks:
  build:
    - workspace: api
      command: python3 build.py
      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/dist]
        env: [REPOCTL_TEST_SETTING]
        runtime: [python3 version.py]
",
    )?;
    write(temp.path(), "apps/catalog/src/input", "input")?;
    write(
        temp.path(),
        "apps/catalog/version.py",
        "print('runtime-v1')\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/build.py",
        "import pathlib,os\np=pathlib.Path('dist')\np.mkdir(exist_ok=True)\n(p/'result').\
         write_text(os.environ['REPOCTL_TEST_SETTING'])\n",
    )?;
    let run = |value: &str| {
        Command::new(env!("CARGO_BIN_EXE_repoctl"))
            .args(["run", "build", "--format", "json", "--repo"])
            .arg(temp.path())
            .env("REPOCTL_TEST_SETTING", value)
            .output()
    };
    assert!(run("one")?.status.success());
    assert_eq!(json(&run("one")?)?["outputs"][0]["output"]["cached"], true);
    assert_eq!(json(&run("two")?)?["outputs"][0]["output"]["cached"], false);
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/result"))?,
        "two"
    );
    write(
        temp.path(),
        "apps/catalog/version.py",
        "print('runtime-v2')\n",
    )?;
    assert_eq!(json(&run("two")?)?["outputs"][0]["output"]["cached"], false);
    Ok(())
}

#[test]
fn test_should_never_cache_failed_commands() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        r"tasks:
  build:
    - workspace: api
      command: python3 fail.py
      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/dist]
",
    )?;
    write(temp.path(), "apps/catalog/src/input", "input")?;
    write(
        temp.path(),
        "apps/catalog/fail.py",
        "import pathlib,sys\np=pathlib.Path('dist')\np.mkdir(exist_ok=True)\nc=p/'count'\nc.\
         write_text(str(int(c.read_text())+1) if c.exists() else '1')\nsys.exit(7)\n",
    )?;
    for _ in 0..2 {
        assert!(!cli(temp.path(), &["run", "build"])?.status.success());
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/count"))?,
        "2"
    );
    assert!(!temp.path().join(".repoctl/cache/tasks").exists());
    Ok(())
}

#[test]
fn test_should_reject_unsafe_cache_contracts_before_dry_run() -> TestResult {
    for (inputs, outputs) in [
        ("apps/catalog/**", "apps/catalog/dist"),
        ("apps/catalog/src/**", "apps/catalog/project.yaml"),
        ("apps/catalog/src/**", "apps/catalog/.git"),
        ("apps/catalog/src/**", "apps/other/dist"),
    ] {
        let temp = fixture()?;
        app(
            temp.path(),
            &format!(
                "tasks:\n  build:\n    - workspace: api\n      command: python3 build.py\n      \
                 cache:\n        enabled: true\n        inputs: [{inputs}]\n        outputs: \
                 [{outputs}]\n"
            ),
        )?;
        assert!(
            !cli(temp.path(), &["run", "build", "--dry-run"])?
                .status
                .success()
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_should_restore_empty_directories_and_executable_artifacts_and_reject_symlinks() -> TestResult
{
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = fixture()?;
    app(
        temp.path(),
        r"tasks:
  build:
    - workspace: api
      command: python3 build.py
      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/dist]
",
    )?;
    write(temp.path(), "apps/catalog/src/input", "input")?;
    write(
        temp.path(),
        "apps/catalog/build.py",
        "import pathlib\np=pathlib.Path('dist')\n(p/'empty'/'nested').mkdir(parents=True,\
         exist_ok=True)\nf=p/'tool'\nf.write_text('run')\nf.chmod(0o755)\n",
    )?;
    assert!(cli(temp.path(), &["run", "build"])?.status.success());
    fs::remove_dir_all(temp.path().join("apps/catalog/dist"))?;
    let hit = cli(temp.path(), &["run", "build"])?;
    assert_eq!(json(&hit)?["outputs"][0]["output"]["cached"], true);
    assert!(temp.path().join("apps/catalog/dist/empty/nested").is_dir());
    assert_ne!(
        fs::metadata(temp.path().join("apps/catalog/dist/tool"))?
            .permissions()
            .mode()
            & 0o111,
        0
    );
    symlink(
        temp.path().join("repo.yaml"),
        temp.path().join("apps/catalog/src/link"),
    )?;
    assert!(!cli(temp.path(), &["run", "build"])?.status.success());
    Ok(())
}

#[test]
fn test_should_discover_cargo_and_python_path_dependencies_without_package_managers() -> TestResult
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
  - name: python
    language: python
    root: jobs
    manifest: jobs/pyproject.toml
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
  - name: rust
    language: rust
    root: .
    manifest: Cargo.toml
  - name: python
    language: python
    root: jobs
    manifest: jobs/pyproject.toml
public_facades:
  rust: [.]
  python: [jobs]
",
    )?;
    write(
        temp.path(),
        "apps/catalog/Cargo.toml",
        r"[package]
name='catalog'
version='0.1.0'
[dependencies]
renamed={package='shared',path='../../frameworks/shared'}
[target.'cfg(unix)'.build-dependencies]
shared={path='../../frameworks/shared'}
",
    )?;
    write(
        temp.path(),
        "frameworks/shared/Cargo.toml",
        "[package]\nname='shared'\nversion='0.1.0'\n",
    )?;
    write(
        temp.path(),
        "frameworks/shared/jobs/pyproject.toml",
        "[project]\nname='shared-jobs'\nversion='0.1.0'\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/jobs/pyproject.toml",
        r"[project]
name='catalog-jobs'
version='0.1.0'
[tool.uv.sources]
shared-jobs={path='../../../frameworks/shared/jobs'}
[tool.poetry.dependencies]
shared-jobs={path='../../../frameworks/shared/jobs'}
",
    )?;
    let output = cli(temp.path(), &["graph", "print"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let edges = json(&output)?["snapshot"]["graph"]["edges"]
        .as_array()
        .cloned()
        .ok_or("missing edges")?;
    assert!(
        edges
            .iter()
            .filter(|edge| edge["to"] == "project:frameworks.shared")
            .count()
            >= 3
    );
    Ok(())
}

#[test]
fn test_should_fail_malformed_and_ambiguous_local_manifests() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        "schema: company.project/v1\nname: apps.catalog\nkind: app\npath: \
         apps/catalog\nworkspaces:\n  - name: web\n    language: typescript\n    root: .\n    \
         manifest: package.json\n",
    )?;
    write(temp.path(), "apps/catalog/package.json", "{bad json}")?;
    assert!(!cli(temp.path(), &["graph", "print"])?.status.success());
    write(
        temp.path(),
        "apps/catalog/package.json",
        r#"{"name":"same"}"#,
    )?;
    write(temp.path(), "apps/other/project.yaml", "schema: company.project/v1\nname: apps.other\nkind: app\npath: apps/other\nworkspaces:\n  - name: web\n    language: typescript\n    root: .\n    manifest: package.json\n")?;
    write(temp.path(), "apps/other/package.json", r#"{"name":"same"}"#)?;
    assert!(!cli(temp.path(), &["graph", "print"])?.status.success());
    Ok(())
}

#[cfg(unix)]
#[test]
fn test_should_execute_buf_argv_and_report_nonzero_checks() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture()?;
    write(temp.path(), "protos/buf.yaml", "version: v2\n")?;
    write(
        temp.path(),
        "bin/buf",
        "#!/usr/bin/env python3\nimport json,pathlib,sys\nwith \
         pathlib.Path('argv.jsonl').open('a') as f: \
         f.write(json.dumps(sys.argv[1:])+'\\n')\nsys.exit(7 if sys.argv[1]=='breaking' else 0)\n",
    )?;
    fs::set_permissions(
        temp.path().join("bin/buf"),
        fs::Permissions::from_mode(0o755),
    )?;
    let path = format!(
        "{}:{}",
        temp.path().join("bin").display(),
        std::env::var("PATH")?
    );
    let comparison = ".git#branch=main,subdir=protos";
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "proto",
            "check",
            "--execute",
            "--against",
            comparison,
            "--format",
            "json",
            "--repo",
        ])
        .arg(temp.path())
        .env("PATH", path)
        .output()?;
    assert!(!output.status.success());
    let data = json(&output)?;
    assert_eq!(data["outputs"][1]["status"], 7);
    assert!(data["diagnostics"].as_array().is_some_and(|items| {
        items
            .iter()
            .any(|item| item["code"] == "proto.check.failed")
    }));
    let argv = fs::read_to_string(temp.path().join("argv.jsonl"))?;
    assert!(argv.contains(comparison));
    Ok(())
}

#[test]
fn test_should_reject_cancelled_ci_and_accept_only_intentional_empty_selection() -> TestResult {
    let temp = fixture()?;
    let report = cli(temp.path(), &["ci", "workflow"])?;
    let data = json(&report)?;
    let content = data["content"].as_str().ok_or("workflow missing")?;
    let workflow: serde_json::Value = serde_norway::from_str(content)?;
    let script = workflow["jobs"]["required"]["steps"][0]["run"]
        .as_str()
        .ok_or("required script missing")?;
    for (graph, tasks, has_tasks, success) in [
        ("success", "success", "true", true),
        ("success", "skipped", "false", true),
        ("success", "cancelled", "true", false),
        ("cancelled", "skipped", "false", false),
        ("failure", "success", "true", false),
        ("success", "skipped", "true", false),
    ] {
        let status = Command::new("sh")
            .args(["-e", "-c", script])
            .env("GRAPH", graph)
            .env("TASKS", tasks)
            .env("HAS_TASKS", has_tasks)
            .status()?;
        assert_eq!(status.success(), success, "{graph}/{tasks}/{has_tasks}");
    }
    Ok(())
}
