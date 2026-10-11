//! Sequential command cache keys and adoption identity preflight regressions.

use std::{error::Error, fs, path::Path, process::Command};

use serde_json::Value;

use super::{
    super::{TestResult, app, fixture, json, write},
    adoption_review::{adoption_command, plan},
};

#[test]
fn test_should_invalidate_later_cached_commands_on_prior_environment_and_runtime_changes()
-> TestResult {
    check_sequential_cache(true)?;
    check_sequential_cache(false)?;
    Ok(())
}

fn check_sequential_cache(cached_generator: bool) -> TestResult {
    let temp = fixture()?;
    let cache = if cached_generator {
        r"      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/stageone]
        env: [REPOCTL_SEQUENCE_VALUE]
        runtime: [python3 runtime.py]
"
    } else {
        ""
    };
    app(
        temp.path(),
        &format!(
            r"tasks:
  build:
    - workspace: api
      command: python3 generate.py
{cache}    - workspace: api
      command: python3 copy.py stageone/result stagetwo/result
      cache:
        enabled: true
        inputs: [apps/catalog/src/**, apps/catalog/stageone/**]
        outputs: [apps/catalog/stagetwo]
    - workspace: api
      command: python3 copy.py stagetwo/result final/result
      cache:
        enabled: true
        inputs: [apps/catalog/src/**, apps/catalog/stagetwo/**]
        outputs: [apps/catalog/final]
"
        ),
    )?;
    write(temp.path(), "apps/catalog/src/input", "input")?;
    write(
        temp.path(),
        "apps/catalog/generate.py",
        r"import pathlib,os
p=pathlib.Path('stageone')
p.mkdir(exist_ok=True)
(p/'result').write_text(os.environ['REPOCTL_SEQUENCE_VALUE']+'/'+os.environ['REPOCTL_SEQUENCE_RUNTIME'])
",
    )?;
    write(
        temp.path(),
        "apps/catalog/runtime.py",
        "import os\nprint(os.environ['REPOCTL_SEQUENCE_RUNTIME'])\n",
    )?;
    write(
        temp.path(),
        "apps/catalog/copy.py",
        r"import pathlib,sys
source=pathlib.Path(sys.argv[1])
target=pathlib.Path(sys.argv[2])
target.parent.mkdir(exist_ok=True)
target.write_text(source.read_text())
",
    )?;
    for (value, runtime, hit) in [
        ("one", "r1", false),
        ("one", "r1", true),
        ("two", "r1", false),
        ("two", "r2", false),
        ("two", "r2", true),
    ] {
        let data = sequence_run(temp.path(), value, runtime)?;
        let outputs = data["outputs"].as_array().ok_or("outputs missing")?;
        assert_eq!(outputs.len(), 3);
        for result in outputs {
            assert_eq!(result["output"]["cached"], cached_generator && hit);
            if !cached_generator {
                assert!(result["output"]["fingerprint"].is_null());
            }
        }
        if !cached_generator {
            assert!(outputs.iter().skip(1).all(|result| {
                result["output"]["cacheWarning"]
                    .as_str()
                    .is_some_and(|warning| warning.contains("earlier command"))
            }));
        }
        assert_eq!(
            fs::read_to_string(temp.path().join("apps/catalog/final/result"))?,
            format!("{value}/{runtime}")
        );
    }
    Ok(())
}

fn sequence_run(root: &Path, value: &str, runtime: &str) -> Result<Value, Box<dyn Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["run", "build", "--repo"])
        .arg(root)
        .args(["--format", "json"])
        .env("REPOCTL_SEQUENCE_VALUE", value)
        .env("REPOCTL_SEQUENCE_RUNTIME", runtime)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    json(&output)
}

#[test]
fn test_should_reject_duplicate_project_identities_before_planning_or_applying() -> TestResult {
    let temp = fixture()?;
    let source = temp.path().join("sources");
    for repo in ["one", "two"] {
        write(
            &source,
            &format!("{repo}/package.json"),
            &format!(r#"{{"name":"{repo}"}}"#),
        )?;
    }
    let output = plan(
        &source,
        temp.path(),
        &[
            "--map",
            "one=apps/team-a/foo",
            "--map",
            "two=apps/team-b/foo",
        ],
    )?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("manifest.project.duplicate_name"));
    assert!(!temp.path().join("apps").exists());
    write_existing_project(temp.path())?;
    let output = plan(
        &source,
        temp.path(),
        &["--include", "one", "--map", "one=apps/team-a/foo"],
    )?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("manifest.project.duplicate_name"));
    assert!(!temp.path().join("apps/team-a").exists());
    fs::remove_dir_all(temp.path().join("apps/existing"))?;
    let output = plan(
        &source,
        temp.path(),
        &["--include", "one", "--map", "one=apps/team-a/foo"],
    )?;
    assert!(output.status.success());
    let data = json(&output)?;
    let plan_path = temp.path().join("plan.json");
    fs::write(&plan_path, serde_json::to_vec(&data)?)?;
    write_existing_project(temp.path())?;
    let output = adoption_command(temp.path(), "apply", &plan_path)?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("manifest.project.duplicate_name"));
    assert!(!temp.path().join("apps/team-a").exists());
    assert!(temp.path().join("apps/existing/project.yaml").is_file());
    Ok(())
}

fn write_existing_project(root: &Path) -> TestResult {
    write(
        root,
        "apps/existing/project.yaml",
        "schema: company.project/v1\nname: apps.foo\nkind: app\npath: apps/existing\nowners: \
         ['@platform']\n",
    )
}
