//! Crash, alias and concurrency behavior of the ledger writer and signer.

#![cfg(unix)]

use eb_stack::ledger::{self, SignedHead};
use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};
use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const SEED: [u8; 32] = [23; 32];

struct Fixture {
    root: tempfile::TempDir,
    dir: PathBuf,
    log: PathBuf,
    lock: PathBuf,
    key: PathBuf,
    work: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for name in ["ledger", "keys", "work"] {
            fs::create_dir(root.path().join(name)).unwrap();
        }
        let key = root.path().join("keys/key.pem");
        let pem = SigningKey::from_bytes(&SEED)
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap();
        fs::write(&key, pem.as_bytes()).unwrap();
        Self {
            dir: root.path().join("ledger"),
            log: root.path().join("ledger/stack.jsonl"),
            lock: root.path().join("ledger/stack.jsonl.posix-lock"),
            work: root.path().join("work"),
            key,
            root,
        }
    }

    fn record_file(&self, event: &str) -> PathBuf {
        let path = self.work.join(format!("{}.json", event.replace('/', "-")));
        fs::write(&path, serde_json::to_vec(&record(event)).unwrap()).unwrap();
        path
    }

    fn append(&self, event: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        command
            .args(["ledger", "append"])
            .arg(self.record_file(event))
            .arg("--log")
            .arg(&self.log);
        command
    }

    fn append_signed(&self, event: &str, head: &Path) -> Command {
        let mut command = self.append(event);
        command
            .arg("--key")
            .arg(&self.key)
            .arg("--head-out")
            .arg(head);
        command
    }

    fn head(&self, out: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        command
            .args(["ledger", "head", "--log"])
            .arg(&self.log)
            .arg("--key")
            .arg(&self.key)
            .arg("--out")
            .arg(out);
        command
    }

    fn entries(&self) -> Vec<ledger::Entry> {
        ledger::read_log(&self.log).unwrap()
    }

    fn ledger_files(&self) -> Vec<String> {
        let mut names = fs::read_dir(&self.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        names
    }
}

fn record(event: &str) -> Value {
    json!({
        "ts": "2026-11-05T00:00:00Z", "actor": "durability-test", "stack": "2026.06",
        "target": "x86_64/amd/zen4", "module": "example/1", "location": "/example/1",
        "action": "install", "build": {"event_id": event}
    })
}

fn line(event: &str, index: usize) -> String {
    ledger::seal_record(&record(event), index, "").unwrap().line
}

fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn failure(output: &Output) -> String {
    assert!(
        !output.status.success(),
        "expected a refusal\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Run with writes past `bytes` failing with `EFBIG`, as a full disk or
/// quota fails them part way.
fn with_file_size_limit(command: &mut Command, bytes: u64) -> Output {
    // The closure only calls async-signal-safe functions between fork and exec.
    unsafe {
        command.pre_exec(move || {
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            let limit = libc::rlimit {
                rlim_cur: bytes as libc::rlim_t,
                rlim_max: bytes as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.output().unwrap()
}

fn record_lock(file: &fs::File, kind: libc::c_int) {
    // The descriptor is live and the initialized flock covers the whole file.
    let mut lock: libc::flock = unsafe { std::mem::zeroed() };
    lock.l_type = kind as _;
    lock.l_whence = libc::SEEK_SET as _;
    assert_eq!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
}

fn check_head(fixture: &Fixture, head: &Path) -> SignedHead {
    let signed = SignedHead::read(head).unwrap();
    signed
        .check(&SigningKey::from_bytes(&SEED).verifying_key())
        .unwrap();
    let entries = fixture.entries();
    assert_eq!(
        signed.root,
        rfc9162::Head::over(&ledger::leaves(&entries[..signed.size])).root
    );
    signed
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_short_append_is_cut_back_and_the_next_append_succeeds() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    let original = fs::read(&fixture.log).unwrap();
    let error = failure(&with_file_size_limit(
        &mut fixture.append("second"),
        original.len() as u64 + 20,
    ));
    assert!(error.contains("append to ledger"), "{error}");
    assert_eq!(fs::read(&fixture.log).unwrap(), original);
    success(&fixture.append("second").output().unwrap());
    assert_eq!(fixture.entries().len(), 2);
    assert!(
        !fixture
            .ledger_files()
            .iter()
            .any(|name| name.contains("quarantine")),
        "a rolled back write leaves no tail to quarantine"
    );
}

#[test]
fn an_interrupted_tail_is_quarantined_before_the_log_is_repaired() {
    let fixture = Fixture::new();
    let first = line("first", 0);
    let torn = line("torn", 1);
    let mut tail = torn.as_bytes()[..40].to_vec();
    tail.extend_from_slice("é".as_bytes().split_at(1).0);
    let mut bytes = format!("{first}\n").into_bytes();
    bytes.extend_from_slice(&tail);
    fs::write(&fixture.log, &bytes).unwrap();

    let out = success(&fixture.append("second").output().unwrap());
    assert!(out.starts_with("appended record 1 id "), "{out}");
    assert!(out.contains("interrupted append"), "{out}");
    let text = fs::read_to_string(&fixture.log).unwrap();
    assert_eq!(text, format!("{first}\n{}\n", line("second", 1)));
    let quarantined = fixture
        .ledger_files()
        .into_iter()
        .filter(|name| name.starts_with("stack.jsonl.quarantine."))
        .collect::<Vec<_>>();
    assert_eq!(quarantined.len(), 1, "{:?}", fixture.ledger_files());
    assert_eq!(fs::read(fixture.dir.join(&quarantined[0])).unwrap(), tail);
}

#[test]
fn an_acknowledged_retry_also_repairs_an_interrupted_tail() {
    let fixture = Fixture::new();
    let head = fixture.work.join("head.json");
    success(&fixture.append_signed("first", &head).output().unwrap());
    let committed = fs::read(&fixture.log).unwrap();
    let mut bytes = committed.clone();
    bytes.extend_from_slice(&line("first", 1).as_bytes()[..17]);
    fs::write(&fixture.log, &bytes).unwrap();
    let out = success(&fixture.append_signed("first", &head).output().unwrap());
    assert!(out.starts_with("already recorded record 0"), "{out}");
    assert_eq!(fs::read(&fixture.log).unwrap(), committed);
    assert_eq!(check_head(&fixture, &head).size, 1);
}

#[test]
fn a_complete_record_without_its_newline_is_not_repaired() {
    let fixture = Fixture::new();
    let original = format!("{}\n{}", line("first", 0), line("second", 1));
    fs::write(&fixture.log, &original).unwrap();
    failure(&fixture.append("third").output().unwrap());
    assert_eq!(fs::read_to_string(&fixture.log).unwrap(), original);
    assert!(!fixture
        .ledger_files()
        .iter()
        .any(|name| name.contains("quarantine")));
}

#[test]
fn a_tail_after_a_corrupt_history_is_left_alone() {
    let fixture = Fixture::new();
    let mut original = format!("{}\n", line("first", 0)).replacen(':', ": ", 1);
    original.push_str(&line("second", 1)[..30]);
    fs::write(&fixture.log, &original).unwrap();
    failure(&fixture.append("third").output().unwrap());
    assert_eq!(fs::read_to_string(&fixture.log).unwrap(), original);
    assert!(!fixture
        .ledger_files()
        .iter()
        .any(|name| name.contains("quarantine")));
}

#[test]
fn an_unsaved_tail_stays_in_the_log() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    let mut original = fs::read(&fixture.log).unwrap();
    original.extend_from_slice(&line("torn", 1).as_bytes()[..25]);
    fs::write(&fixture.log, &original).unwrap();
    chmod(&fixture.dir, 0o555);
    let output = fixture.append("second").output().unwrap();
    chmod(&fixture.dir, 0o755);
    failure(&output);
    assert_eq!(fs::read(&fixture.log).unwrap(), original);
}

#[test]
fn a_failed_head_write_keeps_the_previous_head() {
    let fixture = Fixture::new();
    let head = fixture.work.join("head.json");
    success(&fixture.append_signed("first", &head).output().unwrap());
    success(&fixture.append("second").output().unwrap());
    let previous = fs::read(&head).unwrap();
    failure(&with_file_size_limit(&mut fixture.head(&head), 16));
    assert_eq!(fs::read(&head).unwrap(), previous);
    assert_eq!(check_head(&fixture, &head).size, 1);
    let names = fs::read_dir(&fixture.work)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(names.is_empty(), "temporary files left: {names:?}");
}

#[test]
fn a_head_path_naming_the_log_is_refused() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    let original = fs::read(&fixture.log).unwrap();
    let lexical = fixture.dir.join(".").join("stack.jsonl");
    failure(&fixture.append_signed("second", &lexical).output().unwrap());
    assert_eq!(fs::read(&fixture.log).unwrap(), original);
    let dotted = fixture.work.join("..").join("ledger/stack.jsonl");
    failure(&fixture.head(&dotted).output().unwrap());
    assert_eq!(fs::read(&fixture.log).unwrap(), original);
    let link = fixture.work.join("head.json");
    std::os::unix::fs::symlink(&fixture.log, &link).unwrap();
    failure(&fixture.head(&link).output().unwrap());
    failure(&fixture.append_signed("second", &link).output().unwrap());
    assert_eq!(fs::read(&fixture.log).unwrap(), original);
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn a_head_path_naming_the_record_lock_or_a_quarantine_is_refused() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    let lock = fs::metadata(&fixture.lock).unwrap();
    failure(&fixture.head(&fixture.lock).output().unwrap());
    let after = fs::metadata(&fixture.lock).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!((lock.ino(), lock.len()), (after.ino(), after.len()));
    let quarantine = fixture.dir.join("stack.jsonl.quarantine.saved");
    fs::write(&quarantine, b"{\"torn").unwrap();
    failure(&fixture.head(&quarantine).output().unwrap());
    assert_eq!(fs::read(&quarantine).unwrap(), b"{\"torn");
}

#[test]
fn a_head_path_naming_the_signing_key_is_refused() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    let key = fs::read(&fixture.key).unwrap();
    let lexical = fixture.work.join("../keys/./key.pem");
    failure(&fixture.append_signed("second", &lexical).output().unwrap());
    failure(&fixture.head(&lexical).output().unwrap());
    let link = fixture.work.join("key-link.pem");
    std::os::unix::fs::symlink(&fixture.key, &link).unwrap();
    failure(&fixture.head(&link).output().unwrap());
    let hard = fixture.work.join("key-hard.pem");
    fs::hard_link(&fixture.key, &hard).unwrap();
    failure(&fixture.head(&hard).output().unwrap());
    failure(&fixture.append_signed("second", &hard).output().unwrap());
    assert_eq!(fs::read(&fixture.key).unwrap(), key);
    assert_eq!(fs::read(&hard).unwrap(), key);
    assert_eq!(fixture.entries().len(), 1);
}

#[test]
fn a_signing_key_hard_linked_into_the_ledger_directory_is_refused() {
    let fixture = Fixture::new();
    fs::hard_link(&fixture.key, fixture.dir.join("travels.pem")).unwrap();
    let head = fixture.work.join("head.json");
    failure(&fixture.append_signed("first", &head).output().unwrap());
    assert!(!fixture.log.exists(), "nothing is appended");
    assert!(!head.exists());
    fs::write(&fixture.log, "").unwrap();
    failure(&fixture.head(&head).output().unwrap());
    assert!(!head.exists());
}

#[test]
fn a_signer_waits_for_the_writer_and_signs_its_append() {
    let fixture = Fixture::new();
    let head = fixture.work.join("head.json");
    success(&fixture.append("first").output().unwrap());
    let holder = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fixture.lock)
        .unwrap();
    record_lock(&holder, libc::F_WRLCK);
    let mut signer = fixture
        .head(&head)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        signer.try_wait().unwrap().is_none(),
        "the signer read the log while a writer held it"
    );
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(&fixture.log)
        .unwrap();
    std::io::Write::write_all(&mut log, format!("{}\n", line("second", 1)).as_bytes()).unwrap();
    drop(holder);
    success(&signer.wait_with_output().unwrap());
    assert_eq!(check_head(&fixture, &head).size, 2);
}

#[test]
fn a_read_only_signer_needs_no_write_access_to_the_ledger() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    success(&fixture.append("second").output().unwrap());
    let before = fixture.ledger_files();
    let reader = fs::File::open(&fixture.lock).unwrap();
    record_lock(&reader, libc::F_RDLCK);
    chmod(&fixture.log, 0o444);
    chmod(&fixture.lock, 0o444);
    chmod(&fixture.dir, 0o555);
    let head = fixture.work.join("head.json");
    let output = fixture.head(&head).output().unwrap();
    chmod(&fixture.dir, 0o755);
    success(&output);
    assert_eq!(check_head(&fixture, &head).size, 2);
    assert_eq!(fixture.ledger_files(), before);
}

