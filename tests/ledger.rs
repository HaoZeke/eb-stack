//! `ledger append`, `ledger head`, `ledger show` and `ledger verify` on the
//! binary, and the same writer over MCP.

use eb_stack::ledger;
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    log: PathBuf,
    key: PathBuf,
    public: PathBuf,
    work: PathBuf,
}

/// A log directory, and a key directory that is not inside it.
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let ledger_dir = temp.path().join("ledger");
    let keys = temp.path().join("keys");
    let work = temp.path().join("work");
    for dir in [&ledger_dir, &keys, &work] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let signing = SigningKey::from_bytes(&[11u8; 32]);
    let key = keys.join("ledger.pem");
    std::fs::write(
        &key,
        signing
            .to_pkcs8_pem(LineEnding::LF)
            .expect("pkcs8 pem")
            .as_bytes(),
    )
    .expect("write key");
    let public = work.join("ledger.pub");
    std::fs::write(
        &public,
        signing
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .expect("spki pem"),
    )
    .expect("write public key");
    Fixture {
        log: ledger_dir.join("2026.06.jsonl"),
        key,
        public,
        work,
        _temp: temp,
    }
}

fn record(module: &str, actor: &str) -> Value {
    json!({
        "ts": "2026-11-03T10:00:00Z",
        "actor": actor,
        "stack": "2026.06",
        "target": "x86_64/amd/zen4",
        "module": module,
        "location": format!(
            "/cvmfs/software.eessi.io/host_injections/2026.06/software/linux/x86_64/amd/zen4/software/{module}"
        ),
        "action": "install",
        "reason": {"text": "first install", "refs": ["CVE-2023-45853"]},
        "easyconfig": {"file": "zlib-1.3.1-GCCcore-15.2.0.eb", "sha256": "00".repeat(32)},
    })
}

fn eb(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

fn ok(output: &Output) -> String {
    assert!(
        output.status.success(),
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("utf8 stdout")
}

fn failed(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "expected a failed check\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn s(path: &Path) -> &str {
    path.to_str().expect("utf8 path")
}

/// Append `value` and write the signed head to `head`.
fn append(fixture: &Fixture, value: &Value, head: &Path) -> String {
    let file = fixture.work.join("record.json");
    std::fs::write(&file, serde_json::to_string_pretty(value).unwrap()).expect("record");
    ok(&eb(&[
        "ledger",
        "append",
        s(&file),
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--head-out",
        s(head),
    ]))
}

fn verify_record(fixture: &Fixture, record: usize, head: &Path) -> Output {
    eb(&[
        "ledger",
        "verify",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
        "--record",
        &record.to_string(),
        "--head",
        s(head),
    ])
}

fn verify_range(fixture: &Fixture, from: &Path, to: &Path) -> Output {
    eb(&[
        "ledger",
        "verify",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
        "--from",
        s(from),
        "--to",
        s(to),
    ])
}

/// Three records, with the signed head after each.
fn three(fixture: &Fixture) -> [PathBuf; 3] {
    let heads = [1, 2, 3].map(|n| fixture.work.join(format!("head-{n}.json")));
    for (n, module) in [
        "zlib/1.3.1-GCCcore-15.2.0",
        "PETSc/3.25.0-foss-2026.1",
        "PETSc/3.25.1-foss-2026.1",
    ]
    .iter()
    .enumerate()
    {
        let out = append(fixture, &record(module, "jenkins/run/7"), &heads[n]);
        assert!(
            out.starts_with(&format!("appended record {n} id ")),
            "{out}"
        );
    }
    heads
}

/// Rewrite the log file from records, renumbering them.
fn rewrite(fixture: &Fixture, records: &[Value]) {
    let mut text = String::new();
    for (index, value) in records.iter().enumerate() {
        let mut value = value.clone();
        let object = value.as_object_mut().unwrap();
        object.remove("id");
        object.remove("index");
        let entry = ledger::seal_record(&value, index, "").expect("seal");
        text.push_str(&entry.line);
        text.push('\n');
    }
    std::fs::write(&fixture.log, text).expect("rewrite log");
}

#[test]
fn append_and_verify_round_trip() {
    let fixture = fixture();
    let heads = three(&fixture);

    let text = std::fs::read_to_string(&fixture.log).expect("log");
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3);
    for (index, line) in lines.iter().enumerate() {
        let value: Value = serde_json::from_str(line).expect("json line");
        assert_eq!(value["index"], index);
        assert_eq!(ledger::canonical(&value).unwrap(), *line);
    }

    for (n, head) in heads.iter().enumerate() {
        for record in 0..=n {
            let out = ok(&verify_record(&fixture, record, head));
            assert!(out.starts_with(&format!("record {record} id ")), "{out}");
        }
        failed(&verify_record(&fixture, n + 1, head));
    }
    for (from, to) in [(0, 1), (0, 2), (1, 2), (2, 2)] {
        let out = ok(&verify_range(&fixture, &heads[from], &heads[to]));
        assert!(
            out.contains(&format!("extends the head of size {}", from + 1)),
            "{out}"
        );
    }
    failed(&verify_range(&fixture, &heads[2], &heads[0]));

    let head: Value =
        serde_json::from_str(&std::fs::read_to_string(&heads[2]).unwrap()).expect("head json");
    assert_eq!(head["domain"], "eb-stack-ledger-v1");
    assert_eq!(head["size"], 3);
    let hex = fixture.work.join("ledger.hex");
    std::fs::write(&hex, format!("{}\n", head["public_key"].as_str().unwrap())).unwrap();
    ok(&eb(&[
        "ledger",
        "verify",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&hex),
        "--record",
        "2",
        "--head",
        s(&heads[2]),
    ]));

    let show = ok(&eb(&[
        "ledger",
        "show",
        "--log",
        s(&fixture.log),
        "--package",
        "PETSc",
    ]));
    let rows = show.lines().collect::<Vec<_>>();
    assert_eq!(rows[0], "index\tts\taction\ttarget\tmodule\tactor");
    assert!(rows[1].starts_with("2\t"), "newest first: {show}");
    assert!(rows[2].starts_with("1\t"), "{show}");
    assert_eq!(rows[3], "2 of 3 records");
    let exact = ok(&eb(&[
        "ledger",
        "show",
        "--log",
        s(&fixture.log),
        "--package",
        "PETSc/3.25.0-foss-2026.1",
        "--json",
    ]));
    let records: Value = serde_json::from_str(&exact).expect("show json");
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["index"], 1);
}

