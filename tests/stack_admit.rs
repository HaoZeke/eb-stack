//! `stack admit` and `stack footprints` from the command line: the gate the
//! controller runs before one build job, and the table it reads footprints
//! from.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn eb_stack(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

fn admit(quota: &Path, reservations: &Path, job: &str, footprint: &str, live: Option<&str>) -> Value {
    let mut args = vec![
        "stack",
        "admit",
        "--quota-json",
        quota.to_str().unwrap(),
        "--footprint",
        footprint,
        "--job",
        job,
        "--reservations",
        reservations.to_str().unwrap(),
        "--ts",
        "2026-11-07T12:00:00Z",
    ];
    if let Some(live) = live {
        args.push("--live-jobs");
        args.push(live);
    }
    let run = eb_stack(&args);
    let stdout = String::from_utf8(run.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(stdout.lines().count(), 1, "one JSON line: {stdout}{stderr}");
    let line: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
    let expected = if line["admitted"] == true { 0 } else { 78 };
    assert_eq!(run.status.code(), Some(expected), "{stdout}{stderr}");
    line
}

#[test]
fn a_lane_is_admitted_then_the_next_is_refused_and_a_stale_reservation_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let quota = tmp.path().join("quota.json");
    // 3,000,000 soft minus 2,200,000 used: 800,000 available.
    std::fs::write(
        &quota,
        r#"{"inodes_used": 2200000, "inodes_soft": 3000000, "inodes_hard": 4000000}"#,
    )
    .unwrap();
    let reservations = tmp.path().join("reservations");
    std::fs::create_dir_all(&reservations).unwrap();
    std::fs::write(
        reservations.join("4390.json"),
        r#"{"job": "4390", "footprint": 351000, "ts": "2026-11-06T00:00:00Z"}"#,
    )
    .unwrap();

    // 800,000 - 0 - 351,000 - 100,000 = 349,000: admitted, and 4390, not
    // live any more, is dropped.
    let first = admit(&quota, &reservations, "4401", "351000", Some(""));
    assert_eq!(first["admitted"], true, "{first}");
    assert_eq!(first["reserved"], 0);
    assert_eq!(first["headroom"], 349_000);
    assert_eq!(first["dropped"], serde_json::json!(["4390"]));
    assert!(!reservations.join("4390.json").exists());
    let written: Value = serde_json::from_str(
        &std::fs::read_to_string(reservations.join("4401.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(written["job"], "4401");
    assert_eq!(written["footprint"], 351_000);
    assert_eq!(written["ts"], "2026-11-07T12:00:00Z");

    // 800,000 - 351,000 - 351,000 - 100,000 < 0: refused, exit 78, no file.
    let second = admit(&quota, &reservations, "4402", "351000", Some("4401"));
    assert_eq!(second["admitted"], false, "{second}");
    assert_eq!(second["reserved"], 351_000);
    assert_eq!(second["headroom"], -2_000);
    assert!(!reservations.join("4402.json").exists());

    // A small lane still fits: 800,000 - 351,000 - 61,000 - 100,000.
    let third = admit(&quota, &reservations, "4403", "61000", Some("4401"));
    assert_eq!(third["admitted"], true, "{third}");
    assert_eq!(third["headroom"], 288_000);

    // 4401 asking again is judged on the others alone.
    let again = admit(&quota, &reservations, "4401", "351000", Some("4401,4403"));
    assert_eq!(again["admitted"], true, "{again}");
    assert_eq!(again["reserved"], 61_000);
}

#[test]
fn the_quota_may_come_from_stdin_and_a_bad_job_id_is_a_usage_error() {
    use std::io::Write;
    let tmp = tempfile::tempdir().unwrap();
    let reservations = tmp.path().join("reservations");
    let mut child = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "admit",
            "--quota-json",
            "-",
            "--footprint",
            "61000",
            "--job",
            "4410",
            "--reservations",
            reservations.to_str().unwrap(),
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"inodes_available": 200000}"#)
        .unwrap();
    let run = child.wait_with_output().unwrap();
    assert_eq!(run.status.code(), Some(0), "{}", String::from_utf8_lossy(&run.stderr));
    let line: Value = serde_json::from_str(String::from_utf8_lossy(&run.stdout).trim()).unwrap();
    assert_eq!(line["available"], 200_000);
    assert_eq!(line["headroom"], 39_000);
    assert!(reservations.join("4410.json").exists(), "the directory is created");

    let quota = tmp.path().join("quota.json");
    std::fs::write(&quota, r#"{"inodes_available": 200000}"#).unwrap();
    let bad = eb_stack(&[
        "stack",
        "admit",
        "--quota-json",
        quota.to_str().unwrap(),
        "--footprint",
        "1",
        "--job",
        "../escape",
        "--reservations",
        reservations.to_str().unwrap(),
    ]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("--job"));
}

#[test]
fn footprints_take_the_ledger_peak_then_the_table_then_the_classes() {
    let tmp = tempfile::tempdir().unwrap();
    let ledger = tmp.path().join("2025.jsonl");
    let mut log = String::new();
    for (index, (module, peak)) in [
        ("CP2K/2025.2-foss-2025a", 412_345u64),
        ("CP2K/2025.1-foss-2025a", 380_000),
        ("zlib/1.3.1-GCCcore-14.2.0", 1_100),
    ]
    .iter()
    .enumerate()
    {
        let record = serde_json::json!({
            "ts": "2026-11-03T10:00:00Z",
            "actor": "jenkins/run/1",
            "stack": "2025",
            "target": "x86_64/amd/genoa",
            "module": module,
            "location": format!("/sw/{module}"),
            "action": "install",
            "build": {"job": format!("site/{index}"), "inodes_peak": peak},
        });
        let entry = eb_stack::ledger::seal_record(&record, index, "unused").unwrap();
        log.push_str(&entry.line);
        log.push('\n');
    }
    std::fs::write(&ledger, log).unwrap();
    let table = tmp.path().join("site.tsv");
    std::fs::write(&table, "# name\tinodes\nSeisSol\t200000\nzlib\t5000\n").unwrap();
    let out = tmp.path().join("footprints.json");
    let run = eb_stack(&[
        "stack",
        "footprints",
        "--ledger",
        ledger.to_str().unwrap(),
        "--table",
        table.to_str().unwrap(),
        "--generated-at",
        "2026-11-07T00:00:00Z",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "packages=8 ledger=2 table=1 class=5 records=3\n"
    );
    let document: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(document["generated_at"], "2026-11-07T00:00:00Z");
    assert_eq!(document["default"], 60_000);
    assert_eq!(document["footprints"]["CP2K"], 412_345);
    assert_eq!(document["sources"]["CP2K"]["from"], "ledger");
    assert_eq!(document["sources"]["CP2K"]["module"], "CP2K/2025.2-foss-2025a");
    assert_eq!(document["sources"]["CP2K"]["records"], 2);
    // The table's 5000 beats the ledger's 1100 for zlib.
    assert_eq!(document["footprints"]["zlib"], 5_000);
    assert_eq!(document["sources"]["zlib"]["from"], "ledger+table");
    assert_eq!(document["footprints"]["SeisSol"], 200_000);
    assert_eq!(document["sources"]["SeisSol"]["from"], "table");
    assert_eq!(document["footprints"]["LAMMPS"], 150_000);
    assert_eq!(document["sources"]["LAMMPS"]["from"], "class");
    assert!(document["footprints"].get("HDF5").is_none());
}
