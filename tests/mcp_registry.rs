//! `server.json` matches the MCP registry server schema dated 2025-12-11.
//!
//! The schema is the document at its `$id`. The test reads a cache under
//! `target/index-fixtures`, or downloads the document when that cache is
//! absent. A missing cache and a failed download with an empty body skips
//! the test. A present cache, and a download that returns a body, are
//! checked in full. The schema is not stored in git.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCHEMA_URL: &str =
    "https://static.modelcontextprotocol.io/schemas/2025-12-11/server.schema.json";

fn registry_schema() -> Option<String> {
    cached_text(
        &repo().join("target/index-fixtures/mcp-server.schema.json"),
        SCHEMA_URL,
        |text| text.contains("\"$id\""),
    )
}

/// Read `path` when it exists. Otherwise download `url`.
///
/// `None` means the cache is missing and the download failed before any
/// bytes arrived. The caller prints nothing further and returns. A present
/// file is returned as stored. A download that exits non-zero after writing
/// bytes, or that exits zero with a body `complete` rejects, fails the test
/// and does not replace the cache.
fn cached_text(path: &Path, url: &str, complete: impl Fn(&str) -> bool) -> Option<String> {
    if path.is_file() {
        return Some(std::fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("read {}: {error}", path.display());
        }));
    }
    let output = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--retry-delay", "1", url])
        .output()
        .unwrap_or_else(|error| panic!("curl {url}: {error}"));
    if !output.status.success() {
        if output.stdout.is_empty() {
            eprintln!(
                "skip: {url} was not downloaded and {} is missing: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return None;
        }
        panic!(
            "partial download of {url}: curl wrote {} bytes then failed: {}",
            output.stdout.len(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("download of {url} is not utf-8: {error}"));
    assert!(
        complete(&text),
        "download of {url} finished but the body is incomplete"
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
    }
    std::fs::write(path, &text).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    Some(text)
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn package_version(cargo: &str) -> String {
    let mut in_package = false;
    for line in cargo.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("version = ") {
            return rest.trim_matches('"').to_string();
        }
    }
    panic!("Cargo.toml has no [package] version");
}

#[test]
fn server_json_matches_the_registry_schema() {
    let root = repo();
    let Some(text) = registry_schema() else {
        return;
    };
    let schema: serde_json::Value = serde_json::from_str(&text).expect("parse schema");
    assert_eq!(schema["$id"].as_str(), Some(SCHEMA_URL));

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("server.json")).expect("read server.json"),
    )
    .expect("parse server.json");
    assert_eq!(manifest["$schema"], schema["$id"]);

    let validator = jsonschema::options()
        .build(&schema)
        .expect("schema compiles");
    let errors: Vec<String> = validator
        .iter_errors(&manifest)
        .map(|error| format!("{error} at {}", error.instance_path))
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));

    let cargo = std::fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    let version = package_version(&cargo);
    assert_eq!(manifest["version"].as_str(), Some(version.as_str()));
    assert_eq!(
        manifest["name"].as_str(),
        Some("io.github.HaoZeke/eb-stack")
    );
    let description = manifest["description"].as_str().expect("description");
    assert!(
        !description.is_empty() && description.len() <= 100,
        "{description}"
    );
    assert!(cargo.contains("name = \"eb-stack\""));

    let package = &manifest["packages"][0];
    assert_eq!(package["registryType"].as_str(), Some("cargo"));
    assert_eq!(
        package["registryBaseUrl"].as_str(),
        Some("https://crates.io")
    );
    assert_eq!(package["identifier"].as_str(), Some("eb_stack"));
    assert_eq!(package["version"].as_str(), Some(version.as_str()));
    assert_eq!(package["transport"]["type"].as_str(), Some("stdio"));
    assert!(package.get("runtimeHint").is_none());
    let args = package["packageArguments"]
        .as_array()
        .expect("packageArguments");
    assert!(args
        .iter()
        .any(|arg| { arg["type"] == "positional" && arg["value"] == "mcp" }));
    assert!(args.iter().any(|arg| {
        arg["type"] == "named" && arg["name"] == "--workspace" && arg["isRequired"] == false
    }));

    let readme = std::fs::read_to_string(root.join("README.md")).expect("read README");
    assert!(
        readme.contains("mcp-name: io.github.HaoZeke/eb-stack"),
        "README must contain the visible registry name"
    );
}
