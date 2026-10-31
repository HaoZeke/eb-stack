//! EESSI bot job results, classified.
//!
//! The fixtures are FAILURE comments from EESSI/software-layer pull
//! requests, trimmed to the job table, each with its source URL on the
//! first line. #1873, #1869, #1834 and #1760 built and packaged
//! everything; the FAILURE is the test step's `ERROR: ` line. #1732 and
//! #1733 built nothing because everything was installed. #1818 and #1823
//! failed for real, #1818 on a full disk. #1741 hit bot configuration
//! errors before any job ran.

use eb_stack::bot_result::{
    classify_bot_inputs, classify_bot_job, BotInput, BotJobClass, BotSubClass, InfraCause,
};
use eb_stack::campaign::BuildFindingClass;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/bot_classify_fixtures")
        .join(name)
}

fn input(name: &str) -> BotInput {
    BotInput {
        name: name.into(),
        text: std::fs::read_to_string(fixture(name)).expect("fixture"),
    }
}

fn text(name: &str, text: &str) -> BotInput {
    BotInput {
        name: name.into(),
        text: text.into(),
    }
}

fn build(class: BuildFindingClass) -> Option<BotSubClass> {
    Some(BotSubClass::Build(class))
}

fn infra(cause: InfraCause) -> Option<BotSubClass> {
    Some(BotSubClass::Infra(cause))
}

#[test]
fn each_fixture_names_the_comment_it_came_from() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bot_classify_fixtures");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).expect("fixtures") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).expect("fixture");
        let first = text.lines().next().unwrap_or("");
        assert!(
            first.contains("source: https://github.com/EESSI/software-layer/pull/"),
            "{} has no source URL: {first}",
            path.display()
        );
        count += 1;
    }
    assert_eq!(count, 18);
}

/// Fixture file, class, sub-class, target, job id.
type FixtureCase = (
    &'static str,
    BotJobClass,
    Option<BotSubClass>,
    &'static str,
    Option<&'static str>,
);

#[test]
fn every_fixture_comment_gets_the_class_the_thread_gives_it() {
    use BotJobClass::*;
    let cases: &[FixtureCase] = &[
        (
            "pr1873-zen2-217265.md",
            ErrorOnly,
            None,
            "x86_64/amd/zen2",
            Some("217265"),
        ),
        (
            "pr1869-zen2.md",
            ErrorOnly,
            None,
            "x86_64/amd/zen2",
            Some("216301"),
        ),
        (
            "pr1834-zen2.md",
            ErrorOnly,
            None,
            "x86_64/amd/zen2",
            Some("213636"),
        ),
        (
            "pr1760-riscv64-327885.md",
            ErrorOnly,
            None,
            "riscv64/generic",
            Some("327885"),
        ),
        (
            "pr1741-zen5-sigma2.md",
            ErrorOnly,
            None,
            "x86_64/amd/zen5",
            Some("2417019"),
        ),
        (
            "pr1732-riscv64-327189.md",
            EmptyTarball,
            None,
            "riscv64/generic",
            Some("327189"),
        ),
        (
            "pr1733-riscv64.md",
            EmptyTarball,
            None,
            "riscv64/generic",
            Some("327185"),
        ),
        (
            "pr1741-zen2-206298.md",
            EmptyTarball,
            None,
            "x86_64/amd/zen2",
            Some("206298"),
        ),
        (
            "pr1873-a64fx.md",
            RealFailure,
            build(BuildFindingClass::Unknown),
            "aarch64/a64fx",
            Some("2152654"),
        ),
        (
            "pr1818-riscv64-329698.md",
            RealFailure,
            build(BuildFindingClass::Unknown),
            "riscv64/generic",
            Some("329698"),
        ),
        (
            "pr1818-riscv64-329809.md",
            RealFailure,
            build(BuildFindingClass::Unknown),
            "riscv64/generic",
            Some("329809"),
        ),
        (
            "pr1823-riscv64-first.md",
            RealFailure,
            build(BuildFindingClass::Unknown),
            "riscv64/generic",
            Some("329610"),
        ),
        (
            "pr1732-riscv64-327102.md",
            Unclassified,
            None,
            "riscv64/generic",
            Some("327102"),
        ),
        (
            "pr1823-riscv64-second.md",
            Unclassified,
            None,
            "riscv64/generic",
            Some("329801"),
        ),
    ];
    for (name, class, sub_class, target, job_id) in cases {
        let job = classify_bot_job(&[input(name)]);
        assert_eq!(job.class, *class, "{name}: {job:?}");
        assert_eq!(job.sub_class, *sub_class, "{name}: {job:?}");
        assert_eq!(job.target.as_deref(), Some(*target), "{name}");
        assert_eq!(job.job_id.as_deref(), *job_id, "{name}");
    }
}

