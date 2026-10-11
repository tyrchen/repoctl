//! Reviewed source commitments and explicit unlocked Node manager regressions.

use std::{fs, process::Command};
#[cfg(unix)]
use std::{os::unix::fs::PermissionsExt, path::Path};

use serde_json::Value;

use super::{
    super::{TestResult, app, cli, fixture, json, write},
    adoption_review::{adoption_command, plan},
};

#[test]
fn test_should_reject_changed_missing_and_uncommitted_copy_sources_before_any_write() -> TestResult
{
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "package.json", r#"{"name":"source"}"#)?;
    fs::write(source.join("binary.dat"), [0, 255, 10, 7])?;
    let output = plan(&source, temp.path(), &["--map", "source=apps/source"])?;
    assert!(output.status.success());
    let original = json(&output)?;
    let operations = original["operations"]
        .as_array()
        .ok_or("operations missing")?;
    assert!(
        operations
            .iter()
            .filter(|operation| operation["sourcePath"].is_string())
            .all(|operation| {
                operation["checksum"]
                    .as_str()
                    .is_some_and(|checksum| checksum.starts_with("blake3:") && checksum.len() == 71)
            })
    );
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&original)?)?;
    for missing in [false, true] {
        if missing {
            fs::remove_file(source.join("binary.dat"))?;
        } else {
            fs::write(source.join("binary.dat"), [0, 255, 10, 8])?;
        }
        let applied = adoption_command(temp.path(), "apply", &plan_path)?;
        assert!(!applied.status.success());
        if !missing {
            assert!(
                String::from_utf8_lossy(&applied.stderr).contains("adoption.copy.source_changed")
            );
        }
        assert!(!temp.path().join("apps").exists());
        assert!(!temp.path().join(".github").exists());
    }
    fs::write(source.join("binary.dat"), [0, 255, 10, 7])?;
    let mut unsigned = original.clone();
    for operation in unsigned["operations"]
        .as_array_mut()
        .ok_or("operations missing")?
    {
        if operation["sourcePath"].is_string() {
            operation
                .as_object_mut()
                .ok_or("operation missing")?
                .remove("checksum");
        }
    }
    fs::write(&plan_path, serde_json::to_vec(&unsigned)?)?;
    let applied = adoption_command(temp.path(), "apply", &plan_path)?;
    assert!(!applied.status.success());
    assert!(String::from_utf8_lossy(&applied.stderr).contains("adoption.copy.checksum"));
    assert!(!temp.path().join("apps").exists());
    fs::write(&plan_path, serde_json::to_vec(&original)?)?;
    let applied = adoption_command(temp.path(), "apply", &plan_path)?;
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(
        fs::read(temp.path().join("apps/source/binary.dat"))?,
        [0, 255, 10, 7]
    );
    Ok(())
}

#[test]
fn test_should_validate_original_bytes_of_rewritten_manifest_operations() -> TestResult {
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
        r#"{"name":"consumer","dependencies":{"shared":"^1.0.0"}}"#,
    )?;
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
    let rewritten = data["operations"]
        .as_array()
        .ok_or("operations missing")?
        .iter()
        .find(|operation| operation["destinationPath"] == "apps/consumer/package.json")
        .ok_or("rewrite missing")?;
    assert!(
        rewritten["content"]
            .as_str()
            .is_some_and(|content| content.contains("file:"))
    );
    assert!(rewritten["checksum"].is_string());
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    write(
        &source,
        "consumer/package.json",
        r#"{"name":"consumer","dependencies":{"shared":"^2.0.0"}}"#,
    )?;
    let applied = adoption_command(temp.path(), "apply", &plan_path)?;
    assert!(!applied.status.success());
    assert!(String::from_utf8_lossy(&applied.stderr).contains("adoption.copy.source_changed"));
    assert!(!temp.path().join("apps").exists());
    assert!(!temp.path().join("frameworks").exists());
    Ok(())
}

#[test]
fn test_should_use_nearest_declared_node_manager_without_a_lockfile() -> TestResult {
    for manager in ["pnpm", "yarn", "bun", "npm"] {
        check_unlocked_manager(manager)?;
    }
    Ok(())
}