#[test]
fn a_head_signed_by_another_key_fails() {
    let fixture = fixture();
    let heads = three(&fixture);
    let other = SigningKey::from_bytes(&[12u8; 32]);
    std::fs::write(
        &fixture.public,
        other
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    )
    .unwrap();
    let error = failed(&verify_record(&fixture, 0, &heads[2]));
    assert!(error.contains("not by the trusted key"), "{error}");
}

#[test]
fn a_removed_record_fails_inclusion() {
    let fixture = fixture();
    let heads = three(&fixture);
    let text = std::fs::read_to_string(&fixture.log).unwrap();
    let lines = text.lines().map(str::to_string).collect::<Vec<_>>();

    // The line deleted as it stands: the log no longer reads.
    std::fs::write(&fixture.log, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
    failed(&verify_record(&fixture, 1, &heads[2]));

    // The line deleted and the rest renumbered, then a record appended so the
    // log has as many records as the head: every line reads and the proof
    // still fails.
    let mut kept = vec![
        serde_json::from_str::<Value>(&lines[0]).unwrap(),
        serde_json::from_str::<Value>(&lines[2]).unwrap(),
    ];
    kept.push(record("HDF5/1.14.6-gompi-2026.1", "jenkins/run/8"));
    rewrite(&fixture, &kept);
    ok(&eb(&["ledger", "show", "--log", s(&fixture.log)]));
    for record in 0..3 {
        let error = failed(&verify_record(&fixture, record, &heads[2]));
        assert!(error.contains("is not in the head of size 3"), "{error}");
    }
    // Record 0 is unchanged and still proves against the head that covers
    // only it.
    ok(&verify_record(&fixture, 0, &heads[0]));
}

#[test]
fn a_rewritten_log_fails_consistency() {
    let fixture = fixture();
    let heads = three(&fixture);
    let text = std::fs::read_to_string(&fixture.log).unwrap();
    let mut records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    // History rewritten: record 0 gets another actor, then one more record
    // goes on the end and the same key signs the new head.
    records[0]["actor"] = json!("someone-else");
    records.push(record("HDF5/1.14.6-gompi-2026.1", "jenkins/run/8"));
    rewrite(&fixture, &records);
    let later = fixture.work.join("head-4.json");
    let signed = ok(&eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&later),
        "--ts",
        "2026-11-04T00:00:00Z",
    ]));
    assert!(signed.starts_with("signed head size 4 root "), "{signed}");
    for earlier in &heads {
        let error = failed(&verify_range(&fixture, earlier, &later));
        assert!(error.contains("does not extend"), "{error}");
    }
    // The rewritten log is consistent with itself.
    let again = fixture.work.join("head-4b.json");
    ok(&eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&again),
    ]));
    ok(&verify_range(&fixture, &later, &again));
}

