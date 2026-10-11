//! Execute migration follow-up commands and lockfile-owner CI without registry access.

use std::{fs, process::Command};

use super::{
    super::{TestResult, cli, fixture, json, write},
    adoption_review::{adoption_command, plan},
};

#[cfg(unix)]
#[test]
fn test_should_install_adopted_child_packages_at_the_root_lockfile_owner() -> TestResult {
    use std::os::unix::fs::PermissionsExt;

    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(&source, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n")?;
    write(
        &source,
        "packages/web/package.json",
        r#"{"name":"web","scripts":{"check":"echo check"}}"#,
    )?;
    let data = json(&plan(
        &source,
        temp.path(),
        &["--map", "source=apps/source"],
    )?)?;
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    write(
        temp.path(),
        "bin/pnpm",
        r"#!/usr/bin/env python3
import pathlib,sys
with pathlib.Path('calls').open('a') as f:
 f.write(' '.join(sys.argv[1:])+'\n')
",
    )?;
    fs::set_permissions(
        temp.path().join("bin/pnpm"),
        fs::Permissions::from_mode(0o755),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "ci",
            "run-step",
            "--project",
            "apps.source",
            "--workspace",
            "packages-web",
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
        "install --frozen-lockfile\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/source/packages/web/calls"))?,
        "run check\n"
    );
    Ok(())
}

#[test]
fn test_should_schedule_one_root_refresh_for_multiple_rewritten_child_packages() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "shared/package.json",
        r#"{"name":"shared","version":"1.0.0"}"#,
    )?;
    write(
        &source,
        "consumer/package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(&source, "consumer/package-lock.json", "{}")?;
    for name in ["one", "two"] {
        write(
            &source,
            &format!("consumer/packages/{name}/package.json"),
            &format!(
                r#"{{"name":"{name}","version":"1.0.0","dependencies":{{"shared":"^1.0.0"}}}}"#
            ),
        )?;
    }
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
    assert!(output.status.success());
    let data = json(&output)?;
    assert_eq!(data["lockfileRefreshes"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        data["lockfileRefreshes"][0]["command"]["cwd"],
        "apps/consumer"
    );
    assert_eq!(data["dependencyRewrites"].as_array().map(Vec::len), Some(2));
    assert!(
        !data["operations"]
            .as_array()
            .ok_or("operations missing")?
            .iter()
            .any(|op| op["destinationPath"] == "apps/consumer/package-lock.json")
    );
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    assert!(
        adoption_command(temp.path(), "apply", &plan_path)?
            .status
            .success()
    );
    let rewritten: serde_json::Value = serde_json::from_slice(&fs::read(
        temp.path().join("apps/consumer/packages/one/package.json"),
    )?)?;
    assert_eq!(
        rewritten["dependencies"]["shared"],
        "file:../../../../frameworks/shared"
    );
    let output = cli(temp.path(), &["graph", "print"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
