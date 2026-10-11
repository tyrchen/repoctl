//! Cross-project and contract-boundary regression fixtures.

use std::{fs, process::Command};

use crate::{TestResult, app, cli, fixture, json, write};

#[cfg(unix)]
#[test]
fn test_should_install_both_toolchains_sharing_a_cwd_and_stop_on_install_failure() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
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
  - name: web
    language: typescript
    root: .
    manifest: package.json
tasks:
  build:
    - workspace: rust
      command: python3 check.py
  check:
    - workspace: web
      command: python3 check.py
      depends_on: [{project: apps.catalog, workspace: rust, task: build}]
",
    )?;
    write(
        temp.path(),
        "apps/catalog/Cargo.toml",
        "[package]\nname='catalog'\nversion='0.1.0'\n",
    )?;
    write(temp.path(), "apps/catalog/Cargo.lock", "version = 4\n")?;
    write(
        temp.path(),
        "apps/catalog/package.json",
        r#"{"name":"catalog"}"#,
    )?;
    write(temp.path(), "apps/catalog/package-lock.json", "{}")?;
    write(
        temp.path(),
        "apps/catalog/check.py",
        "import pathlib\npathlib.Path('should-not-run').touch()\n",
    )?;
    for (program, status) in [("cargo", 0), ("npm", 7)] {
        write(
            temp.path(),
            &format!("bin/{program}"),
            &format!(
                "#!/usr/bin/env python3\nimport pathlib,sys\nwith \
                 pathlib.Path('installs').open('a') as f: f.write('{program} '+' \
                 '.join(sys.argv[1:])+'\\n')\nsys.exit({status})\n"
            ),
        )?;
        fs::set_permissions(
            temp.path().join("bin").join(program),
            fs::Permissions::from_mode(0o755),
        )?;
    }
    let path = format!(
        "{}:{}",
        temp.path().join("bin").display(),
        std::env::var("PATH")?
    );
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "ci",
            "run-step",
            "--project",
            "apps.catalog",
            "--workspace",
            "web",
            "--task",
            "check",
            "--install",
            "--format",
            "json",
            "--repo",
        ])
        .arg(temp.path())
        .env("PATH", path)
        .output()?;
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/installs"))?,
        "cargo fetch --locked\nnpm ci\n"
    );
    assert!(!temp.path().join("apps/catalog/should-not-run").exists());
    assert!(
        json(&output)?["diagnostics"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["code"] == "ci.install.failed"))
    );
    Ok(())
}

#[test]
fn test_should_invalidate_cached_consumers_when_dependency_sources_change() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        r"depends_on: [frameworks.shared]
tasks:
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
        "import pathlib\np=pathlib.Path('dist')\np.mkdir(exist_ok=True)\n(p/'artifact').\
         write_text(pathlib.Path('../../frameworks/shared/source').read_text())\n",
    )?;
    write(
        temp.path(),
        "frameworks/shared/project.yaml",
        "schema: company.project/v1\nname: frameworks.shared\nkind: framework\npath: \
         frameworks/shared\n",
    )?;
    write(temp.path(), "frameworks/shared/source", "one")?;
    assert!(cli(temp.path(), &["run", "build"])?.status.success());
    assert_eq!(
        json(&cli(temp.path(), &["run", "build"])?)?["outputs"][0]["output"]["cached"],
        true
    );
    write(temp.path(), "frameworks/shared/source", "two")?;
    assert_eq!(
        json(&cli(temp.path(), &["run", "build"])?)?["outputs"][0]["output"]["cached"],
        false
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/dist/artifact"))?,
        "two"
    );
    Ok(())
}

#[test]
fn test_should_execute_shared_diamond_prerequisite_once() -> TestResult {
    let temp = fixture()?;
    app(
        temp.path(),
        r"tasks:
  base:
    - workspace: api
      command: python3 task.py base
  left:
    - workspace: api
      command: python3 task.py left base
      depends_on: [{project: apps.catalog, workspace: api, task: base}]
  right:
    - workspace: api
      command: python3 task.py right base
      depends_on: [{project: apps.catalog, workspace: api, task: base}]
  join:
    - workspace: api
      command: python3 task.py join left right
      depends_on:
        - {project: apps.catalog, workspace: api, task: left}
        - {project: apps.catalog, workspace: api, task: right}
",
    )?;
    write(
        temp.path(),
        "apps/catalog/task.py",
        r"import pathlib,sys
for name in sys.argv[2:]:
 if not pathlib.Path(name).exists(): sys.exit(7)
pathlib.Path(sys.argv[1]).touch(exist_ok=False)
",
    )?;
    for concurrency in ["1", "8"] {
        let output = cli(temp.path(), &["run", "join", "--concurrency", concurrency])?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(json(&output)?["outputs"].as_array().map(Vec::len), Some(4));
        for name in ["base", "left", "right", "join"] {
            fs::remove_file(temp.path().join("apps/catalog").join(name))?;
        }
    }
    Ok(())
}

#[test]
fn test_should_report_unowned_paths_and_reject_invalid_refs() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "")?;
    let output = cli(
        temp.path(),
        &["affected", "--changed-file", "unowned/source.py"],
    )?;
    assert!(
        json(&output)?["diagnostics"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item["code"] == "affected.unowned_file"))
    );
    let output = cli(
        temp.path(),
        &["affected", "--base=--output=bad", "--head", "HEAD"],
    )?;
    assert!(!output.status.success());
    Ok(())
}

#[test]
fn test_should_reject_unresolvable_adoption_workspace_dependencies() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "consumer/Cargo.toml",
        "[package]\nname='consumer'\nversion='0.1.0'\n[dependencies]\nshared={workspace=true,\
         features=['serde']}\n",
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", "plan", "--source"])
        .arg(&source)
        .arg("--dest")
        .arg(temp.path())
        .args(["--ci", "off", "--format", "json"])
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("adoption.workspace_dependency"));
    Ok(())
}

#[test]
fn test_should_include_declared_document_directories_and_exclude_secrets() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "ai:\n  docs: [docs]\n")?;
    write(
        temp.path(),
        "apps/catalog/docs/guide.md",
        "owned documentation",
    )?;
    write(temp.path(), "apps/catalog/.env", "SECRET=never-include")?;
    let output = cli(
        temp.path(),
        &[
            "context",
            "apps.catalog",
            "--include-docs",
            "--changed-file",
            "apps/catalog/.env",
            "--max-bytes",
            "4096",
        ],
    )?;
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout)?;
    assert!(text.contains("owned documentation"));
    assert!(!text.contains("SECRET=never-include"));
    assert!(text.len() <= 4096);
    Ok(())
}

#[test]
fn test_should_cap_all_document_entries_across_declared_directories() -> TestResult {
    for prefix in ["empty", ".env-empty"] {
        let temp = fixture()?;
        app(temp.path(), "ai:\n  docs: [docs-one, docs-two]\n")?;
        for directory in ["docs-one", "docs-two"] {
            write(
                temp.path(),
                &format!("apps/catalog/{directory}/guide.md"),
                "declared documentation",
            )?;
        }
        let args = [
            "context",
            "apps.catalog",
            "--include-docs",
            "--max-bytes",
            "4096",
        ];
        let output = cli(temp.path(), &args)?;
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("declared documentation"));
        for directory in ["docs-one", "docs-two"] {
            let root = temp.path().join(format!("apps/catalog/{directory}"));
            for index in 0..2050 {
                fs::create_dir(root.join(format!("{prefix}-{index}")))?;
            }
        }
        let output = cli(temp.path(), &args)?;
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("documentation traversal exceeds 4096 entries")
        );
    }
    Ok(())
}