#[test]
fn a_read_only_signer_still_waits_for_the_writer() {
    let fixture = Fixture::new();
    success(&fixture.append("first").output().unwrap());
    let holder = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fixture.lock)
        .unwrap();
    record_lock(&holder, libc::F_WRLCK);
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(&fixture.log)
        .unwrap();
    chmod(&fixture.log, 0o444);
    chmod(&fixture.lock, 0o444);
    chmod(&fixture.dir, 0o555);
    let head = fixture.work.join("head.json");
    let mut signer = fixture
        .head(&head)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let waited = signer.try_wait().unwrap().is_none();
    std::io::Write::write_all(&mut log, format!("{}\n", line("second", 1)).as_bytes()).unwrap();
    drop(holder);
    let output = signer.wait_with_output().unwrap();
    chmod(&fixture.dir, 0o755);
    assert!(waited, "the read-only signer did not wait for the writer");
    success(&output);
    assert_eq!(check_head(&fixture, &head).size, 2);
}

#[test]
fn a_read_only_signer_refuses_a_log_without_its_record_lock() {
    let fixture = Fixture::new();
    fs::write(&fixture.log, format!("{}\n", line("first", 0))).unwrap();
    chmod(&fixture.dir, 0o555);
    let head = fixture.work.join("head.json");
    let output = fixture.head(&head).output().unwrap();
    chmod(&fixture.dir, 0o755);
    let error = failure(&output);
    assert!(error.contains("record lock"), "{error}");
    assert!(!head.exists());
    assert_eq!(fixture.ledger_files(), vec!["stack.jsonl".to_string()]);
}

