//! `stack sbom --backport-list` from a lock, recorded OSV responses, and the
//! SBOM reverse closure.

use eb_stack::domain::StackLock;
use eb_stack::mcp::handle_message;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/backport_fixtures")
}

fn lock_path() -> PathBuf {
    root().join("stack.lock.json")
}

fn load_lock() -> StackLock {
    serde_json::from_str(&std::fs::read_to_string(lock_path()).unwrap()).unwrap()
}

#[test]
fn the_lock_lists_cross_user_and_network_facing_and_records_the_rest() {
    let lock = load_lock();
    let cache = root().join("osv");
    let reach = root().join("reach.txt");
    let document = eb_stack::backport::backport_list(&eb_stack::backport::BackportRequest {
        lock: &lock,
        lock_path: &lock_path(),
        cves: &[],
        osv_cache: Some(&cache),
        reachability: Some(&reach),
    })
    .expect("backport list");
    assert_eq!(document.schema_version, 1);
    assert_eq!(document.generation, "GCCcore-15.2.0");
    assert_eq!(document.stack_modules, 7);
    assert_eq!(document.listed, 2);
    assert_eq!(document.recorded, 3);
    let listed: Vec<(&str, &str, &str, usize)> = document
        .rows
        .iter()
        .filter(|row| row.on_list)
        .map(|row| {
            (
                row.module.as_str(),
                row.reach.as_str(),
                row.advisory.as_str(),
                row.blast_radius,
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (
                "notebook/7.4.0-GCCcore-15.2.0",
                "cross-user",
                "GHSA-test-notebook",
                1
            ),
            (
                "cURL/8.17.0-GCCcore-15.2.0",
                "network-facing",
                "GHSA-test-curl",
                2
            ),
        ]
    );
    let curl = document
        .rows
        .iter()
        .find(|row| row.name == "cURL")
        .expect("curl");
    assert_eq!(curl.fixed_in, ["8.18.0"]);
    assert_eq!(
        curl.blast,
        [
            "cURL/8.17.0-GCCcore-15.2.0".to_string(),
            "git/2.52.0-GCCcore-15.2.0".to_string()
        ]
    );
    let recorded: Vec<(&str, &str, usize)> = document
        .rows
        .iter()
        .filter(|row| !row.on_list)
        .map(|row| (row.module.as_str(), row.reach.as_str(), row.blast_radius))
        .collect();
    assert_eq!(
        recorded,
        [
            ("zlib/1.3.1-GCCcore-15.2.0", "local-only", 2),
            ("WinOnly/1.0-GCCcore-15.2.0", "not-reachable", 1),
            ("Mystery/1.0-GCCcore-15.2.0", "unclassified", 1),
        ]
    );
    let zlib = document
        .rows
        .iter()
        .find(|row| row.name == "zlib")
        .expect("zlib");
    assert_eq!(
        zlib.blast,
        [
            "App/1.0-GCCcore-15.2.0".to_string(),
            "zlib/1.3.1-GCCcore-15.2.0".to_string()
        ]
    );
    assert_eq!(
        document.notes,
        [
            "GHSA-test-missing names jupyter_server, which this lock does not have".to_string(),
            "GHSA-test-unparsed names notebook, but its ranges were not parsed".to_string(),
            "cURL/8.17.0-GCCcore-15.2.0 is not in range for GHSA-test-curl-old (>=0,<8.0.0)"
                .to_string(),
        ]
    );
    let markdown = eb_stack::backport::render_markdown(&document);
    assert!(markdown.contains("## Backport"));
    assert!(markdown.contains(
        "These modules stay off the backport list. A backport is cross-user or network-facing."
    ));
    assert!(markdown.find("notebook/7.4.0").unwrap() < markdown.find("cURL/8.17.0").unwrap());
}

#[test]
fn the_cli_writes_the_list_and_rejects_a_partial_flag_set() {
    let tmp = tempfile::tempdir().unwrap();
    let md = tmp.path().join("backport.md");
    let sbom = tmp.path().join("stack.cdx.json");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "sbom", "--lock"])
        .arg(lock_path())
        .arg("--out")
        .arg(&sbom)
        .arg("--backport-list")
        .arg(&md)
        .arg("--osv-cache")
        .arg(root().join("osv"))
        .arg("--reachability")
        .arg(root().join("reach.txt"))
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}\n{stdout}");
    assert!(
        stdout.contains("components=7 recipes_read=7 recipes_unreadable=0"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "backport=2 recorded=3 stack=7 out={}",
            md.display()
        )),
        "{stdout}"
    );
    assert!(sbom.is_file());
    let json: Value =
        serde_json::from_str(&std::fs::read_to_string(md.with_extension("json")).unwrap()).unwrap();
    assert_eq!(json["listed"], 2);
    assert_eq!(json["recorded"], 3);
    assert!(std::fs::read_to_string(&md)
        .unwrap()
        .contains("network-facing"));

    let missing = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "sbom",
            "--lock",
            "/no/such.lock",
            "--cve",
            "GHSA-test-curl",
        ])
        .output()
        .expect("run");
    assert_eq!(missing.status.code(), Some(2));
    let err = String::from_utf8_lossy(&missing.stderr);
    assert!(err.contains("need --backport-list"), "{err}");

    let bare = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "sbom",
            "--lock",
            "/no/such.lock",
            "--backport-list",
            "/tmp/backport.md",
        ])
        .output()
        .expect("run");
    assert_eq!(bare.status.code(), Some(2));
    let err = String::from_utf8_lossy(&bare.stderr);
    assert!(err.contains("needs --cve or --osv-cache"), "{err}");
}

#[test]
fn one_cve_does_not_pull_the_rest_of_the_cache() {
    let lock = load_lock();
    let cache = root().join("osv");
    let document = eb_stack::backport::backport_list(&eb_stack::backport::BackportRequest {
        lock: &lock,
        lock_path: &lock_path(),
        cves: &["GHSA-test-curl".into()],
        osv_cache: Some(&cache),
        reachability: None,
    })
    .expect("one advisory");
    assert_eq!(document.advisories, ["GHSA-test-curl"]);
    assert_eq!(document.listed, 1);
    assert_eq!(document.recorded, 0);
    assert!(document.notes.is_empty(), "{:?}", document.notes);
    assert_eq!(document.rows[0].reach, "network-facing");
}

#[test]
fn mcp_stack_sbom_can_write_the_backport_list() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("stack.cdx.json");
    let md = tmp.path().join("backport.md");
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_sbom",
            "arguments": {
                "lock": lock_path(),
                "out": &out,
                "backport_list": &md,
                "osv_cache": root().join("osv"),
                "reachability": root().join("reach.txt")
            }
        }
    }))
    .expect("mcp");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["listed"], 2);
    assert_eq!(body["recorded"], 3);
    assert_eq!(body["stack"], 7);
    assert!(md.is_file());
    let _ = Path::new(&out);
}
