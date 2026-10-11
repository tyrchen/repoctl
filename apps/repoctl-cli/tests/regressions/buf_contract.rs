//! Optional integration contract against an installed, real Buf executable.

use std::{fs, process::Command};

use crate::{TestResult, fixture, git, json, write};

#[test]
#[ignore = "requires REPOCTL_TEST_BUF pointing at an installed Buf binary"]
fn test_should_detect_a_real_buf_breaking_change() -> TestResult {
    let temp = fixture()?;
    let binary = std::env::var("REPOCTL_TEST_BUF")?;
    fs::create_dir_all(temp.path().join("bin"))?;
    fs::copy(binary, temp.path().join("bin/buf"))?;
    let config =
        "version: v2\nmodules:\n  - path: .\nlint:\n  use: [MINIMAL]\nbreaking:\n  use: [FILE]\n";
    write(temp.path(), "baseline/buf.yaml", config)?;
    write(temp.path(), "protos/buf.yaml", config)?;
    write(
        temp.path(),
        "baseline/acme/v1/service.proto",
        "syntax = \"proto3\";\npackage acme.v1;\nmessage Request { string name = 1; }\n",
    )?;
    write(
        temp.path(),
        "protos/acme/v1/service.proto",
        "syntax = \"proto3\";\npackage acme.v1;\nmessage Request {}\n",
    )?;
    let path = format!(
        "{}:{}",
        temp.path().join("bin").display(),
        std::env::var("PATH")?
    );
    let output = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args(["proto", "check", "--execute", "--against"])
        .arg(temp.path().join("baseline"))
        .args(["--format", "json", "--repo"])
        .arg(temp.path())
        .env("PATH", path)
        .output()?;
    assert!(!output.status.success());
    let data = json(&output)?;
    assert_eq!(data["outputs"][0]["status"], 0);
    assert_ne!(data["outputs"][1]["status"], 0);
    let output = &data["outputs"][1];
    let message = format!(
        "{} {}",
        output["stdout"].as_str().unwrap_or(""),
        output["stderr"].as_str().unwrap_or("")
    )
    .to_lowercase();
    assert!(
        message.contains("name") || message.contains("field"),
        "{message}"
    );
    write(
        temp.path(),
        "protos/acme/v1/service.proto",
        "syntax = \"proto3\";\npackage acme.v1;\nmessage Request { string name = 1; }\n",
    )?;
    git(temp.path(), &["init", "-b", "main"])?;
    git(
        temp.path(),
        &["config", "user.email", "repoctl-test@example.com"],
    )?;
    git(temp.path(), &["config", "user.name", "repoctl test"])?;
    git(temp.path(), &["add", "repo.yaml", "protos"])?;
    git(temp.path(), &["commit", "-m", "baseline"])?;
    write(
        temp.path(),
        "protos/acme/v1/service.proto",
        "syntax = \"proto3\";\npackage acme.v1;\nmessage Request {}\n",
    )?;
    let path = format!(
        "{}:{}",
        temp.path().join("bin").display(),
        std::env::var("PATH")?
    );
    let comparison = Command::new(env!("CARGO_BIN_EXE_repoctl"))
        .args([
            "proto",
            "check",
            "--execute",
            "--against",
            ".git#branch=main,subdir=protos",
            "--format",
            "json",
            "--repo",
        ])
        .arg(temp.path())
        .env("PATH", path)
        .output()?;
    let data = json(&comparison)?;
    assert_eq!(data["outputs"][0]["status"], 0);
    let output = &data["outputs"][1];
    assert_ne!(output["status"], 0);
    let message = format!(
        "{} {}",
        output["stdout"].as_str().unwrap_or(""),
        output["stderr"].as_str().unwrap_or("")
    )
    .to_lowercase();
    assert!(
        message.contains("name") || message.contains("field"),
        "{message}"
    );
    Ok(())
}
