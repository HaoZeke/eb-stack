#![cfg(unix)]

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn append_waits_for_the_posix_writer() {
    let root = tempfile::tempdir().unwrap();
    let log = root.path().join("ledger.jsonl");
    let sidecar = root.path().join("ledger.jsonl.posix-lock");
    let record = root.path().join("record.json");
    std::fs::write(&record, serde_json::to_vec(&serde_json::json!({
        "ts": "2026-11-05T00:00:00Z", "actor": "lock-test", "stack": "2026.06", "target": "x86_64/amd/zen4",
        "module": "example/1", "location": "/example/1", "action": "install"
    })).unwrap()).unwrap();
    let holder = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&sidecar)
        .unwrap();
    // The file descriptor is live and the initialized flock covers the file.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    assert_eq!(
        unsafe { libc::fcntl(holder.as_raw_fd(), libc::F_SETLK, &lock) },
        0
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["ledger", "append"])
        .arg(&record)
        .arg("--log")
        .arg(&log)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "append bypassed the POSIX writer lock"
    );
    assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
    drop(holder);
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let lines = std::fs::read_to_string(log).unwrap();
    let entries: Vec<_> = lines.lines().collect();
    assert_eq!(entries.len(), 1);
    let entry: serde_json::Value = serde_json::from_str(entries[0]).unwrap();
    assert_eq!(entry["index"], 0);
    assert_eq!(entry["module"], "example/1");
}
