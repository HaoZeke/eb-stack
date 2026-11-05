use eb_stack::ledger::{self, SignedHead};
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _root: tempfile::TempDir,
    log: PathBuf,
    record: PathBuf,
    key: PathBuf,
    head: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    for directory in ["ledger", "keys", "work"] {
        std::fs::create_dir(root.path().join(directory)).unwrap();
    }
    let key = root.path().join("keys/key.pem");
    let signing = SigningKey::from_bytes(&[19; 32]);
    std::fs::write(
        &key,
        signing.to_pkcs8_pem(LineEnding::LF).unwrap().as_bytes(),
    )
    .unwrap();
    Fixture {
        log: root.path().join("ledger/stack.jsonl"),
        record: root.path().join("work/record.json"),
        head: root.path().join("work/head.json"),
        key,
        _root: root,
    }
}

fn record(event: &str) -> Value {
    json!({
        "ts": "2026-11-05T00:00:00Z", "actor": "retry-test", "stack": "2026.06",
        "target": "x86_64/amd/zen4", "module": "example/1", "location": "/example/1",
        "action": "install", "stage": "finished", "build": {"event_id": event}
    })
}

fn save(fixture: &Fixture, value: &Value) {
    std::fs::write(&fixture.record, serde_json::to_vec(value).unwrap()).unwrap();
}

fn command(fixture: &Fixture, head: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command
        .args(["ledger", "append"])
        .arg(&fixture.record)
        .arg("--log")
        .arg(&fixture.log)
        .arg("--key")
        .arg(&fixture.key)
        .arg("--head-out")
        .arg(head);
    command
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn rows(fixture: &Fixture) -> Vec<ledger::Entry> {
    ledger::read_log(&fixture.log).unwrap()
}

fn check_head(fixture: &Fixture, size: usize) {
    let head = SignedHead::read(&fixture.head).unwrap();
    head.check(&SigningKey::from_bytes(&[19; 32]).verifying_key())
        .unwrap();
    assert_eq!(head.size, size);
    let entries = rows(fixture);
    assert_eq!(
        head.root,
        rfc9162::Head::over(&ledger::leaves(&entries)).root
    );
}

#[test]
fn repeated_event_acknowledges_the_existing_entry() {
    let fixture = fixture();
    save(&fixture, &record("run/module/finished"));
    success(command(&fixture, &fixture.head).output().unwrap());
    let original = std::fs::read(&fixture.log).unwrap();
    let output = success(command(&fixture, &fixture.head).output().unwrap());
    assert_eq!(std::fs::read(&fixture.log).unwrap(), original);
    assert!(output.contains("already recorded"), "{output}");
    check_head(&fixture, 1);
}

#[test]
fn reused_event_identifier_with_different_content_is_refused() {
    let fixture = fixture();
    let mut value = record("run/module/finished");
    save(&fixture, &value);
    success(command(&fixture, &fixture.head).output().unwrap());
    let original = std::fs::read(&fixture.log).unwrap();
    value["reason"] = json!("different operation");
    save(&fixture, &value);
    assert!(!command(&fixture, &fixture.head)
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(std::fs::read(&fixture.log).unwrap(), original);
    check_head(&fixture, 1);
}

#[test]
fn head_output_failure_can_be_retried_without_appending_again() {
    let fixture = fixture();
    save(&fixture, &record("run/module/finished"));
    let missing = fixture.head.parent().unwrap().join("missing/head.json");
    assert!(!command(&fixture, &missing)
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(rows(&fixture).len(), 1);
    success(command(&fixture, &fixture.head).output().unwrap());
    assert_eq!(rows(&fixture).len(), 1);
    check_head(&fixture, 1);
}

#[test]
fn concurrent_retries_append_one_entry() {
    let fixture = fixture();
    save(&fixture, &record("run/module/finished"));
    let first = command(&fixture, &fixture.head).spawn().unwrap();
    let second = command(&fixture, &fixture.head).spawn().unwrap();
    assert!(first.wait_with_output().unwrap().status.success());
    assert!(second.wait_with_output().unwrap().status.success());
    assert_eq!(rows(&fixture).len(), 1);
    check_head(&fixture, 1);
}

#[test]
fn event_identifier_requires_a_nonempty_string() {
    let fixture = fixture();
    let mut value = record("run/module/finished");
    value["build"]["event_id"] = json!(42);
    save(&fixture, &value);
    assert!(!command(&fixture, &fixture.head)
        .output()
        .unwrap()
        .status
        .success());
    assert!(rows(&fixture).is_empty());
}

#[test]
fn acknowledged_retry_checks_the_expected_history_prefix() {
    let fixture = fixture();
    let empty = fixture.head.parent().unwrap().join("empty.json");
    SignedHead::sign(
        &[],
        &SigningKey::from_bytes(&[19; 32]),
        "2026-11-05T00:00:00Z",
    )
    .write(&empty)
    .unwrap();
    save(&fixture, &record("first"));
    success(
        command(&fixture, &fixture.head)
            .arg("--expect-head")
            .arg(&empty)
            .output()
            .unwrap(),
    );
    save(&fixture, &record("second"));
    success(command(&fixture, &fixture.head).output().unwrap());
    save(&fixture, &record("first"));
    success(
        command(&fixture, &fixture.head)
            .arg("--expect-head")
            .arg(&empty)
            .output()
            .unwrap(),
    );
    assert_eq!(rows(&fixture).len(), 2);
    check_head(&fixture, 2);
    let original = std::fs::read(&fixture.log).unwrap();
    let mut changed = SignedHead::read(&empty).unwrap();
    changed.root = "00".repeat(32);
    changed.write(&empty).unwrap();
    assert!(!command(&fixture, &fixture.head)
        .arg("--expect-head")
        .arg(&empty)
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(std::fs::read(&fixture.log).unwrap(), original);
}

#[test]
fn duplicated_event_identifiers_in_existing_history_are_refused() {
    let fixture = fixture();
    let value = record("run/module/finished");
    let first = ledger::seal_record(&value, 0, "2026-11-05T00:00:00Z").unwrap();
    let second = ledger::seal_record(&value, 1, "2026-11-05T00:00:00Z").unwrap();
    let original = format!("{}\n{}\n", first.line, second.line);
    std::fs::write(&fixture.log, &original).unwrap();
    save(&fixture, &value);
    assert!(!command(&fixture, &fixture.head)
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(std::fs::read_to_string(&fixture.log).unwrap(), original);
}
