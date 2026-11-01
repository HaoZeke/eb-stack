//! The second-version check `stack batch` runs over each planned pull request.
//!
//! The fixture robot tree carries two GROMACS versions on the `foss-2026.1`
//! toolchain, which is generation 2026.1 of EESSI 2026.06. A batch that adds
//! one next to the other, whether the other is installed or in an earlier
//! batch of the same run, is a second version in one generation: a warning in
//! `pr-body.md` and `batch.json`, accepted with `--allow-versions`.

use eb_stack::mcp::handle_message;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/batch_versions_fixtures")
        .join(name)
}

fn path(name: &str) -> String {
    fixture(name).to_str().unwrap().to_string()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

/// The one batch directory a single-batch run wrote.
fn only_batch(out: &std::path::Path) -> PathBuf {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(out)
        .expect("out dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 1, "one batch directory: {dirs:?}");
    dirs.pop().unwrap()
}

/// The batch directories a run wrote, sorted by name.
fn batch_dirs(out: &std::path::Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(out)
        .expect("out dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn read_json(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read json")).expect("parse json")
}

/// A `stack batch` run over the GROMACS fixture tree. `extra` adds flags such
/// as `--allow-versions`; `installed` and the roots vary per test.
fn batch(
    out: &std::path::Path,
    installed: &str,
    roots: &[&str],
    extra: &[&str],
) -> std::process::Output {
    let mut args: Vec<String> = vec!["stack".into(), "batch".into()];
    for root in roots {
        args.push("--root".into());
        args.push((*root).into());
    }
    args.push("--easyconfigs".into());
    args.push(path("easyconfigs"));
    args.push("--installed".into());
    args.push(path(installed));
    for flag in [
        "--eessi-version",
        "2026.06",
        "--easybuild-version",
        "5.4.1",
        "--generation",
        "2026.1",
        "--generated-at",
        "2026-10-30T00:00:00Z",
    ] {
        args.push(flag.into());
    }
    for flag in extra {
        args.push((*flag).into());
    }
    args.push("--out-dir".into());
    args.push(out.to_str().unwrap().into());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&argv)
}

const WARNING: &str = "GROMACS has 2 versions in generation 2026.1: 2025.4 (already in the stack), 2026.2 (added: GROMACS-2026.2-foss-2026.1.eb); pass --allow-versions GROMACS@2026.1 to accept";

#[test]
fn an_installed_first_version_makes_the_root_a_second_version_warning() {
    let out = tempfile::tempdir().expect("tempdir");
    let output = batch(
        out.path(),
        "installed-gromacs-2025.txt",
        &["GROMACS-2026.2-foss-2026.1.eb"],
        &[],
    );
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let dir = only_batch(out.path());
    let body = std::fs::read_to_string(dir.join("pr-body.md")).expect("pr-body.md");
    assert!(
        body.contains("Second versions in one generation, from `eb-stack stack generation`:"),
        "{body}"
    );
    assert!(body.contains(WARNING), "{body}");
    let record = read_json(&dir.join("batch.json"));
    assert_eq!(record["versions"]["count"], 1, "{record}");
    assert_eq!(record["versions"]["warnings"][0], WARNING, "{record}");
    let plan = std::fs::read_to_string(out.path().join("plan.md")).expect("plan.md");
    assert!(
        plan.contains("1 second-version warning(s); `pr-body.md` has the details."),
        "{plan}"
    );
}

#[test]
fn allow_versions_accepts_the_second_version() {
    let out = tempfile::tempdir().expect("tempdir");
    let output = batch(
        out.path(),
        "installed-gromacs-2025.txt",
        &["GROMACS-2026.2-foss-2026.1.eb"],
        &["--allow-versions", "GROMACS@2026.1"],
    );
    assert!(output.status.success(), "stderr:\n{}", text(&output.stderr));
    let dir = only_batch(out.path());
    let body = std::fs::read_to_string(dir.join("pr-body.md")).expect("pr-body.md");
    assert!(
        !body.contains("Second versions in one generation"),
        "the allow list removes the block:\n{body}"
    );
    let record = read_json(&dir.join("batch.json"));
    assert_eq!(record["versions"]["count"], 0, "{record}");
    assert_eq!(record["versions"]["warnings"], json!([]), "{record}");
    let plan = std::fs::read_to_string(out.path().join("plan.md")).expect("plan.md");
    assert!(!plan.contains("second-version warning"), "{plan}");
}

