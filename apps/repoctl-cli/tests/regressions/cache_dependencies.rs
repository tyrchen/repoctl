//! Cache dependents require artifact-aware fingerprints for every prerequisite command.

use std::{fs, path::Path, process::Command};

use serde_json::Value;

use super::super::{TestResult, app, fixture, json, write};

#[test]
fn test_should_bypass_cached_descendants_of_a_silent_uncached_generator() -> TestResult {
    let temp = fixture()?;
    write_generators(temp.path())?;
    app(temp.path(), &tasks(false, false))?;
    for value in ["one", "two", "two"] {
        let output = run(temp.path(), value)?;
        for task in ["assemble", "build"] {
            let result = task_output(&output, task)?;
            assert_eq!(result["cached"], false);
            assert!(result["fingerprint"].is_null());
            assert!(
                result["cacheWarning"]
                    .as_str()
                    .is_some_and(|text| text.contains("artifact-aware"))
            );
        }
        assert_eq!(
            fs::read_to_string(temp.path().join("apps/catalog/out/result"))?,
            value
        );
    }
    app(temp.path(), &tasks(true, false))?;
    let initial = run(temp.path(), "three")?;
    assert_eq!(task_output(&initial, "build")?["cached"], false);
    let hit = run(temp.path(), "three")?;
    assert_eq!(task_output(&hit, "prepare")?["cached"], true);
    assert_eq!(task_output(&hit, "assemble")?["cached"], true);
    assert_eq!(task_output(&hit, "build")?["cached"], true);
    let changed = run(temp.path(), "four")?;
    assert_eq!(task_output(&changed, "build")?["cached"], false);
    assert_eq!(
        fs::read_to_string(temp.path().join("apps/catalog/out/result"))?,
        "four"
    );
    Ok(())
}

#[test]
fn test_should_bypass_when_one_command_in_a_prerequisite_has_no_fingerprint() -> TestResult {
    let temp = fixture()?;
    write_generators(temp.path())?;
    app(temp.path(), &tasks(true, true))?;
    for _ in 0..2 {
        let output = run(temp.path(), "mixed")?;
        assert!(task_output(&output, "prepare")?["fingerprint"].is_string());
        assert_eq!(task_output(&output, "build")?["cached"], false);
        assert!(task_output(&output, "build")?["cacheWarning"].is_string());
    }
    Ok(())
}

fn write_generators(root: &Path) -> TestResult {
    write(root, "apps/catalog/src/input", "input")?;
    write(
        root,
        "apps/catalog/generate.py",
        r"import pathlib,os
p=pathlib.Path('dist')
p.mkdir(exist_ok=True)
(p/'generated').write_text(os.environ['REPOCTL_GENERATED_VALUE'])
",
    )?;
    write(
        root,
        "apps/catalog/copy.py",
        r"import pathlib,sys
source=pathlib.Path(sys.argv[1])
target=pathlib.Path(sys.argv[2])
target.parent.mkdir(exist_ok=True)
target.write_text(source.read_text())
",
    )?;
    Ok(())
}

fn tasks(cached: bool, mixed: bool) -> String {
    let prepare_cache = if cached {
        r"      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/dist]
        env: [REPOCTL_GENERATED_VALUE]
"
    } else {
        ""
    };
    let extra = if mixed {
        "    - workspace: api\n      command: python3 -c pass\n"
    } else {
        ""
    };
    format!(
        r"tasks:
  prepare:
    - workspace: api
      command: python3 generate.py
{prepare_cache}{extra}  assemble:
    - workspace: api
      command: python3 copy.py dist/generated build/intermediate
      depends_on: [{{project: apps.catalog, workspace: api, task: prepare}}]
      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/build]
  build:
    - workspace: api
      command: python3 copy.py build/intermediate out/result
      depends_on: [{{project: apps.catalog, workspace: api, task: assemble}}]
      cache:
        enabled: true
        inputs: [apps/catalog/src/**]
        outputs: [apps/catalog/out]
"
    )
}

fn run(root: &Path, value: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["run", "build", "--format", "json", "--repo"])
        .arg(root)
        .env("REPOCTL_GENERATED_VALUE", value)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    json(&output)
}

fn task_output<'a>(data: &'a Value, task: &str) -> Result<&'a Value, Box<dyn std::error::Error>> {
    data["outputs"]
        .as_array()
        .ok_or("outputs missing")?
        .iter()
        .find(|output| output["task"] == task)
        .map(|output| &output["output"])
        .ok_or_else(|| "task output missing".into())
}
