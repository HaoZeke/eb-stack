//! The tile log beside a ledger: `ledger append` writes it, `ledger tiles`
//! syncs it, `ledger head` signs its checkpoint, `ledger find` answers keys
//! from it, and `ledger verify` refuses a tile log that disagrees with the
//! ledger.

use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    log: PathBuf,
    tiles: PathBuf,
    key: PathBuf,
    public: PathBuf,
    work: PathBuf,
}

/// A log directory, and a key directory that is not inside it.
fn fixture_with_key(seed: u8) -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let ledger_dir = temp.path().join("ledger");
    let keys = temp.path().join("keys");
    let work = temp.path().join("work");
    for dir in [&ledger_dir, &keys, &work] {
        std::fs::create_dir_all(dir).expect("mkdir");
    }
    let signing = SigningKey::from_bytes(&[seed; 32]);
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
        tiles: ledger_dir.join("2026.06.jsonl.tiles"),
        key,
        public,
        work,
        _temp: temp,
    }
}

fn fixture() -> Fixture {
    fixture_with_key(11)
}

fn record(module: &str, route: &str, job: &str) -> Value {
    json!({
        "ts": "2026-11-03T10:00:00Z",
        "actor": "jenkins/run/7",
        "stack": "2026.06",
        "target": "x86_64/amd/zen4",
        "module": module,
        "location": format!(
            "/cvmfs/software.eessi.io/host_injections/2026.06/software/linux/x86_64/amd/zen4/software/{module}"
        ),
        "action": "install",
        "route": route,
        "build": {"job": job, "log": "sha256:00"},
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

/// Append `value` with the signing key and write the signed head to `head`.
fn append_signed(fixture: &Fixture, value: &Value, head: &Path) -> Output {
    let file = fixture.work.join("record.json");
    std::fs::write(&file, serde_json::to_string_pretty(value).unwrap()).expect("record");
    eb(&[
        "ledger",
        "append",
        s(&file),
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--head-out",
        s(head),
    ])
}

/// Append `value` without a key: the tile log grows, the checkpoint does not.
fn append_plain(fixture: &Fixture, value: &Value) -> Output {
    let file = fixture.work.join("record.json");
    std::fs::write(&file, serde_json::to_string_pretty(value).unwrap()).expect("record");
    eb(&["ledger", "append", s(&file), "--log", s(&fixture.log)])
}

/// Three records, with the signed head after each: PETSc on kvm, PETSc on
/// native, zlib on native.
fn three(fixture: &Fixture) -> [PathBuf; 3] {
    let heads = [1, 2, 3].map(|n| fixture.work.join(format!("head-{n}.json")));
    for (n, (module, route, job)) in [
        ("PETSc/3.25.0-foss-2026.1", "kvm", "snellius/4411"),
        ("PETSc/3.25.1-foss-2026.1", "native", "snellius/4412"),
        ("zlib/1.3.1-GCCcore-15.2.0", "native", "snellius/4413"),
    ]
    .iter()
    .enumerate()
    {
        let out = ok(&append_signed(
            fixture,
            &record(module, route, job),
            &heads[n],
        ));
        assert!(
            out.starts_with(&format!("appended record {n} id ")),
            "{out}"
        );
        assert!(
            out.contains(&format!("tile log size {} root ", n + 1)),
            "{out}"
        );
        assert!(
            out.contains("checkpoint eb-stack-ledger/2026.06/2026 size "),
            "{out}"
        );
    }
    heads
}

fn find(fixture: &Fixture, extra: &[&str]) -> Output {
    let mut args = vec![
        "ledger",
        "find",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
    ];
    args.extend_from_slice(extra);
    eb(&args)
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

/// Rewrite the log file from records, renumbering them.
fn rewrite(fixture: &Fixture, records: &[Value]) {
    let mut text = String::new();
    for (index, value) in records.iter().enumerate() {
        let mut value = value.clone();
        let object = value.as_object_mut().unwrap();
        object.remove("id");
        object.remove("index");
        let entry = eb_stack::ledger::seal_record(&value, index, "").expect("seal");
        text.push_str(&entry.line);
        text.push('\n');
    }
    std::fs::write(&fixture.log, text).expect("rewrite log");
}

/// The records of the log, in order.
fn records(fixture: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(&fixture.log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect()
}

/// Flip one bit of the file at `path`.
fn flip_a_bit(path: &Path, at: usize) {
    let mut bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        at < bytes.len(),
        "{} has {} bytes",
        path.display(),
        bytes.len()
    );
    bytes[at] ^= 1;
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn append_writes_the_tile_log_and_find_answers_keys() {
    let fixture = fixture();
    let heads = three(&fixture);
    assert!(fixture.tiles.join(".state/treeState").is_file());
    assert!(fixture.tiles.join("checkpoint").is_file());
    assert!(fixture.tiles.join("tile/entries/000.p/3").is_file());
    let checkpoint = std::fs::read_to_string(fixture.tiles.join("checkpoint")).unwrap();
    assert!(
        checkpoint.starts_with("eb-stack-ledger/2026.06/2026\n3\n"),
        "{checkpoint}"
    );
    assert!(
        checkpoint.contains("\u{2014} eb-stack-ledger/2026.06/2026 "),
        "{checkpoint}"
    );
    let ids = records(&fixture)
        .iter()
        .map(|record| record["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();

    let petsc = ok(&find(&fixture, &["--key", "pkg=PETSc"]));
    let rows = petsc.lines().collect::<Vec<_>>();
    assert_eq!(rows.len(), 3, "{petsc}");
    assert!(rows[0].starts_with("0\t{\"action\":\"install\""), "{petsc}");
    assert!(rows[1].starts_with("1\t{\"action\":\"install\""), "{petsc}");
    assert_eq!(
        rows[2],
        format!(
            "2 of 3 records match; checkpoint eb-stack-ledger/2026.06/2026 size 3 root {}",
            tree_root(&fixture)
        )
    );
    for (index, line) in std::fs::read_to_string(&fixture.log)
        .unwrap()
        .lines()
        .enumerate()
        .take(2)
    {
        assert_eq!(
            rows[index],
            format!("{index}\t{line}"),
            "the line as written"
        );
    }

    let kvm = ok(&find(
        &fixture,
        &["--key", "pkg=PETSc", "--key", "route=kvm"],
    ));
    assert!(kvm.starts_with("0\t"), "{kvm}");
    assert!(kvm.contains("\n1 of 3 records match;"), "{kvm}");

    let native = ok(&find(&fixture, &["--key", "route=native"]));
    assert!(native.contains("\n2 of 3 records match;"), "{native}");
    assert!(native.starts_with("1\t"), "{native}");

    let module = ok(&find(
        &fixture,
        &["--key", "module=zlib/1.3.1-GCCcore-15.2.0"],
    ));
    assert!(module.starts_with("2\t"), "{module}");
    let job = ok(&find(&fixture, &["--key", "job=snellius/4412"]));
    assert!(job.starts_with("1\t"), "{job}");
    let by_id = ok(&find(&fixture, &["--key", &format!("id={}", ids[2])]));
    assert!(by_id.starts_with("2\t"), "{by_id}");
    let actor = ok(&find(
        &fixture,
        &["--key", "actor=jenkins/run/7", "--key", "action=install"],
    ));
    assert!(actor.contains("\n3 of 3 records match;"), "{actor}");

    let none = ok(&find(&fixture, &["--key", "pkg=HDF5"]));
    assert!(none.starts_with("0 of 3 records match;"), "{none}");
    let disjoint = ok(&find(
        &fixture,
        &["--key", "pkg=zlib", "--key", "route=kvm"],
    ));
    assert!(disjoint.starts_with("0 of 3 records match;"), "{disjoint}");

    let json = ok(&find(&fixture, &["--key", "pkg=PETSc", "--json"]));
    let found: Value = serde_json::from_str(&json).expect("find json");
    assert_eq!(found.as_array().unwrap().len(), 2);
    assert_eq!(found[0]["index"], 0);
    assert_eq!(found[1]["module"], "PETSc/3.25.1-foss-2026.1");

    let malformed = eb(&[
        "ledger",
        "find",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
        "--key",
        "pkg",
    ]);
    assert_eq!(malformed.status.code(), Some(2));
    let bare = eb(&[
        "ledger",
        "find",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
    ]);
    assert_eq!(bare.status.code(), Some(2));

    for head in &heads {
        let out = ok(&verify_record(&fixture, 0, head));
        assert!(
            out.contains("tile log size 3 root ") && out.contains("extends the head"),
            "{out}"
        );
    }
}

/// The hex root of the tile log's tree state.
fn tree_root(fixture: &Fixture) -> String {
    let state = std::fs::read_to_string(fixture.tiles.join(".state/treeState")).unwrap();
    state.lines().nth(1).unwrap().trim().to_string()
}

#[test]
fn records_past_the_checkpoint_are_counted_and_not_shown() {
    let fixture = fixture();
    three(&fixture);
    let out = ok(&append_plain(
        &fixture,
        &record("PETSc/3.25.2-foss-2026.1", "kvm", "snellius/4414"),
    ));
    assert!(out.contains("tile log size 4 root "), "{out}");
    assert!(!out.contains("checkpoint "), "no key, no checkpoint: {out}");

    let kvm = ok(&find(&fixture, &["--key", "route=kvm"]));
    let rows = kvm.lines().collect::<Vec<_>>();
    assert!(rows[0].starts_with("0\t"), "{kvm}");
    assert_eq!(rows.len(), 3, "{kvm}");
    assert!(rows[1].starts_with("1 of 3 records match;"), "{kvm}");
    assert_eq!(
        rows[2],
        "1 more past the checkpoint, not shown; `ledger head` signs them"
    );

    let anchor = fixture.work.join("anchor.json");
    let signed = ok(&eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&anchor),
    ]));
    assert!(signed.starts_with("signed head size 4 root "), "{signed}");
    assert!(
        signed.contains("checkpoint eb-stack-ledger/2026.06/2026 size 4 root "),
        "{signed}"
    );
    let kvm = ok(&find(&fixture, &["--key", "route=kvm"]));
    assert!(kvm.contains("\n2 of 4 records match;"), "{kvm}");
    assert!(!kvm.contains("past the checkpoint"), "{kvm}");
}

#[test]
fn ledger_tiles_syncs_an_existing_log_into_any_directory() {
    let fixture = fixture();
    let heads = three(&fixture);
    let other = fixture.work.join("elsewhere").join("tiles");
    let built = ok(&eb(&[
        "ledger",
        "tiles",
        "--log",
        s(&fixture.log),
        "--tiles",
        s(&other),
    ]));
    assert!(
        built.starts_with(&format!(
            "tile log size 3 root {} at {}; 3 appended",
            tree_root(&fixture),
            other.display()
        )),
        "{built}"
    );
    let again = ok(&eb(&[
        "ledger",
        "tiles",
        "--log",
        s(&fixture.log),
        "--tiles",
        s(&other),
    ]));
    assert!(again.ends_with("; 0 appended\n"), "{again}");

    // No checkpoint yet in the other directory.
    let error = failed(&find(
        &fixture,
        &["--key", "pkg=PETSc", "--tiles", s(&other)],
    ));
    assert!(error.contains("has no checkpoint"), "{error}");
    let anchor = fixture.work.join("anchor.json");
    ok(&eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&anchor),
        "--tiles",
        s(&other),
    ]));
    let found = ok(&find(
        &fixture,
        &["--key", "pkg=PETSc", "--tiles", s(&other)],
    ));
    assert!(found.contains("\n2 of 3 records match;"), "{found}");
    let verified = ok(&eb(&[
        "ledger",
        "verify",
        "--log",
        s(&fixture.log),
        "--public-key",
        s(&fixture.public),
        "--record",
        "1",
        "--head",
        s(&heads[2]),
        "--tiles",
        s(&other),
    ]));
    assert!(verified.contains("tile log size 3 root "), "{verified}");
}

#[test]
fn a_custom_origin_is_needed_to_read_the_checkpoint_it_signed() {
    let fixture = fixture();
    three(&fixture);
    let anchor = fixture.work.join("anchor.json");
    let signed = ok(&eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&anchor),
        "--origin",
        "example.org/eessi/2026.06",
    ]));
    assert!(
        signed.contains("checkpoint example.org/eessi/2026.06 size 3 root "),
        "{signed}"
    );
    let error = failed(&find(&fixture, &["--key", "pkg=PETSc"]));
    assert!(
        error.contains("not signed by the trusted key as eb-stack-ledger/2026.06/2026"),
        "{error}"
    );
    let found = ok(&find(
        &fixture,
        &[
            "--key",
            "pkg=PETSc",
            "--origin",
            "example.org/eessi/2026.06",
        ],
    ));
    assert!(
        found.contains("checkpoint example.org/eessi/2026.06 size 3"),
        "{found}"
    );
    let spaced = eb(&[
        "ledger",
        "head",
        "--log",
        s(&fixture.log),
        "--key",
        s(&fixture.key),
        "--out",
        s(&anchor),
        "--origin",
        "has space",
    ]);
    assert_eq!(spaced.status.code(), Some(2));
}