#[test]
fn only_the_later_batch_of_a_split_pair_warns() {
    let out = tempfile::tempdir().expect("tempdir");
    let output = batch(
        out.path(),
        "installed-empty.txt",
        &[
            "GROMACS-2025.4-foss-2026.1.eb",
            "GROMACS-2026.2-foss-2026.1.eb",
        ],
        &["--size", "1"],
    );
    assert!(output.status.success(), "stderr:\n{}", text(&output.stderr));
    let dirs = batch_dirs(out.path());
    assert_eq!(dirs.len(), 2, "two batches: {dirs:?}");
    let warns = |dir: &std::path::Path| {
        read_json(&dir.join("batch.json"))
            .get("versions")
            .and_then(|v| v["count"].as_u64())
            .unwrap_or(0)
    };
    assert_eq!(warns(&dirs[0]), 0, "the first batch has no warning");
    assert_eq!(warns(&dirs[1]), 1, "the later batch carries the warning");
    let later = std::fs::read_to_string(dirs[1].join("pr-body.md")).expect("pr-body.md");
    assert!(later.contains(WARNING), "{later}");
    let first = std::fs::read_to_string(dirs[0].join("pr-body.md")).expect("pr-body.md");
    assert!(
        !first.contains("Second versions in one generation"),
        "{first}"
    );
}

#[test]
fn a_bad_allow_spec_is_a_usage_error_that_writes_nothing() {
    let out = tempfile::tempdir().expect("tempdir");
    let output = batch(
        out.path(),
        "installed-empty.txt",
        &["GROMACS-2026.2-foss-2026.1.eb"],
        &["--allow-versions", "@2026.1"],
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr:\n{}",
        text(&output.stderr)
    );
    assert!(
        text(&output.stderr).contains("--allow-versions @2026.1"),
        "{}",
        text(&output.stderr)
    );
    let entries: Vec<_> = std::fs::read_dir(out.path())
        .expect("out dir")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert!(entries.is_empty(), "the out dir stays empty: {entries:?}");
}

#[test]
fn the_mcp_tool_runs_the_same_second_version_check() {
    let out = tempfile::tempdir().expect("tempdir");
    let cli = batch(
        out.path().join("cli").as_path(),
        "installed-gromacs-2025.txt",
        &["GROMACS-2026.2-foss-2026.1.eb"],
        &[],
    );
    assert!(cli.status.success(), "stderr:\n{}", text(&cli.stderr));
    let cli_record = read_json(&only_batch(&out.path().join("cli")).join("batch.json"));

    let mcp_out = out.path().join("mcp");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_batch",
            "arguments": {
                "root": ["GROMACS-2026.2-foss-2026.1.eb"],
                "easyconfigs": [path("easyconfigs")],
                "installed": path("installed-gromacs-2025.txt"),
                "installed_modulerc": [path("modulerc-empty.lua")],
                "open_prs": [path("open-prs.json")],
                "eessi_version": "2026.06",
                "easybuild_version": "5.4.1",
                "generation": "2026.1",
                "generated_at": "2026-10-30T00:00:00Z",
                "allow_versions": [],
                "out_dir": mcp_out.to_str().unwrap(),
            }
        }
    }))
    .expect("response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let mcp_record = read_json(&only_batch(&mcp_out).join("batch.json"));
    assert_eq!(
        mcp_record["versions"], cli_record["versions"],
        "MCP and CLI agree: {mcp_record} vs {cli_record}"
    );
    assert_eq!(
        mcp_record["versions"]["warnings"][0], WARNING,
        "{mcp_record}"
    );

    let allowed_out = out.path().join("allowed");
    let allowed = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_batch",
            "arguments": {
                "root": ["GROMACS-2026.2-foss-2026.1.eb"],
                "easyconfigs": [path("easyconfigs")],
                "installed": path("installed-gromacs-2025.txt"),
                "installed_modulerc": [path("modulerc-empty.lua")],
                "open_prs": [path("open-prs.json")],
                "eessi_version": "2026.06",
                "easybuild_version": "5.4.1",
                "generation": "2026.1",
                "generated_at": "2026-10-30T00:00:00Z",
                "allow_versions": ["GROMACS@2026.1"],
                "out_dir": allowed_out.to_str().unwrap(),
            }
        }
    }))
    .expect("response");
    assert_eq!(allowed["result"]["isError"], false, "{allowed}");
    let allowed_record = read_json(&only_batch(&allowed_out).join("batch.json"));
    assert_eq!(allowed_record["versions"]["count"], 0, "{allowed_record}");
}