#[test]
fn error_only_noise_carries_the_test_step_error_and_the_tarball() {
    let job = classify_bot_job(&[input("pr1873-zen2-217265.md")]);
    assert_eq!(job.instance.as_deref(), Some("eessi-bot-mc-aws"));
    assert_eq!(job.repository.as_deref(), Some("eessi.io-2025.06-software"));
    assert_eq!(
        job.evidence,
        [
            "found `No missing installations`",
            "eessi-2025.06-software-linux-x86_64-amd-zen2-17931241740.tar.zst: entries: 18368",
            "test result: found message matching ERROR:",
            "test result: EESSI test suite was not run, test step itself failed to execute.",
        ]
    );
}

#[test]
fn a_bot_instance_that_submitted_no_job_is_bot_config_per_command() {
    let jobs = classify_bot_inputs(&[input("pr1741-no-jobs.md")]);
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    for (job, target) in jobs.iter().zip(["x86_64/amd/zen5", "aarch64/nvidia/grace"]) {
        assert_eq!(job.class, BotJobClass::Infra);
        assert_eq!(job.sub_class, infra(InfraCause::BotConfig));
        assert_eq!(job.target.as_deref(), Some(target));
        assert_eq!(job.instance.as_deref(), Some("eessi-bot-sigma2"));
        assert_eq!(job.evidence[1], "no jobs were submitted");
    }
}

#[test]
fn a_pull_request_the_bot_cannot_merge_is_infra() {
    let job = classify_bot_job(&[input("pr1760-merge.md")]);
    assert_eq!(job.class, BotJobClass::Infra);
    assert_eq!(job.sub_class, infra(InfraCause::Merge));
    assert!(job.evidence[1].contains("patch does not apply"), "{job:?}");
}

#[test]
fn the_job_output_gives_a_disk_full_failure_its_class() {
    let jobs = classify_bot_inputs(&[input("pr1818-riscv64-329698.md"), input("slurm-329698.out")]);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let job = &jobs[0];
    assert_eq!(job.class, BotJobClass::RealFailure);
    assert_eq!(job.sub_class, build(BuildFindingClass::Resource));
    assert_eq!(
        job.sources,
        ["pr1818-riscv64-329698.md", "slurm-329698.out"]
    );
    assert!(job
        .evidence
        .contains(&"OSError: [Errno 28] No space left on device".to_string()));
}

#[test]
fn a_result_file_alone_is_the_build_check_and_the_comment_adds_the_test_step() {
    let alone = classify_bot_job(&[input("_bot_job217265.result")]);
    assert_eq!(alone.class, BotJobClass::Success, "{alone:?}");
    assert_eq!(alone.job_id.as_deref(), Some("217265"));
    assert_eq!(alone.target.as_deref(), Some("x86_64/amd/zen2"));
    let jobs = classify_bot_inputs(&[
        input("_bot_job217265.result"),
        input("pr1873-zen2-217265.md"),
    ]);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0].class, BotJobClass::ErrorOnly);
}

/// EasyBuild output in the shape the job output carries it, with the
/// bot's `FAILED: ` marker. The cause comes from the campaign table.
#[test]
fn a_real_failure_takes_its_cause_from_the_campaign_classifier() {
    let cases = [
        (
            "== FAILED: Installation ended unsuccessfully: build failed (first 300 chars): cmd \"make -j 16\" exited with exit code 2\nsrc/fft.c:12:5: error: 'plan' undeclared (first use in this function)",
            BuildFindingClass::Compile,
        ),
        (
            "== FAILED: Installation ended unsuccessfully: Sanity check failed: no file found at 'bin/foo'",
            BuildFindingClass::Sanity,
        ),
        (
            "ERROR: Shell command failed!\nOSError: [Errno 28] No space left on device\n== FAILED: Installation ended unsuccessfully",
            BuildFindingClass::Resource,
        ),
        (
            "tar: libfoo.a: Cannot write: Disk quota exceeded\n== FAILED: Installation ended unsuccessfully",
            BuildFindingClass::Resource,
        ),
    ];
    for (log, class) in cases {
        let job = classify_bot_job(&[text("slurm-1.out", log)]);
        assert_eq!(job.class, BotJobClass::RealFailure, "{log}");
        assert_eq!(job.sub_class, build(class), "{log}");
        assert_eq!(job.job_id.as_deref(), Some("1"));
    }
}

/// The bot's own `ERROR: ` banner names no step, so it is not read as a
/// compiler error.
#[test]
fn the_error_banner_alone_leaves_the_cause_unknown() {
    let job = classify_bot_job(&[text(
        "slurm-2.out",
        "ERROR: Build of /easyconfigs/f/foo/foo-1.0-GCC-15.2.0.eb failed (err: None)\nFAILED: foo/1.0-GCC-15.2.0",
    )]);
    assert_eq!(job.class, BotJobClass::RealFailure);
    assert_eq!(job.sub_class, build(BuildFindingClass::Unknown));
}

