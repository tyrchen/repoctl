//! Regression contracts for the PR's task-selection and resource-limit findings.

use std::{fmt::Write as _, fs, path::Path};

use super::super::{TestResult, app, cli, fixture, json, write};

#[test]
fn test_should_emit_only_selected_ci_roots_and_keep_prerequisite_toolchains() -> TestResult {
    let temp = fixture()?;
    write_ci_prerequisites(temp.path())?;
    let output = cli(
        temp.path(),
        &[
            "ci",
            "matrix",
            "--tasks",
            "check",
            "--changed-file",
            "apps/catalog/check.py",
        ],
    )?;
    assert!(output.status.success());
    let data = json(&output)?;
    let entries = data["entries"].as_array().ok_or("missing entries")?;
    assert_eq!(entries.len(), 1);
    let entry = entries.first().ok_or("missing root task")?;
    assert_eq!(entry["project"], "apps.catalog");
    assert_eq!(entry["task"], "check");
    assert_eq!(entry["needsNode"], true);
    assert_eq!(entry["needsPython"], true);
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
    assert_eq!(json(&step)?["outputs"].as_array().map(Vec::len), Some(4));
    assert_eq!(
        fs::read_to_string(temp.path().join("frameworks/tooling/calls"))?,
        "prepare-one\nprepare-two\ncheck\n"
    );
    let all = json(&cli(
        temp.path(),
        &["ci", "matrix", "--tasks", "check", "--fallback", "all"],
    )?)?;
    let all_entries = all["entries"].as_array().ok_or("missing all entries")?;
    assert_eq!(all_entries.len(), 2);
    assert!(all_entries.iter().all(|entry| entry["task"] == "check"));
    Ok(())
}

fn write_ci_prerequisites(root: &Path) -> TestResult {
    app(
        root,
        r"tasks:
  check:
    - workspace: api
      command: python3 check.py
      depends_on:
        - project: frameworks.tooling
          workspace: web
          task: check
",
    )?;
    write(root, "apps/catalog/check.py", "print('checked')\n")?;
    write(
        root,
        "frameworks/tooling/project.yaml",
        r"schema: company.project/v1
name: frameworks.tooling
kind: framework
path: frameworks/tooling
workspaces:
  - name: web
    language: typescript
    toolchain: npm
    root: .
    manifest: package.json
tasks:
  prepare:
    - workspace: web
      command: python3 record.py prepare-one
    - workspace: web
      command: python3 record.py prepare-two
  check:
    - workspace: web
      command: python3 record.py check
      depends_on:
        - project: frameworks.tooling
          workspace: web
          task: prepare
",
    )?;
    write(
        root,
        "frameworks/tooling/package.json",
        r#"{"name":"@local/tooling"}"#,
    )?;
    write(
        root,
        "frameworks/tooling/record.py",
        "import pathlib,sys\nwith pathlib.Path('calls').open('a') as f: \
         f.write(sys.argv[1]+'\\n')\n",
    )?;
    Ok(())
}

#[test]
fn test_should_bound_the_prerequisite_union_before_resolving_dependencies() -> TestResult {
    let temp = fixture()?;
    for group in 0..2 {
        let mut project = format!(
            "schema: company.project/v1\nname: frameworks.deps{group}\nkind: framework\npath: \
             frameworks/deps{group}\nworkspaces:\n  - name: api\n    language: python\n    root: \
             .\n    manifest: pyproject.toml\ntasks:\n"
        );
        for index in 0..128 {
            write!(
                project,
                "  prep{index}:\n    - workspace: api\n      command: python3 -c pass\n"
            )?;
        }
        write(
            temp.path(),
            &format!("frameworks/deps{group}/project.yaml"),
            &project,
        )?;
    }
    for count in [256, 257] {
        let tasks = prerequisite_union_tasks(count);
        app(temp.path(), &tasks)?;
        let output = cli(temp.path(), &["run", "check", "--dry-run"])?;
        if count == 256 {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let data = json(&output)?;
            let commands = data["commands"].as_array().ok_or("missing commands")?;
            assert_eq!(commands.len(), 258);
            assert!(
                commands
                    .iter()
                    .filter(|c| c["task"] == "check")
                    .all(|c| c["prerequisites"].as_array().map(Vec::len) == Some(256))
            );
        } else {
            assert!(!output.status.success());
            // deps2 does not exist: the union must be rejected before resolving any targets.
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("task.too_many_prerequisites")
            );
        }
    }
    Ok(())
}

fn prerequisite_union_tasks(count: usize) -> String {
    let mut tasks = String::from("tasks:\n  check:\n");
    for dependencies in [
        (0..128).collect::<Vec<_>>(),
        (0..127).chain(128..count).collect(),
    ] {
        tasks
            .push_str("    - workspace: api\n      command: python3 check.py\n      depends_on:\n");
        for dependency in dependencies {
            let _ = write!(
                tasks,
                "        - project: frameworks.deps{}\n          workspace: api\n          task: \
                 prep{}\n",
                dependency / 128,
                dependency % 128
            );
        }
    }
    tasks
}
