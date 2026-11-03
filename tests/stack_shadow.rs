//! `stack shadow` against a site layer and an EESSI base module inventory.
//!
//! zlib is the same software version as the base module. FFTW is another
//! version of a base name. OnlyName is a bare base name. VASP is site-only.

use eb_stack::mcp::handle_message;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/shadow_fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

fn shadow(extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "stack".to_string(),
        "shadow".to_string(),
        "--easyconfigs".to_string(),
        fixture("easyconfigs"),
        "--base".to_string(),
        fixture("base"),
        "--base".to_string(),
        fixture("bare.txt"),
    ];
    args.extend(extra.iter().map(|arg| (*arg).to_string()));
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(&args)
        .output()
        .expect("run eb-stack")
}

const REPORT: &str = "\
same zlib-1.3.1-GCCcore-15.2.0.eb name=zlib version=1.3.1 base=1.3.1-GCCcore-15.2.0
different FFTW-3.3.11-gompi-2023a.eb name=FFTW version=3.3.11 base=3.3.10-gompi-2023a
unversioned OnlyName-1.2-GCCcore-15.2.0.eb name=OnlyName version=1.2 base=-
same=1 different=1 unversioned=1 recipes=4 base=3
";

#[test]
fn a_site_layer_that_repeats_the_base_exits_1() {
    let run = shadow(&[]);
    let stdout = String::from_utf8(run.stdout).expect("utf8");
    let stderr = String::from_utf8(run.stderr).expect("utf8");
    assert_eq!(stdout, REPORT, "{stderr}");
    assert!(
        stderr.contains("3 site-layer recipes shadow a module the EESSI base stack provides"),
        "{stderr}"
    );
    assert_eq!(run.status.code(), Some(1));
}

#[test]
fn a_site_only_recipe_exits_0() {
    let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "shadow",
            "--easyconfigs",
            &fixture("easyconfigs/v/VASP/VASP-6.5.1-foss-2026.1.eb"),
            "--base",
            &fixture("base"),
            "--base",
            &fixture("bare.txt"),
        ])
        .output()
        .expect("run");
    let stdout = String::from_utf8(run.stdout).expect("utf8");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        stdout,
        "same=0 different=0 unversioned=0 recipes=1 base=3\n"
    );
}

#[test]
fn json_reports_the_same_hits() {
    let run = shadow(&["--json"]);
    let stdout = String::from_utf8(run.stdout).expect("utf8");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(value["same"], 1);
    assert_eq!(value["different"], 1);
    assert_eq!(value["unversioned"], 1);
    assert_eq!(value["recipes"], 4);
    assert_eq!(value["base_names"], 3);
    assert_eq!(value["hits"][0]["kind"], "same");
    assert_eq!(value["hits"][0]["name"], "zlib");
    assert_eq!(run.status.code(), Some(1));
}

#[test]
fn an_unreadable_base_names_the_path() {
    let missing = fixture("missing-base");
    let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "shadow",
            "--easyconfigs",
            &fixture("easyconfigs"),
            "--base",
            &missing,
        ])
        .output()
        .expect("run");
    let stderr = String::from_utf8(run.stderr).expect("utf8");
    assert!(stderr.contains(&missing), "{stderr}");
    assert_eq!(run.status.code(), Some(1));
}

#[test]
fn a_recipe_that_does_not_parse_names_the_file() {
    let broken = fixture("broken");
    let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "shadow",
            "--easyconfigs",
            &broken,
            "--base",
            &fixture("base"),
        ])
        .output()
        .expect("run");
    let stderr = String::from_utf8(run.stderr).expect("utf8");
    assert!(
        stderr.contains("Broken-1.0.eb") && stderr.contains("did not parse"),
        "{stderr}"
    );
    assert_eq!(run.status.code(), Some(1));
}

#[test]
fn a_missing_flag_is_status_2() {
    let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "shadow", "--easyconfigs", &fixture("easyconfigs")])
        .output()
        .expect("run");
    assert_eq!(run.status.code(), Some(2));
}

#[test]
fn the_mcp_tool_returns_the_same_report() {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_shadow",
            "arguments": {
                "easyconfigs": [fixture("easyconfigs")],
                "base": [fixture("base"), fixture("bare.txt")]
            }
        }
    }))
    .expect("tools/call");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(text.contains(REPORT.trim_end()), "{text}");
    assert!(
        text.contains("3 site-layer recipes shadow a module the EESSI base stack provides"),
        "{text}"
    );
}