#[test]
fn job_output_alone_tells_error_noise_from_an_empty_tarball() {
    let built = "\
>> Creating tarball /tmp/eessi-2025.06-software-linux-x86_64-amd-zen4-1793.tar.zst from /cvmfs...
wrote file list to /tmp/files.list.txt
/tmp/eessi-2025.06-software-linux-x86_64-amd-zen4-1793.tar.zst created!
No missing installations, party time!
ERROR: the test step could not load ReFrame
";
    let job = classify_bot_job(&[text("slurm-3.out", built)]);
    assert_eq!(job.class, BotJobClass::ErrorOnly, "{job:?}");
    assert_eq!(job.target.as_deref(), Some("x86_64/amd/zen4"));

    let empty = built
        .replace("wrote file list to /tmp/files.list.txt\n", "")
        .replace("ERROR: the test step could not load ReFrame\n", "");
    let job = classify_bot_job(&[text("slurm-4.out", &empty)]);
    assert_eq!(job.class, BotJobClass::EmptyTarball, "{job:?}");
}

#[test]
fn infra_signatures_in_the_job_output() {
    let cases = [
        (
            "FATAL:   container creation failed: mount /cvmfs/software.eessi.io: no such device",
            InfraCause::Container,
        ),
        (
            "slurmstepd: error: *** JOB 5 ON node12 CANCELLED AT 2026-10-21T05:02:07 DUE TO NODE FAILURE ***",
            InfraCause::Scheduler,
        ),
        (
            "Failed to initialize root file catalog (16 - file catalog failure)",
            InfraCause::Network,
        ),
        (
            "ERROR: /cvmfs/software.eessi.io is not available!",
            InfraCause::Network,
        ),
        (
            "ERROR: job config file (JOB_CFG_FILE=./cfg/job.cfg) does not exist or not readable",
            InfraCause::BotConfig,
        ),
        (
            "Can't locate File/Temp.pm in @INC (@INC contains: /cvmfs/software.eessi.io/versions/2023.06/compat/linux/x86_64/usr/lib/perl5/5.38)",
            InfraCause::CompatLayer,
        ),
        (
            "ssh: connect to host build-node port 22: Connection refused",
            InfraCause::Network,
        ),
    ];
    for (log, cause) in cases {
        let job = classify_bot_job(&[text("slurm-5.out", log)]);
        assert_eq!(job.class, BotJobClass::Infra, "{log}: {job:?}");
        assert_eq!(job.sub_class, infra(cause), "{log}");
    }
}

#[test]
fn a_missing_job_output_file_is_infra() {
    let comment = "\
New job on instance `eessi-bot-mc-aws` for repository `eessi.io-2025.06-software`
Building for: `x86_64/generic`
|Oct 27 18:04:22 UTC 2026|finished|<details><summary>:cry: FAILURE _(click triangle for details)_</summary><dl><dt>_Details_</dt><dd>:x: no job output file <code>slurm-6.out</code><br/></dd></dl></details>|
";
    let job = classify_bot_job(&[text("comment.md", comment)]);
    assert_eq!(job.class, BotJobClass::Infra);
    assert_eq!(job.sub_class, infra(InfraCause::MissingOutput));
    assert_eq!(job.job_id.as_deref(), Some("6"));
}

#[test]
fn unknown_text_stays_unclassified() {
    for (name, text_in) in [
        ("notes.txt", "the build looked fine to me\n"),
        (
            "slurm-7.out",
            "== building and installing foo/1.0...\n== COMPLETED\n",
        ),
        (
            "comment.md",
            "Updates by the bot instance `eessi-bot-aws`\n- received bot command `status`\n",
        ),
    ] {
        let job = classify_bot_job(&[text(name, text_in)]);
        assert_eq!(job.class, BotJobClass::Unclassified, "{name}: {job:?}");
        assert_eq!(job.sub_class, None, "{name}");
    }
}

#[test]
fn command_classifies_every_fixture_and_counts_the_classes() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bot_classify_fixtures");
    let mut files = std::fs::read_dir(&dir)
        .expect("fixtures")
        .map(|entry| entry.expect("entry").path())
        .collect::<Vec<_>>();
    files.sort();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "bot-classify", "--json"])
        .args(&files)
        .output()
        .expect("run eb-stack");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        report["counts"],
        serde_json::json!({
            "real-failure": 4,
            "error-only": 5,
            "empty-tarball": 3,
            "infra": 3,
            "success": 0,
            "unclassified": 2,
        })
    );
    let disk = report["jobs"]
        .as_array()
        .expect("jobs")
        .iter()
        .find(|job| job["job_id"] == "329698")
        .expect("job 329698");
    assert_eq!(disk["class"], "real-failure");
    assert_eq!(disk["sub_class"], "resource");

    let text = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "bot-classify"])
        .arg(fixture("pr1873-zen2-217265.md"))
        .output()
        .expect("run eb-stack");
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(
        stdout.starts_with("error-only x86_64/amd/zen2 job 217265 (eessi-bot-mc-aws): "),
        "{stdout}"
    );
    assert!(
        stdout.ends_with(
            "real-failure=0 error-only=1 empty-tarball=0 infra=0 success=0 unclassified=0 jobs=1\n"
        ),
        "{stdout}"
    );
}

#[test]
fn a_directory_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "bot-classify"])
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/bot_classify_fixtures"))
        .output()
        .expect("run eb-stack");
    assert_eq!(output.status.code(), Some(2));
}
