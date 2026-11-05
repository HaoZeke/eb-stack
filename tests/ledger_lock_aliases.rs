#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;

fn append(root: &Path, log: &Path) -> std::process::Output {
    let record = root.join("record.json");
    fs::write(&record, serde_json::to_vec(&serde_json::json!({
        "ts": "2026-11-05T00:00:00Z", "actor": "lock-test", "stack": "2026.06",
        "target": "x86_64/amd/zen4", "module": "example/1", "location": "/example/1",
        "action": "install"
    })).unwrap()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["ledger", "append"]).arg(record).arg("--log").arg(log)
        .output().unwrap()
}

#[test]
fn hardlinked_log_cannot_select_a_different_record_lock() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("ledger.jsonl");
    let alias = root.path().join("alias.jsonl");
    fs::write(&log, "").unwrap();
    fs::hard_link(&log, &alias).unwrap();
    let output = append(root.path(), &alias);
    assert!(!output.status.success(), "hardlinked ledger accepted");
    assert_eq!(fs::read(&log).unwrap(), b"");
    assert_eq!(fs::read(&alias).unwrap(), b"");
}

#[test]
fn symbolic_record_lock_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("ledger.jsonl");
    let outside = root.path().join("outside");
    fs::write(&outside, "protected").unwrap();
    std::os::unix::fs::symlink(&outside, root.path().join("ledger.jsonl.posix-lock")).unwrap();
    let output = append(root.path(), &log);
    assert!(!output.status.success(), "symbolic record lock accepted");
    assert_eq!(fs::read(&log).unwrap(), b"");
    assert_eq!(fs::read(&outside).unwrap(), b"protected");
}

#[test]
fn hardlinked_record_lock_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("ledger.jsonl");
    let outside = root.path().join("outside");
    fs::write(&outside, "protected").unwrap();
    fs::hard_link(&outside, root.path().join("ledger.jsonl.posix-lock")).unwrap();
    let output = append(root.path(), &log);
    assert!(!output.status.success(), "hardlinked record lock accepted");
    assert_eq!(fs::read(&log).unwrap(), b"");
    assert_eq!(fs::read(&outside).unwrap(), b"protected");
}