#[test]
fn concurrent_writers_and_signers_leave_the_newest_head() {
    let fixture = Fixture::new();
    let head = fixture.work.join("head.json");
    success(&fixture.append("seed").output().unwrap());
    let mut total = 1;
    for round in 0..3 {
        let mut children = Vec::new();
        for n in 0..8 {
            let event = format!("round-{round}/event-{n}");
            children.push(
                fixture
                    .append_signed(&event, &head)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            children.push(
                fixture
                    .head(&head)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
        }
        for child in children {
            success(&child.wait_with_output().unwrap());
        }
        total += 8;
        assert_eq!(fixture.entries().len(), total);
        assert_eq!(check_head(&fixture, &head).size, total, "round {round}");
    }
}

#[test]
fn in_process_signers_keep_cross_process_writers_out() {
    let fixture = Fixture::new();
    let head = fixture.work.join("head.json");
    success(&fixture.append("seed").output().unwrap());
    let mut children = (0..12)
        .map(|n| {
            fixture
                .append_signed(&format!("child-{n}"), &head)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect::<Vec<_>>();
    let records = (0..12)
        .map(|n| fixture.record_file(&format!("thread-{n}")))
        .collect::<Vec<_>>();
    std::thread::scope(|scope| {
        for n in 0..4 {
            let (fixture, head) = (&fixture, &head);
            scope.spawn(move || {
                for _ in 0..15 {
                    ledger::sign_head(&ledger::HeadRequest {
                        log: fixture.log.clone(),
                        key: fixture.key.clone(),
                        out: head.clone(),
                        ts: None,
                    })
                    .unwrap_or_else(|error| panic!("signer {n}: {error:#}"));
                }
            });
        }
        for chunk in records.chunks(3) {
            let (fixture, head) = (&fixture, &head);
            scope.spawn(move || {
                for record in chunk {
                    ledger::append(&ledger::AppendRequest {
                        log: fixture.log.clone(),
                        record: record.clone(),
                        expect_head: None,
                        key: Some(fixture.key.clone()),
                        head_out: Some(head.clone()),
                    })
                    .unwrap_or_else(|error| panic!("writer: {error:#}"));
                }
            });
        }
    });
    for child in children.drain(..) {
        success(&child.wait_with_output().unwrap());
    }
    assert_eq!(fixture.entries().len(), 25);
    assert_eq!(check_head(&fixture, &head).size, 25);
    drop(fixture.root);
}