#[test]
fn a_checkpoint_signed_by_another_key_fails_find() {
    let fixture = fixture();
    three(&fixture);
    let other = SigningKey::from_bytes(&[12u8; 32]);
    std::fs::write(
        &fixture.public,
        other
            .verifying_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap(),
    )
    .unwrap();
    let error = failed(&find(&fixture, &["--key", "pkg=PETSc"]));
    assert!(error.contains("not signed by the trusted key"), "{error}");
}

#[test]
fn a_tampered_entry_bundle_fails_verify_and_find() {
    let fixture = fixture();
    let heads = three(&fixture);
    // Flip a bit inside the first entry's bytes, past its length prefix.
    flip_a_bit(&fixture.tiles.join("tile/entries/000.p/3"), 10);
    let error = failed(&verify_record(&fixture, 0, &heads[2]));
    assert!(error.contains("tile log"), "{error}");
    assert!(error.contains("does not hash to its leaf"), "{error}");
    let error = failed(&find(&fixture, &["--key", "pkg=PETSc"]));
    assert!(error.contains("tile log"), "{error}");
}

#[test]
fn a_tampered_hash_tile_fails_verify_and_find() {
    let fixture = fixture();
    let heads = three(&fixture);
    flip_a_bit(&fixture.tiles.join("tile/0/000.p/3"), 40);
    let error = failed(&verify_record(&fixture, 1, &heads[2]));
    assert!(error.contains("the partial tiles fold to"), "{error}");
    let error = failed(&find(&fixture, &["--key", "route=native"]));
    assert!(error.contains("the partial tiles fold to"), "{error}");
}

