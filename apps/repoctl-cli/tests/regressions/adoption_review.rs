//! Regression coverage for virtual Cargo workspaces and safe Node adoption.

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;

use super::super::{TestResult, cli, fixture, json, write};

#[test]
fn test_should_discover_virtual_workspace_members_and_implicit_path_packages() -> TestResult {
    let temp = fixture()?;
    write(
        temp.path(),
        "apps/catalog/project.yaml",
        "schema: company.project/v1\nname: apps.catalog\nkind: app\npath: \
         apps/catalog\nworkspaces:\n  - name: rust\n    language: rust\n    root: .\n    \
         manifest: Cargo.toml\n",
    )?;
    write(
        temp.path(),
        "frameworks/shared/project.yaml",
        "schema: company.project/v1\nname: frameworks.shared\nkind: framework\npath: \
         frameworks/shared\nworkspaces:\n  - name: rust\n    language: rust\n    root: .\n    \
         manifest: Cargo.toml\npublic_facades:\n  rust: [.]\n",
    )?;
    write(
        temp.path(),
        "frameworks/shared/Cargo.toml",
        "[package]\nname='shared'\nversion='0.1.0'\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/Cargo.toml",
        r"[workspace]
members=['crates/*','explicit','explicit-ignored','../aux/*']
exclude=['crates/ignored','explicit-ignored']
[workspace.dependencies]
shared={path='../../frameworks/shared'}
",
    )?;
    write(
        temp.path(),
        "apps/catalog/crates/one/Cargo.toml",
        r"[package]
name='one'
version='0.1.0'
[dependencies]
shared={workspace=true}
helper={path='../../helper'}
",
    )?;
    write(
        temp.path(),
        "apps/catalog/crates/ignored/Cargo.toml",
        "malformed excluded manifest",
    )?;
    write(
        temp.path(),
        "apps/catalog/explicit/Cargo.toml",
        r"[package]
name='explicit'
version='0.1.0'
[target.'cfg(unix)'.build-dependencies]
shared={path='../../../frameworks/shared'}
",
    )?;
    write(
        temp.path(),
        "apps/catalog/helper/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\n[dev-dependencies]\nshared={path='../../../\
         frameworks/shared'}\n",
    )?;

    write(
        temp.path(),
        "apps/catalog/explicit-ignored/Cargo.toml",
        "malformed excluded explicit member",
    )?;
    write(
        temp.path(),
        "apps/aux/member/Cargo.toml",
        r"[package]
name='sibling'
version='0.1.0'
workspace='../../catalog'
[dependencies]
shared={workspace=true}
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
        .ok_or("edges missing")?;
    assert_eq!(
        edges
            .iter()
            .filter(|edge| edge["to"] == "project:frameworks.shared")
            .count(),
        4
    );
    let affected = json(&cli(
        temp.path(),
        &["affected", "--changed-file", "frameworks/shared/src/lib.rs"],
    )?)?;
    assert!(
        affected["transitivelyAffected"]
            .as_array()
            .is_some_and(|values| values.contains(&Value::from("apps.catalog")))
    );
    write(
        temp.path(),
        "apps/catalog/Cargo.toml",
        "[workspace]\nmembers=['missing']\n",
    )?;
    assert!(!cli(temp.path(), &["graph", "print"])?.status.success());
    write(
        temp.path(),
        "apps/catalog/Cargo.toml",
        "[workspace]\nmembers=['../../../../missing-escape/*']\n",
    )?;
    let output = cli(temp.path(), &["graph", "print"])?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cargo members escape repository"));

    Ok(())
}

#[test]
fn test_should_inherit_node_lockfile_owners_without_inventing_locks() -> TestResult {
    for (toolchain, lock) in [
        ("pnpm", "pnpm-lock.yaml"),
        ("yarn", "yarn.lock"),
        ("npm", "package-lock.json"),
    ] {
        check_node_lock_owner(toolchain, lock)?;
    }
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "package.json", r#"{"name":"unlocked"}"#)?;
    let data = json(&plan(
        &source,
        temp.path(),
        &["--map", "source=apps/source"],
    )?)?;
    assert!(
        !data["manifestSyntheses"][0]["content"]
            .as_str()
            .ok_or("manifest missing")?
            .contains("lockfile:")
    );
    Ok(())
}

#[test]
fn test_should_invalidate_rewritten_node_locks_and_require_an_explicit_refresh() -> TestResult {
    for (toolchain, lock, lock_content) in [
        ("npm", "package-lock.json", "{}"),
        ("pnpm", "pnpm-lock.yaml", "lockfileVersion: '9.0'"),
        ("yarn", "yarn.lock", "# yarn lockfile v1"),
        ("yarn", "yarn.lock", "__metadata:\n  version: 8"),
        ("bun", "bun.lock", "{}"),
    ] {
        check_rewritten_lock_refresh(toolchain, lock, lock_content)?;
    }
    Ok(())
}

#[test]
fn test_should_reject_existing_and_duplicate_manifests_before_copying() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(&source, "package.json", r#"{"name":"new-source"}"#)?;
    write(
        temp.path(),
        "apps/existing/project.yaml",
        "original manifest",
    )?;
    let output = plan(&source, temp.path(), &["--map", "source=apps/existing"])?;
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/existing/project.yaml"))?,
        "original manifest"
    );
    assert!(!temp.path().join("apps/existing/package.json").exists());
    let output = plan(&source, temp.path(), &["--map", "source=apps/new"])?;
    assert!(output.status.success());
    let data = json(&output)?;
    let path = temp.path().join("plan.json");
    fs::write(&path, serde_json::to_vec(&data)?)?;
    write(temp.path(), "apps/new/project.yaml", "appeared after plan")?;
    assert!(
        !adoption_command(temp.path(), "apply", &path)?
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/new/project.yaml"))?,
        "appeared after plan"
    );
    assert!(!temp.path().join("apps/new/package.json").exists());
    fs::remove_file(temp.path().join("apps/new/project.yaml"))?;
    let mut duplicate = data;
    let operations = duplicate["operations"]
        .as_array_mut()
        .ok_or("operations missing")?;
    let manifest = operations
        .iter()
        .find(|op| op["destinationPath"] == "apps/new/project.yaml")
        .cloned()
        .ok_or("manifest operation missing")?;
    operations.push(manifest);
    fs::write(&path, serde_json::to_vec(&duplicate)?)?;
    assert!(
        !adoption_command(temp.path(), "apply", &path)?
            .status
            .success()
    );
    assert!(!temp.path().join("apps/new/package.json").exists());
    duplicate["operations"]
        .as_array_mut()
        .ok_or("operations missing")?
        .pop();
    duplicate["ciOperations"] = serde_json::json!([{
        "path": "apps/existing/project.yaml", "operation": "write-file", "content": "overwrite"
    }]);
    fs::write(&path, serde_json::to_vec(&duplicate)?)?;
    assert!(
        !adoption_command(temp.path(), "apply", &path)?
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/existing/project.yaml"))?,
        "original manifest"
    );
    assert!(!temp.path().join("apps/new/package.json").exists());

    Ok(())
}

