//! Deepest project ownership for multilingual package and local path edges.

use std::path::Path;

use super::super::{TestResult, cli, fixture, json, write};

#[test]
fn test_should_classify_dependencies_under_the_deepest_project_owner() -> TestResult {
    for (language, manifest) in [
        ("typescript", "package.json"),
        ("python", "pyproject.toml"),
        ("rust", "Cargo.toml"),
    ] {
        check_nested_owner(language, manifest)?;
    }
    Ok(())
}

fn check_nested_owner(language: &str, manifest: &str) -> TestResult {
    let temp = fixture()?;
    for (name, kind, path, public, helper) in [
        ("apps.catalog", "app", "apps/catalog", "", false),
        (
            "frameworks.aaa",
            "framework",
            "frameworks/container",
            "public",
            false,
        ),
        (
            "frameworks.zzz",
            "framework",
            "frameworks/container/public/nested",
            "api",
            true,
        ),
    ] {
        let extra = if helper {
            format!(
                "  - name: helper\n    language: {language}\n    root: helper\n    manifest: \
                 helper/{manifest}\n"
            )
        } else {
            String::new()
        };
        let areas = if public.is_empty() {
            String::new()
        } else {
            format!("public_facades:\n  library: [{public}]\n")
        };
        write(
            temp.path(),
            &format!("{path}/project.yaml"),
            &format!(
                "schema: company.project/v1\nname: {name}\nkind: {kind}\npath: \
                 {path}\n{areas}workspaces:\n  - name: main\n    language: {language}\n    root: \
                 .\n    manifest: {manifest}\n{extra}"
            ),
        )?;
    }
    write_packages(temp.path(), language, manifest)?;
    let output = cli(temp.path(), &["graph", "print"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let graph = json(&output)?;
    let edges = graph["snapshot"]["graph"]["edges"]
        .as_array()
        .ok_or("edges missing")?;
    assert!(
        edges
            .iter()
            .any(|edge| edge["from"] == "project:apps.catalog"
                && edge["to"] == "project:frameworks.zzz"
                && edge["kind"] == "uses-framework-internal")
    );
    assert!(
        !edges
            .iter()
            .any(|edge| (edge["from"] == "project:apps.catalog"
                || edge["from"] == "project:frameworks.zzz")
                && edge["to"] == "project:frameworks.aaa")
    );
    // The nested project's private root must not inherit its parent's public policy.
    let validated = cli(temp.path(), &["graph", "validate", "--mode", "metadata"])?;
    assert!(!validated.status.success());
    assert!(
        String::from_utf8_lossy(&validated.stdout).contains("policy.framework_internal_dependency")
            || String::from_utf8_lossy(&validated.stderr)
                .contains("policy.framework_internal_dependency"),
        "{language}: {} {}",
        String::from_utf8_lossy(&validated.stdout),
        String::from_utf8_lossy(&validated.stderr)
    );
    Ok(())
}

fn write_packages(root: &Path, language: &str, manifest: &str) -> TestResult {
    let (consumer, outer, nested, helper) = match language {
        "typescript" => (
            r#"{"name":"consumer","dependencies":{"nested":"workspace:*","nested-path":"file:../../frameworks/container/public/nested"}}"#,
            r#"{"name":"outer"}"#,
            r#"{"name":"nested","dependencies":{"helper":"file:./helper"}}"#,
            r#"{"name":"helper"}"#,
        ),
        "python" => (
            r"[project]
name='consumer'
dependencies=['nested']
[tool.uv.sources]
nested-path={path='../../frameworks/container/public/nested'}
",
            r"[project]
name='outer'
",
            r"[project]
name='nested'
dependencies=['helper']
[tool.uv.sources]
helper={path='helper'}
",
            r"[project]
name='helper'
",
        ),
        _ => (
            r"[package]
name='consumer'
version='0.1.0'
[dependencies]
nested={path='../../frameworks/container/public/nested'}
",
            r"[package]
name='outer'
version='0.1.0'
",
            r"[package]
name='nested'
version='0.1.0'
[dependencies]
helper={path='helper'}
",
            r"[package]
name='helper'
version='0.1.0'
",
        ),
    };
    for (directory, content) in [
        ("apps/catalog", consumer),
        ("frameworks/container", outer),
        ("frameworks/container/public/nested", nested),
        ("frameworks/container/public/nested/helper", helper),
    ] {
        write(root, &format!("{directory}/{manifest}"), content)?;
    }
    Ok(())
}
