//! Namespace-alias safety for the tile log beside a ledger.
//!
//! Tile output names must stay separate from the canonical log, signing
//! key and JSON head. Namespace aliases must fail before any append.

use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    ledger_dir: PathBuf,
    tiles: PathBuf,
    keys: PathBuf,
    work: PathBuf,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let ledger_dir = temp.path().join("ledger");
    let tiles = temp.path().join("tiles");
    let keys = temp.path().join("keys");
    let work = temp.path().join("work");
    for dir in [&ledger_dir, &tiles, &keys, &work] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    Fixture {
        _temp: temp,
        ledger_dir,
        tiles,
        keys,
        work,
    }
}

fn write_key(path: &Path, seed: u8) {
    let signing = SigningKey::from_bytes(&[seed; 32]);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir key parent");
    }
    std::fs::write(
        path,
        signing
            .to_pkcs8_pem(LineEnding::LF)
            .expect("pkcs8 pem")
            .as_bytes(),
    )
    .expect("write key");
    let public = path.with_extension("pub");
    std::fs::write(
        &public,
        signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("spki pem"),
    )
    .expect("write public key");
}

fn record(event_id: &str) -> Value {
    json!({
        "ts": "2026-11-07T20:30:00Z",
        "actor": "namespace-test",
        "stack": "tile-alias-probe",
        "target": "x86_64/amd/zen4",
        "module": "Probe/1.0-foss-2026a",
        "location": "/example/modules/Probe/1.0-foss-2026a",
        "action": "install",
        "stage": "finished",
        "route": "native",
        "build": {"event_id": event_id, "job": "namespace-probe", "log": "sha256:00"},
    })
}

fn eb(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

fn s(path: &Path) -> &str {
    path.to_str().expect("utf8 path")
}

fn write_record(work: &Path, value: &Value) -> PathBuf {
    let file = work.join("record.json");
    std::fs::write(&file, serde_json::to_string_pretty(value).unwrap()).expect("record");
    file
}

/// The append must not leave a canonical log behind when it refuses a
/// namespace alias, preserving the canonical input bytes.
fn assert_refused_before_append(output: &Output, log: &Path) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected status 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log_bytes = if log.exists() {
        std::fs::metadata(log).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };
    assert_eq!(
        log_bytes,
        0,
        "canonical append must not run before a tile-namespace refusal; log has {log_bytes} bytes\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn key_inside_log_directory_still_refused_before_append() {
    let fixture = fixture();
    let log = fixture.ledger_dir.join("2026.jsonl");
    // The signing key must stay outside the canonical log directory.
    let tiles = fixture.ledger_dir.join("tiles");
    std::fs::create_dir_all(&tiles).expect("tiles");
    let key = tiles.join("checkpoint");
    write_key(&key, 21);
    let head = fixture.work.join("head.json");
    let record = write_record(&fixture.work, &record("ns-beside-log-1"));
    let output = eb(&[
        "ledger",
        "append",
        s(&record),
        "--log",
        s(&log),
        "--key",
        s(&key),
        "--head-out",
        s(&head),
        "--tiles",
        s(&tiles),
        "--origin",
        "eb-stack-ledger/tile-alias-probe/2026",
    ]);
    assert_refused_before_append(&output, &log);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("inside the ledger directory"),
        "stderr should name refuse_key_beside_log: {stderr}"
    );
}

#[test]
fn key_at_tiles_checkpoint_outside_log_dir_refused_before_append() {
    let fixture = fixture();
    let log = fixture.ledger_dir.join("2026.jsonl");
    let key = fixture.tiles.join("checkpoint");
    write_key(&key, 22);
    let key_before = std::fs::read(&key).expect("key bytes");
    let head = fixture.work.join("head.json");
    let record = write_record(&fixture.work, &record("ns-key-checkpoint-1"));
    let output = eb(&[
        "ledger",
        "append",
        s(&record),
        "--log",
        s(&log),
        "--key",
        s(&key),
        "--head-out",
        s(&head),
        "--tiles",
        s(&fixture.tiles),
        "--origin",
        "eb-stack-ledger/tile-alias-probe/2026",
    ]);
    assert_refused_before_append(&output, &log);
    let key_after = std::fs::read(&key).expect("key after");
    assert_eq!(
        key_before, key_after,
        "signing key bytes at tiles/checkpoint must not change"
    );
}