fn check_unlocked_manager(manager: &str) -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "package.json",
        &format!(
            r#"{{"name":"root","packageManager":"{manager}@1.0.0","scripts":{{"build":"echo build","check":"echo check"}}}}"#
        ),
    )?;
    write(
        &source,
        "packages/inherited/package.json",
        r#"{"name":"inherited","scripts":{"build":"echo build","check":"echo check"}}"#,
    )?;
    write(
        &source,
        "packages/override/package.json",
        r#"{"name":"override","packageManager":"npm@11.0.0","scripts":{"build":"echo build","check":"echo check"}}"#,
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", "plan", "--source"])
        .arg(&source)
        .arg("--dest")
        .arg(temp.path())
        .args([
            "--map",
            "source=apps/source",
            "--ci",
            "off",
            "--verification",
            "full",
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
    let manifest: Value = serde_norway::from_str(
        data["manifestSyntheses"][0]["content"]
            .as_str()
            .ok_or("manifest missing")?,
    )?;
    for workspace in manifest["workspaces"]
        .as_array()
        .ok_or("workspaces missing")?
    {
        let expected = if workspace["root"] == "packages/override" {
            "npm"
        } else {
            manager
        };
        assert_eq!(workspace["toolchain"], expected);
        assert!(workspace["lockfile"].is_null());
    }
    let commands = data["verification"]["commands"]
        .as_array()
        .ok_or("commands missing")?;
    assert_eq!(
        commands
            .iter()
            .filter(|command| command["program"] != "repoctl")
            .count(),
        3
    );
    for command in commands
        .iter()
        .filter(|command| command["program"] != "repoctl")
    {
        let expected = if command["cwd"] == "apps/source/packages/override" {
            "npm"
        } else {
            manager
        };
        assert_eq!(command["program"], expected);
    }
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    let matrix = json(&cli(
        temp.path(),
        &["ci", "matrix", "--tasks", "check", "--fallback", "all"],
    )?)?;
    assert_eq!(matrix["entries"].as_array().map(Vec::len), Some(3));
    let content = data["manifestSyntheses"][0]["content"]
        .as_str()
        .ok_or("manifest missing")?;
    assert!(content.contains(&format!("command: {manager} run check")));
    #[cfg(unix)]
    check_unlocked_install(temp.path(), manager)?;
    Ok(())
}

#[cfg(unix)]
fn check_unlocked_install(root: &Path, manager: &str) -> TestResult {
    for toolchain in ["npm", "pnpm", "yarn", "bun"] {
        let relative = format!("bin/{toolchain}");
        write(
            root,
            &relative,
            r"#!/usr/bin/env python3
import pathlib,sys
with pathlib.Path('calls').open('a') as f:
 f.write(pathlib.Path(sys.argv[0]).name+' '+' '.join(sys.argv[1:])+'\n')
",
        )?;
        fs::set_permissions(root.join(relative), fs::Permissions::from_mode(0o755))?;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "ci",
            "run-step",
            "--project",
            "apps.source",
            "--workspace",
            "packages-inherited",
            "--task",
            "check",
            "--install",
            "--repo",
        ])
        .arg(root)
        .args(["--format", "json"])
        .env(
            "PATH",
            format!("{}:{}", root.join("bin").display(), std::env::var("PATH")?),
        )
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(root.join("apps/source/packages/inherited/calls"))?;
    assert!(calls.starts_with(&format!("{manager} install")));
    assert!(calls.ends_with(&format!("{manager} run check\n")));
    Ok(())
}

#[test]
fn test_should_reject_invalid_unlocked_package_manager_declarations() -> TestResult {
    for declaration in [
        "false".to_string(),
        "\"unknown@1.0\"".to_string(),
        "\"pnpm@\"".to_string(),
        format!("\"pnpm@{}\"", "1".repeat(257)),
    ] {
        let temp = fixture()?;
        let source = temp.path().join("source");
        write(
            &source,
            "package.json",
            &format!(r#"{{"name":"source","packageManager":{declaration}}}"#),
        )?;
        let output = plan(&source, temp.path(), &["--map", "source=apps/source"])?;
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("adoption.node.package_manager"));
        assert!(!temp.path().join("apps").exists());
    }
    Ok(())
}

#[test]
fn test_should_invalidate_cache_when_root_workspace_configuration_changes() -> TestResult {
    let temp = fixture()?;
    app(temp.path(), "tasks:\n  build:\n    - workspace: api\n      command: python3 build.py\n      cache:\n        enabled: true\n        inputs: [apps/catalog/src/**]\n        outputs: [apps/catalog/out]\n")?;
    write(temp.path(), "apps/catalog/src/input", "input")?;
    write(
        temp.path(),
        "apps/catalog/build.py",
        "import pathlib\np=pathlib.Path('out')\np.mkdir(exist_ok=True)\n(p/'result').write_text('\
         result')\n",
    )?;
    for (config, original, changed) in [
        (
            ".cargo/config.toml",
            "[build]\njobs=1\n",
            "[build]\njobs=2\n",
        ),
        (".cargo/config", "[build]\njobs=1\n", "[build]\njobs=2\n"),
        (
            "pnpm-workspace.yaml",
            "packages: ['apps/*']\n",
            "packages: ['apps/**']\n",
        ),
    ] {
        write(temp.path(), config, original)?;
        assert_eq!(
            json(&cli(temp.path(), &["run", "build"])?)?["outputs"][0]["output"]["cached"],
            false
        );
        assert_eq!(
            json(&cli(temp.path(), &["run", "build"])?)?["outputs"][0]["output"]["cached"],
            true
        );
        write(temp.path(), config, changed)?;
        assert_eq!(
            json(&cli(temp.path(), &["run", "build"])?)?["outputs"][0]["output"]["cached"],
            false
        );
    }
    Ok(())
}