pub(super) fn plan(
    source: &Path,
    destination: &Path,
    options: &[&str],
) -> Result<Output, Box<dyn std::error::Error>> {
    plan_with_verification(source, destination, options, "structural")
}

fn plan_with_verification(
    source: &Path,
    destination: &Path,
    options: &[&str],
    verification: &str,
) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", "plan", "--source"])
        .arg(source)
        .arg("--dest")
        .arg(destination)
        .args([
            "--ci",
            "off",
            "--verification",
            verification,
            "--format",
            "json",
        ])
        .args(options)
        .output()?)
}

pub(super) fn adoption_command(
    root: &Path,
    operation: &str,
    plan: &Path,
) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["adopt", operation, "--plan"])
        .arg(plan)
        .args(["--format", "json"])
        .current_dir(root)
        .output()?)
}

fn check_node_lock_owner(toolchain: &str, lock: &str) -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("source");
    write(
        &source,
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    )?;
    write(&source, lock, "lockfile fixture")?;
    write(
        &source,
        "packages/web/package.json",
        r#"{"name":"web","scripts":{"check":"echo check","build":"echo build"}}"#,
    )?;
    write(
        &source,
        "packages/standalone/package.json",
        r#"{"name":"standalone","scripts":{"check":"echo check"}}"#,
    )?;
    write(&source, "packages/standalone/bun.lock", "bun fixture")?;
    let output = plan_with_verification(
        &source,
        temp.path(),
        &["--map", "source=apps/source"],
        "full",
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data = json(&output)?;
    let content = data["manifestSyntheses"][0]["content"]
        .as_str()
        .ok_or("manifest missing")?;
    let manifest: Value = serde_norway::from_str(content)?;
    let web = manifest["workspaces"]
        .as_array()
        .ok_or("workspaces missing")?
        .iter()
        .find(|workspace| workspace["root"] == "packages/web")
        .ok_or("web missing")?;
    assert_eq!(web["toolchain"], toolchain);
    assert_eq!(web["lockfile"], lock);
    assert!(content.contains(&format!("command: {toolchain} run check")));
    let build = data["verification"]["commands"]
        .as_array()
        .ok_or("verification commands missing")?
        .iter()
        .find(|command| command["program"] == toolchain)
        .ok_or("Node build missing")?;
    assert_eq!(build["cwd"], "apps/source/packages/web");
    assert_eq!(build["args"], serde_json::json!(["run", "build"]));
    assert!(content.contains("toolchain: bun"));
    assert!(!content.contains("packages/web/package-lock.json"));
    Ok(())
}

fn check_rewritten_lock_refresh(toolchain: &str, lock: &str, lock_content: &str) -> TestResult {
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
        r#"{"name":"consumer","version":"1.0.0","dependencies":{"shared":"^1.0.0"}}"#,
    )?;
    write(&source, &format!("consumer/{lock}"), lock_content)?;
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
    let refreshes = data["lockfileRefreshes"]
        .as_array()
        .ok_or("refreshes missing")?;
    assert_eq!(refreshes.len(), 1);
    let refresh = refreshes.first().ok_or("refresh missing")?;
    assert_eq!(refresh["command"]["program"], toolchain);
    assert_eq!(refresh["command"]["cwd"], "apps/consumer");
    assert_eq!(refresh["lockfile"], format!("apps/consumer/{lock}"));
    assert!(
        !data["operations"]
            .as_array()
            .ok_or("operations missing")?
            .iter()
            .any(|op| op["destinationPath"] == refresh["lockfile"])
    );
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    let applied = adoption_command(temp.path(), "apply", &plan_path)?;
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert!(!temp.path().join(format!("apps/consumer/{lock}")).exists());
    let verified = adoption_command(temp.path(), "verify", &plan_path)?;
    assert!(!verified.status.success());
    assert!(
        String::from_utf8_lossy(&verified.stdout).contains("adoption.lockfile.refresh_required")
    );
    Ok(())
}