#[test]
fn canonical_form_is_stable_across_key_order() {
    let forward = record("zlib/1.3.1-GCCcore-15.2.0", "jenkins/run/7");
    let mut reversed = serde_json::Map::new();
    let object = forward.as_object().unwrap();
    let mut keys = object.keys().cloned().collect::<Vec<_>>();
    keys.reverse();
    for key in keys {
        let value = match &object[&key] {
            Value::Object(inner) => {
                let mut flipped = serde_json::Map::new();
                let mut names = inner.keys().cloned().collect::<Vec<_>>();
                names.reverse();
                for name in names {
                    flipped.insert(name.clone(), inner[&name].clone());
                }
                Value::Object(flipped)
            }
            other => other.clone(),
        };
        reversed.insert(key, value);
    }
    let reversed = Value::Object(reversed);

    let a = fixture();
    let b = fixture();
    let one = a.work.join("forward.json");
    let two = b.work.join("reversed.json");
    std::fs::write(&one, serde_json::to_string_pretty(&forward).unwrap()).unwrap();
    // Hand-written text with the keys in reverse order and extra spaces.
    let mut text = String::from("{ ");
    for (at, (key, value)) in reversed.as_object().unwrap().iter().enumerate() {
        if at > 0 {
            text.push_str(" ,\n  ");
        }
        text.push_str(&format!("{} :  {}", json!(key), value));
    }
    text.push_str(" }\n");
    std::fs::write(&two, &text).unwrap();
    for (fixture, file) in [(&a, &one), (&b, &two)] {
        ok(&eb(&[
            "ledger",
            "append",
            s(file),
            "--log",
            s(&fixture.log),
        ]));
    }
    let left = std::fs::read(&a.log).unwrap();
    let right = std::fs::read(&b.log).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&left),
        String::from_utf8_lossy(&right)
    );
    let line = String::from_utf8(left).unwrap();
    assert!(
        line.starts_with("{\"action\":\"install\",\"actor\":"),
        "{line}"
    );
    assert!(line.contains(",\"easyconfig\":{\"file\":"), "{line}");
    assert!(
        line.ends_with(",\"ts\":\"2026-11-03T10:00:00Z\"}\n"),
        "{line}"
    );
    assert_eq!(
        ledger::record_id(forward.as_object().unwrap()).unwrap(),
        ledger::record_id(reversed.as_object().unwrap()).unwrap()
    );
}

#[test]
fn a_key_inside_the_ledger_directory_is_refused() {
    let fixture = fixture();
    let beside = fixture.log.parent().unwrap().join("ledger.pem");
    std::fs::copy(&fixture.key, &beside).unwrap();
    let file = fixture.work.join("record.json");
    std::fs::write(&file, record("zlib/1.3.1", "jenkins/run/7").to_string()).unwrap();
    let error = failed(&eb(&[
        "ledger",
        "append",
        s(&file),
        "--log",
        s(&fixture.log),
        "--key",
        s(&beside),
        "--head-out",
        s(&fixture.work.join("head.json")),
    ]));
    assert!(error.contains("inside the ledger directory"), "{error}");
    assert!(!fixture.log.exists(), "nothing is appended");
}

#[test]
fn append_refuses_a_stale_head_and_another_stack() {
    let fixture = fixture();
    let heads = three(&fixture);
    let file = fixture.work.join("record.json");
    std::fs::write(&file, record("HDF5/1.14.6", "jenkins/run/8").to_string()).unwrap();
    let error = failed(&eb(&[
        "ledger",
        "append",
        s(&file),
        "--log",
        s(&fixture.log),
        "--expect-head",
        s(&heads[1]),
    ]));
    assert!(error.contains("not the expected head size 2"), "{error}");
    ok(&eb(&[
        "ledger",
        "append",
        s(&file),
        "--log",
        s(&fixture.log),
        "--expect-head",
        s(&heads[2]),
    ]));

    let mut other = record("HDF5/1.14.6", "jenkins/run/9");
    other["stack"] = json!("2025");
    std::fs::write(&file, other.to_string()).unwrap();
    let error = failed(&eb(&[
        "ledger",
        "append",
        s(&file),
        "--log",
        s(&fixture.log),
    ]));
    assert!(error.contains("stack"), "{error}");
}