#[test]
fn head_at_tiles_checkpoint_refused_before_append() {
    let fixture = fixture();
    let log = fixture.ledger_dir.join("2026.jsonl");
    let key = fixture.keys.join("ledger.pem");
    write_key(&key, 23);
    let head = fixture.tiles.join("checkpoint");
    let record = write_record(&fixture.work, &record("ns-head-checkpoint-1"));
    let output = eb(&[
        "ledger",
        "append",
        s(&record),
        "--log",
        s(&log),
        "--key",
        s(&key),
        "--head-out",
        s(&head),
        "--tiles",
        s(&fixture.tiles),
        "--origin",
        "eb-stack-ledger/tile-alias-probe/2026",
    ]);
    assert_refused_before_append(&output, &log);
    assert!(
        !head.exists() || std::fs::metadata(&head).map(|m| m.len()).unwrap_or(0) == 0,
        "head must not be written onto the reserved checkpoint path before refusal"
    );
}

#[test]
fn log_named_manifest_with_tiles_eq_parent_refused_before_append() {
    let fixture = fixture();
    let ns = fixture.work.join("ns");
    std::fs::create_dir_all(&ns).expect("ns");
    let log = ns.join("manifest");
    let key = fixture.keys.join("ledger.pem");
    write_key(&key, 24);
    let head = fixture.work.join("head.json");
    let record = write_record(&fixture.work, &record("ns-log-manifest-1"));
    let output = eb(&[
        "ledger",
        "append",
        s(&record),
        "--log",
        s(&log),
        "--key",
        s(&key),
        "--head-out",
        s(&head),
        "--tiles",
        s(&ns),
        "--origin",
        "eb-stack-ledger/tile-alias-probe/2026",
    ]);
    assert_refused_before_append(&output, &log);
}

#[test]
fn sign_head_key_at_tiles_checkpoint_refused_without_replacing_key() {
    let fixture = fixture();
    let log = fixture.ledger_dir.join("2026.jsonl");
    let key = fixture.keys.join("ledger.pem");
    write_key(&key, 25);
    let head = fixture.work.join("head.json");
    let record = write_record(&fixture.work, &record("ns-signhead-setup-1"));
    let setup = eb(&[
        "ledger",
        "append",
        s(&record),
        "--log",
        s(&log),
        "--key",
        s(&key),
        "--head-out",
        s(&head),
        "--tiles",
        s(&fixture.tiles),
        "--origin",
        "eb-stack-ledger/tile-alias-probe/2026",
    ]);
    assert!(
        setup.status.success(),
        "setup append must succeed\nstderr:\n{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let checkpoint = fixture.tiles.join("checkpoint");
    assert!(checkpoint.exists(), "setup must publish a checkpoint");
    let key_bytes = std::fs::read(&key).expect("key");
    // Move the only key link onto the reserved checkpoint path (nlink stays 1).
    std::fs::remove_file(&checkpoint).expect("remove note");
    std::fs::rename(&key, &checkpoint).expect("move key onto checkpoint");
    let before = std::fs::read(&checkpoint).expect("pem at checkpoint");
    assert_eq!(before, key_bytes);
    let out = fixture.work.join("head2.json");
    let output = eb(&[
        "ledger",
        "head",
        "--log",
        s(&log),
        "--key",
        s(&checkpoint),
        "--out",
        s(&out),
        "--tiles",
        s(&fixture.tiles),
        "--origin",
        "eb-stack-ledger/tile-alias-probe/2026",
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "sign_head must refuse key-at-checkpoint\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after = std::fs::read(&checkpoint).expect("after");
    assert_eq!(
        before, after,
        "key material at checkpoint must not be replaced"
    );
}
