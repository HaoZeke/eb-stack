//! Explicit waiting forecasts arbitrate through the same admission lock.
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _directory: tempfile::TempDir,
    quota: PathBuf,
    reservations: PathBuf,
    lock: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let quota = directory.path().join("quota.json");
        let reservations = directory.path().join("reservations");
        let lock = directory.path().join("lock");
        std::fs::create_dir(&reservations).unwrap();
        std::fs::write(&quota, r#"{"inodes_available":500}"#).unwrap();
        Self { _directory: directory, quota, reservations, lock }
    }

    fn record(&self, job: &str, phase: Option<&str>) {
        let mut record = json!({"job":job,"footprint":300,"margin":100,"ts":"2026-11-12T00:00:00Z"});
        if let Some(phase) = phase {
            record["phase"] = json!(phase);
        }
        std::fs::write(self.reservations.join(format!("{job}.json")), record.to_string()).unwrap();
    }

    fn read(&self, job: &str) -> Value {
        serde_json::from_slice(&std::fs::read(self.reservations.join(format!("{job}.json"))).unwrap()).unwrap()
    }

    fn command(&self, job: &str, lock: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        command.args(["stack", "admit", "--quota-json"]);
        command.arg(&self.quota);
        command.args(["--reservations"]).arg(&self.reservations);
        command.args(["--job", job, "--footprint", "300", "--margin", "100",
                      "--unknown-reserve", "200", "--live-jobs", "11,12"]);
        if lock {
            command.arg("--lock").arg(&self.lock).args(["--lock-kind", "posix"]);
        }
        command
    }

    fn admit(&self, job: &str, lock: bool) -> (Output, Value) {
        let output = self.command(job, lock).output().unwrap();
        let report = serde_json::from_slice(&output.stdout).unwrap();
        (output, report)
    }
}

#[test]
fn waiting_candidates_admit_one_writer_without_erasing_declared_peaks() {
    let fixture = Fixture::new();
    fixture.record("11", Some("forecast"));
    fixture.record("12", Some("forecast"));
    let (first, report) = fixture.admit("11", true);
    assert_eq!(first.status.code(), Some(78), "{report}");
    assert_eq!(report["reserved"], 300);
    assert_eq!(fixture.read("11")["phase"], "waiting");
    assert_eq!(fixture.read("11")["footprint"], 300);
    let (second, report) = fixture.admit("12", true);
    assert!(second.status.success(), "{report}");
    assert_eq!(report["reserved"], 0);
    assert_eq!(report["unknown"], json!([]));
    assert_eq!(fixture.read("12")["phase"], "admitted");
    assert_eq!(fixture.read("12")["footprint"], 300);
    let (again, report) = fixture.admit("11", true);
    assert_eq!(again.status.code(), Some(78), "{report}");
    assert_eq!(report["reserved"], 300);
}

#[test]
fn legacy_and_setup_records_keep_their_full_forecasts() {
    for phase in [None, Some("forecast"), Some("admitted")] {
        let fixture = Fixture::new();
        fixture.record("11", phase);
        fixture.record("12", Some("forecast"));
        let (output, report) = fixture.admit("12", true);
        assert_eq!(output.status.code(), Some(78), "{report}");
        assert_eq!(report["reserved"], 300);
        assert_eq!(fixture.read("11")["footprint"], 300);
        assert_eq!(fixture.read("11").get("phase").and_then(Value::as_str), phase);
    }
}

#[test]
fn waiting_records_are_fully_counted_without_a_shared_lock() {
    let fixture = Fixture::new();
    fixture.record("11", Some("waiting"));
    fixture.record("12", Some("forecast"));
    let (output, report) = fixture.admit("12", false);
    assert_eq!(output.status.code(), Some(78), "{report}");
    assert_eq!(report["reserved"], 300);
    assert_eq!(fixture.read("12")["phase"], "forecast");
}

#[test]
fn invalid_lifecycle_refuses_without_mutating_records() {
    let fixture = Fixture::new();
    fixture.record("11", Some("unverified"));
    fixture.record("12", Some("forecast"));
    let before = std::fs::read(fixture.reservations.join("12.json")).unwrap();
    let output = fixture.command("12", true).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reservation phase"));
    assert_eq!(std::fs::read(fixture.reservations.join("12.json")).unwrap(), before);
}

#[test]
fn a_failed_quota_observation_never_admits_a_waiting_candidate() {
    let fixture = Fixture::new();
    fixture.record("11", Some("waiting"));
    fixture.record("12", Some("forecast"));
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "admit", "--quota-cmd", "exit 9", "--reservations"])
        .arg(&fixture.reservations)
        .args(["--job", "12", "--footprint", "300", "--margin", "100", "--lock"])
        .arg(&fixture.lock).args(["--lock-kind", "posix", "--live-jobs", "11,12"])
        .output().unwrap();
    assert_eq!(output.status.code(), Some(78));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["admitted"], false);
    assert_eq!(fixture.read("12")["phase"], "waiting");
}

#[test]
fn independent_processes_cannot_commit_both_large_writers() {
    for _ in 0..4 {
        let fixture = Fixture::new();
        fixture.record("11", Some("forecast"));
        fixture.record("12", Some("forecast"));
        let mut left = fixture.command("11", true).spawn().unwrap();
        let mut right = fixture.command("12", true).spawn().unwrap();
        let mut codes = [left.wait().unwrap().code().unwrap(), right.wait().unwrap().code().unwrap()];
        codes.sort_unstable();
        assert_eq!(codes, [0, 78]);
        let admitted = ["11", "12"].into_iter()
            .filter(|job| fixture.read(job)["phase"] == "admitted").count();
        assert_eq!(admitted, 1);
        assert_eq!(fixture.read("11")["footprint"], 300);
        assert_eq!(fixture.read("12")["footprint"], 300);
    }
}

#[test]
fn stale_waiting_records_follow_the_explicit_stale_removal_policy() {
    let fixture = Fixture::new();
    fixture.record("11", Some("waiting"));
    fixture.record("12", Some("forecast"));
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "admit", "--quota-json"]).arg(&fixture.quota)
        .arg("--reservations").arg(&fixture.reservations)
        .args(["--job", "12", "--footprint", "300", "--margin", "100", "--live-jobs", "12",
               "--drop-stale", "--lock"]).arg(&fixture.lock)
        .args(["--lock-kind", "posix"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!fixture.reservations.join("11.json").exists());
}