#[test]
fn a_tampered_tree_state_fails_verify() {
    let fixture = fixture();
    let heads = three(&fixture);
    let state = fixture.tiles.join(".state/treeState");
    let text = std::fs::read_to_string(&state).unwrap();
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    lines[1] = lines[1].replacen(|c: char| c != '0', "0", 1);
    std::fs::write(&state, format!("{}\n", lines.join("\n"))).unwrap();
    let error = failed(&verify_range(&fixture, &heads[0], &heads[2]));
    assert!(error.contains("tile log"), "{error}");
}

#[test]
fn a_tampered_ledger_line_is_caught_by_the_tile_log_on_sync() {
    let fixture = fixture();
    let heads = three(&fixture);
    let mut values = records(&fixture);
    values[1]["actor"] = json!("someone-else");
    rewrite(&fixture, &values);

    // The tile log still holds the signed lines: proofs against the head
    // fail on the ledger, and the stale ledger line differs from the tile.
    let error = failed(&verify_record(&fixture, 1, &heads[2]));
    assert!(error.contains("is not in the head of size 3"), "{error}");

    // A sync notices the disagreement, keeps the old log, and rebuilds.
    let synced = eb(&["ledger", "tiles", "--log", s(&fixture.log)]);
    let out = ok(&synced);
    assert!(out.contains("; 3 appended"), "{out}");
    let warning = String::from_utf8_lossy(&synced.stderr);
    assert!(
        warning.contains("disagreed with the ledger and was rebuilt"),
        "{warning}"
    );
    let stale = std::fs::read_dir(fixture.log.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("2026.06.jsonl.tiles.stale-"))
        .count();
    assert_eq!(stale, 1);
    // The rebuilt log carries no checkpoint until a head signs one, and it
    // still does not extend the signed heads.
    let error = failed(&find(&fixture, &["--key", "pkg=PETSc"]));
    assert!(error.contains("has no checkpoint"), "{error}");
    let error = failed(&verify_record(&fixture, 0, &heads[2]));
    assert!(error.contains("is not in the head of size 3"), "{error}");
    // Record 0 is unchanged: the ledger proves it against the first head,
    // and the rebuilt tile log extends that head.
    let out = ok(&verify_record(&fixture, 0, &heads[0]));
    assert!(out.contains("tile log size 3 root "), "{out}");
}

