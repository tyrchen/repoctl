//! Regression coverage for migration containment, preview semantics and legacy Bun locks.

use std::{error::Error, fs, path::Path, process::Command};

use serde_json::Value;

use super::{
    super::{TestResult, fixture, json, write},
    adoption_review::{adoption_command, plan},
};

#[test]
fn test_should_exclude_nested_destination_from_candidates_and_single_source_traversal() -> TestResult
{
    check_nested_destination(false)?;
    check_nested_destination(true)?;
    Ok(())
}

fn check_nested_destination(single_source: bool) -> TestResult {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("collection");
    let destination = source.join("universe");
    write(
        &destination,
        "repo.yaml",
        "schema: company.repo/v1\nname: universe\nlayout: functional\n",
    )?;
    write(&destination, "README.md", "initialized destination")?;
    // A malformed destination manifest must never enter source indexing or synthesis.
    write(
        &destination,
        "package.json",
        "malformed destination package",
    )?;
    if single_source {
        write(&source, "package.json", r#"{"name":"collection"}"#)?;
    } else {
        write(&source, "library/package.json", r#"{"name":"library"}"#)?;
    }
    let name = if single_source {
        "collection"
    } else {
        "library"
    };
    let mapping = format!("{name}=apps/imported");
    let output = plan(
        &source.join("."),
        &destination.join("."),
        &["--map", &mapping],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    assert_eq!(data["sources"].as_array().map(Vec::len), Some(1));
    assert_eq!(data["sources"][0]["name"], name);
    let canonical_destination = fs::canonicalize(&destination)?;
    for operation in data["operations"].as_array().ok_or("operations missing")? {
        if let Some(path) = operation["sourcePath"].as_str() {
            assert!(!Path::new(path).starts_with(&canonical_destination));
        }
    }
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    let applied = adoption_command(&destination, "apply", &plan_path)?;
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(destination.join("apps/imported/package.json").is_file());
    assert!(!destination.join("apps/imported/universe").exists());
    assert_eq!(
        fs::read_to_string(destination.join("README.md"))?,
        "initialized destination"
    );
    assert!(!plan(&destination, &destination, &[])?.status.success());

    Ok(())
}

#[test]
fn test_should_preserve_ci_preview_semantics_after_saving_and_applying_a_plan() -> TestResult {
    for (mode, existing) in [
        ("report-only", true),
        ("report-only", false),
        ("off", true),
        ("update", true),
    ] {
        check_ci_preview(mode, existing)?;
    }
    Ok(())
}

fn check_ci_preview(mode: &str, existing: bool) -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "package.json", r#"{"name":"source"}"#)?;
    let workflow = temp.path().join(".github/workflows/repoctl.yml");
    if existing {
        write(
            temp.path(),
            ".github/workflows/repoctl.yml",
            "original workflow",
        )?;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", "plan", "--source"])
        .arg(&source)
        .arg("--dest")
        .arg(temp.path())
        .args([
            "--ci",
            mode,
            "--verification",
            "structural",
            "--format",
            "json",
            "--map",
            "source=apps/source",
        ])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    if mode == "off" {
        assert_eq!(data["ciOperations"].as_array().map(Vec::len), Some(0));
    } else {
        assert_eq!(
            data["ciOperations"][0]["operation"],
            if mode == "update" {
                "write-file"
            } else {
                "report-file"
            }
        );
        assert!(
            data["ciOperations"][0]["content"]
                .as_str()
                .is_some_and(|content| content.contains("jobs:"))
        );
    }
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    if mode == "update" {
        assert!(fs::read_to_string(workflow)?.contains("jobs:"));
    } else if existing {
        assert_eq!(fs::read_to_string(workflow)?, "original workflow");
    } else {
        assert!(!workflow.exists());
    }

    Ok(())
}

#[test]
fn test_should_preserve_binary_bun_locks_and_refresh_rewritten_locks_as_text() -> TestResult {
    check_bun_lock(false)?;
    check_bun_lock(true)?;
    Ok(())
}

fn check_bun_lock(rewritten: bool) -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("sources");
    write(
        &source,
        "shared/package.json",
        r#"{"name":"shared","version":"1.0.0"}"#,
    )?;
    write(
        &source,
        "consumer/package.json",
        r#"{"name":"consumer","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(
        &source,
        "consumer/packages/web/package.json",
        if rewritten {
            r#"{"name":"web","scripts":{"check":"echo ok"},"dependencies":{"shared":"^1.0.0"}}"#
        } else {
            r#"{"name":"web","scripts":{"check":"echo ok"}}"#
        },
    )?;
    let binary_lock = [0xff, 0x00, 0x80, 0x42];
    fs::write(source.join("consumer/bun.lockb"), binary_lock)?;
    let output = plan(
        &source,
        temp.path(),
        &[
            "--map",
            "consumer=apps/consumer",
            "--map",
            "shared=frameworks/shared",
        ],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    let content = assert_bun_plan(&data, rewritten)?;
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    if rewritten {
        assert!(!temp.path().join("apps/consumer/bun.lockb").exists());
        assert!(
            !adoption_command(temp.path(), "verify", &plan_path)?
                .status
                .success()
        );
        write(temp.path(), "apps/consumer/bun.lock", "{}")?;
        assert!(
            adoption_command(temp.path(), "verify", &plan_path)?
                .status
                .success()
        );
    } else {
        assert_eq!(
            fs::read(temp.path().join("apps/consumer/bun.lockb"))?,
            binary_lock
        );
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/consumer/project.yaml"))?,
        content
    );

    Ok(())
}

fn assert_bun_plan(data: &Value, rewritten: bool) -> Result<String, Box<dyn Error>> {
    let content = data["manifestSyntheses"]
        .as_array()
        .ok_or("syntheses missing")?
        .iter()
        .find(|synthesis| synthesis["source"] == "consumer")
        .and_then(|synthesis| synthesis["content"].as_str())
        .ok_or("consumer manifest missing")?;
    let manifest: Value = serde_norway::from_str(content)?;
    let workspaces = manifest["workspaces"]
        .as_array()
        .ok_or("workspaces missing")?;
    let expected_lock = if rewritten { "bun.lock" } else { "bun.lockb" };
    for workspace in workspaces {
        assert_eq!(workspace["toolchain"], "bun");
        assert_eq!(workspace["lockfile"], expected_lock);
    }
    assert!(content.contains("command: bun run check"));
    if rewritten {
        assert_eq!(data["lockfileRefreshes"].as_array().map(Vec::len), Some(1));
        let refresh = &data["lockfileRefreshes"][0];
        assert_eq!(refresh["lockfile"], "apps/consumer/bun.lock");
        assert_eq!(refresh["command"]["program"], "bun");
        assert_eq!(refresh["command"]["cwd"], "apps/consumer");
        assert!(
            refresh["command"]["args"]
                .as_array()
                .is_some_and(|args| args.contains(&Value::from("--save-text-lockfile")))
        );
    } else {
        assert_eq!(data["lockfileRefreshes"].as_array().map(Vec::len), Some(0));
    }
    Ok(content.to_string())
}

#[test]
fn test_should_reject_oversized_saved_operation_lists_before_creating_files() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "package.json", r#"{"name":"source"}"#)?;
    let mut data = json(&plan(
        &source,
        temp.path(),
        &["--map", "source=apps/source"],
    )?)?;
    let operation = data["operations"][0].clone();
    data["operations"] = Value::Array(vec![operation; 20_001]);
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    let output = adoption_command(temp.path(), "apply", &plan_path)?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("adoption.limit.operations"));
    assert!(!temp.path().join("apps/source").exists());
    Ok(())
}