#[test]
fn verify_needs_one_proof_on_the_command_line() {
    let fixture = fixture();
    let bare = eb(&[
        "ledger",
        "verify",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
    ]);
    assert_eq!(bare.status.code(), Some(2));
    let half = eb(&[
        "ledger",
        "verify",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
        "--record",
        "0",
    ]);
    assert_eq!(half.status.code(), Some(2));
}

fn mcp(name: &str, arguments: Value) -> Value {
    let response = eb_stack::mcp::handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    }))
    .expect("tools/call response");
    response["result"].clone()
}

#[test]
fn mcp_ledger_tools_append_show_and_verify() {
    let fixture = fixture();
    let file = fixture.work.join("record.json");
    let mut heads = Vec::new();
    for (n, module) in ["zlib/1.3.1", "PETSc/3.25.0-foss-2026.1"]
        .iter()
        .enumerate()
    {
        std::fs::write(&file, record(module, "jenkins/run/7").to_string()).unwrap();
        let head = fixture.work.join(format!("mcp-head-{n}.json"));
        let result = mcp(
            "eb_ledger_append",
            json!({
                "log": s(&fixture.log),
                "record": s(&file),
                "key": s(&fixture.key),
                "head_out": s(&head),
            }),
        );
        assert_ne!(result["isError"], true, "{result}");
        heads.push(head);
    }
    let show = mcp(
        "eb_ledger_show",
        json!({"log": s(&fixture.log), "package": "PETSc"}),
    );
    assert_ne!(show["isError"], true, "{show}");
    assert!(show.to_string().contains("1 of 2 records"), "{show}");
    let inclusion = mcp(
        "eb_ledger_verify",
        json!({
            "log": s(&fixture.log),
            "public_key": s(&fixture.public),
            "record": 0,
            "head": s(&heads[1]),
        }),
    );
    assert_ne!(inclusion["isError"], true, "{inclusion}");
    let consistency = mcp(
        "eb_ledger_verify",
        json!({
            "log": s(&fixture.log),
            "public_key": s(&fixture.public),
            "from": s(&heads[0]),
            "to": s(&heads[1]),
        }),
    );
    assert_ne!(consistency["isError"], true, "{consistency}");
    let past = mcp(
        "eb_ledger_verify",
        json!({
            "log": s(&fixture.log),
            "public_key": s(&fixture.public),
            "record": 1,
            "head": s(&heads[0]),
        }),
    );
    assert_eq!(past["isError"], true, "{past}");
}

#[test]
fn an_empty_log_signs_a_head_without_a_checkpoint() {
    let fixture = fixture();
    std::fs::write(&fixture.log, "").expect("empty log");
    let head = fixture.work.join("head-0.json");
    let signed = ok(&eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&head),
        "--ts",
        "2026-11-03T12:00:00Z",
    ]));
    let empty_root = rfc9162::hex(&rfc9162::root(&[]));
    assert_eq!(
        signed,
        format!(
            "signed head size 0 root {empty_root} at 2026-11-03T12:00:00Z to {}\n",
            head.display()
        ),
        "a size-0 head has no checkpoint line"
    );
    let tiles = PathBuf::from(format!("{}.tiles", fixture.log.display()));
    assert!(!tiles.join("checkpoint").exists(), "no checkpoint is written");
    let extends = ok(&verify_range(&fixture, &head, &head));
    assert_eq!(
        extends,
        format!("the head of size 0 root {empty_root} extends the head of size 0 root {empty_root}\n"),
        "a size-0 head verifies without a tile-log line"
    );
    let later = fixture.work.join("head-1.json");
    append(&fixture, &record("zlib/1.3.1-GCCcore-15.2.0", "rg"), &later);
    let grown = ok(&verify_range(&fixture, &head, &later));
    assert!(
        grown.starts_with("the head of size 1 root "),
        "the first record's head extends the empty one: {grown}"
    );
    assert!(
        grown.contains("tile log size 1 root "),
        "the tile log is audited once a record derives the origin: {grown}"
    );
}