#[test]
fn a_tile_log_of_another_ledger_fails_verify() {
    let fixture = fixture();
    let heads = three(&fixture);
    let other = fixture_with_key(11);
    for (module, route, job) in [
        ("HDF5/1.14.6-gompi-2026.1", "kvm", "snellius/5000"),
        ("HDF5/1.14.7-gompi-2026.1", "kvm", "snellius/5001"),
        ("zlib/1.3.1-GCCcore-15.2.0", "native", "snellius/5002"),
    ] {
        ok(&append_plain(&other, &record(module, route, job)));
    }
    std::fs::remove_dir_all(&fixture.tiles).unwrap();
    copy_dir(&other.tiles, &fixture.tiles);
    let error = failed(&verify_record(&fixture, 2, &heads[2]));
    assert!(
        error.contains("is at root") && error.contains("the head says"),
        "{error}"
    );
    let error = failed(&verify_range(&fixture, &heads[0], &heads[1]));
    assert!(
        error.contains("does not extend the head of size 2"),
        "{error}"
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn verify_needs_the_tile_log() {
    let fixture = fixture();
    let heads = three(&fixture);
    std::fs::remove_dir_all(&fixture.tiles).unwrap();
    let error = failed(&verify_record(&fixture, 0, &heads[0]));
    assert!(error.contains("no tile log at"), "{error}");
    let error = failed(&find(&fixture, &["--key", "pkg=PETSc"]));
    assert!(error.contains("no tile log at"), "{error}");
    ok(&eb(&["ledger", "tiles", "--log", s(&fixture.log)]));
    ok(&verify_record(&fixture, 0, &heads[0]));
}

#[test]
fn a_record_with_an_unknown_route_is_refused() {
    let fixture = fixture();
    let head = fixture.work.join("head.json");
    let error = failed(&append_signed(
        &fixture,
        &record("zlib/1.3.1-GCCcore-15.2.0", "docker", "snellius/1"),
        &head,
    ));
    assert!(
        error.contains("route \"docker\" is not one of native, kvm, qemu"),
        "{error}"
    );
    assert!(
        !fixture.log.exists() || std::fs::read(&fixture.log).unwrap().is_empty(),
        "nothing is appended"
    );
    assert!(!fixture.tiles.exists(), "no tile log is written");
}

/// A backfill record as the Snellius build jobs write it: a staged install
/// through KVM on an accelerator target, with the build and easyconfig
/// references.
fn backfill_record(module: &str, route: &str, target: &str, job: &str) -> Value {
    json!({
        "ts": "2026-11-05T22:14:09Z",
        "actor": "jenkins/eessi-2026.06/1187",
        "stack": "2026.06",
        "target": target,
        "module": module,
        "location": format!(
            "/cvmfs/software.eessi.io/host_injections/2026.06/software/linux/{target}/software/{module}"
        ),
        "action": "install",
        "stage": "finished",
        "route": route,
        "build": {
            "job": job,
            "phase": "build",
            "status": "succeeded",
            "summary_log": format!("{job}/summary.log"),
            "summary_sha256": "11".repeat(32),
            "node": "gcn42",
            "duration_s": 5311,
        },
        "easyconfig": {"file": format!("{}.eb", module.replace('/', "-"))},
        "reason": {"text": "2026.06 backfill", "refs": ["software-layer#1234"]},
    })
}

#[test]
fn backfill_records_with_route_and_accelerator_targets_are_found() {
    let fixture = fixture();
    let icelake = "x86_64/intel/icelake+accel/nvidia/cc80";
    let head = fixture.work.join("head.json");
    for (module, route, target, job) in [
        ("GROMACS/2026.1-foss-2026.1-CUDA-13.0.2", "kvm", icelake, "snellius/4411"),
        ("GROMACS/2026.1-foss-2026.1", "native", "x86_64/intel/icelake", "snellius/4412"),
        ("LAMMPS/2Aug2025-foss-2026.1-kokkos-CUDA-13.0.2", "kvm", icelake, "snellius/4413"),
        ("LAMMPS/2Aug2025-foss-2026.1-kokkos", "kvm", "x86_64/amd/zen4", "snellius/4414"),
    ] {
        ok(&append_signed(
            &fixture,
            &backfill_record(module, route, target, job),
            &head,
        ));
    }
    let hits = ok(&find(
        &fixture,
        &["--key", "route=kvm", "--key", &format!("target={icelake}")],
    ));
    let rows = hits.lines().collect::<Vec<_>>();
    assert_eq!(rows.len(), 3, "{hits}");
    assert!(rows[0].starts_with("0	"), "{hits}");
    assert!(rows[1].starts_with("2	"), "{hits}");
    assert!(rows[2].starts_with("2 of 4 records match;"), "{hits}");
    assert!(rows[0].contains("\"route\":\"kvm\""), "{hits}");
    let kvm = ok(&find(&fixture, &["--key", "route=kvm"]));
    assert!(kvm.contains("\n3 of 4 records match;"), "{kvm}");
    let job = ok(&find(&fixture, &["--key", "job=snellius/4414", "--json"]));
    let found: Value = serde_json::from_str(&job).unwrap();
    assert_eq!(found[0]["module"], "LAMMPS/2Aug2025-foss-2026.1-kokkos");
    assert_eq!(found[0]["build"]["status"], "succeeded");
    let shown = ok(&eb(&[
        "ledger",
        "show",
        "--log",
        s(&fixture.log),
        "--package",
        "GROMACS",
    ]));
    assert!(shown.contains("2 of 4 records"), "{shown}");
    ok(&verify_record(&fixture, 3, &head));
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

/// The structured body of a tool result.
fn body(result: &Value) -> &Value {
    &result["structuredContent"]
}

#[test]
fn mcp_ledger_tiles_and_find_mirror_the_commands() {
    let fixture = fixture();
    three(&fixture);
    let other = fixture.work.join("mcp-tiles");
    let synced = mcp(
        "eb_ledger_tiles",
        json!({"log": s(&fixture.log), "tiles": s(&other)}),
    );
    assert_ne!(synced["isError"], true, "{synced}");
    assert_eq!(body(&synced)["tiles"]["size"], 3, "{synced}");
    assert_eq!(body(&synced)["tiles"]["appended"], 3, "{synced}");
    let anchor = fixture.work.join("anchor.json");
    let head = mcp(
        "eb_ledger_head",
        json!({
            "log": s(&fixture.log),
            "key": s(&fixture.key),
            "out": s(&anchor),
            "tiles": s(&other),
        }),
    );
    assert_ne!(head["isError"], true, "{head}");
    assert_eq!(
        body(&head)["checkpoint"]["origin"],
        "eb-stack-ledger/2026.06/2026",
        "{head}"
    );
    assert_eq!(body(&head)["checkpoint"]["size"], 3, "{head}");
    let found = mcp(
        "eb_ledger_find",
        json!({
            "log": s(&fixture.log),
            "public_key": s(&fixture.public),
            "keys": ["pkg=PETSc", "route=kvm"],
            "tiles": s(&other),
        }),
    );
    assert_ne!(found["isError"], true, "{found}");
    let found = body(&found);
    assert_eq!(found["found"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["found"][0]["index"], 0, "{found}");
    assert_eq!(found["unsigned"], 0, "{found}");
    assert_eq!(found["checkpoint_size"], 3, "{found}");
    let empty = mcp(
        "eb_ledger_find",
        json!({
            "log": s(&fixture.log),
            "public_key": s(&fixture.public),
            "tiles": s(&other),
        }),
    );
    assert_eq!(empty["isError"], true, "{empty}");
    let verified = mcp(
        "eb_ledger_verify",
        json!({
            "log": s(&fixture.log),
            "public_key": s(&fixture.public),
            "record": 2,
            "head": s(&anchor),
            "tiles": s(&other),
        }),
    );
    assert_ne!(verified["isError"], true, "{verified}");
    assert_eq!(body(&verified)["tiles_size"], 3, "{verified}");
}
