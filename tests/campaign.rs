use eb_stack::campaign::{
    classify_build_failure, run_campaign, BuildFindingClass, CampaignRequest, CampaignStatus,
    CAMPAIGN_SCHEMA_VERSION,
};
use eb_stack::target::{
    BuildTarget, EasyBuildWorkload, TargetExecutor, TargetRuntime, TargetTransport,
};
use std::collections::BTreeMap;
use std::process::Command;

fn target(command: &str) -> BuildTarget {
    BuildTarget {
        name: "test-builder".into(),
        transport: TargetTransport::Local,
        executor: TargetExecutor::Direct,
        runtime: TargetRuntime::Host,
        easybuild: EasyBuildWorkload {
            command: command.into(),
            robot_paths: vec!["/tmp".into()],
            work_root: "/tmp".into(),
            tmp_root: "/tmp".into(),
            environment: BTreeMap::new(),
        },
    }
}

#[test]
fn failure_classifier_preserves_the_build_error_domain() {
    let cases = [
        (
            "Checksum verification for source failed",
            BuildFindingClass::Checksum,
        ),
        (
            "CMake Error at CMakeLists.txt:42",
            BuildFindingClass::Configure,
        ),
        (
            "fatal error: hdf5.h: No such file or directory",
            BuildFindingClass::DependencyMissing,
        ),
        ("undefined reference to H5Fopen", BuildFindingClass::Link),
        ("ctest: 3 tests failed", BuildFindingClass::Test),
        (
            "slurmstepd: error: Detected 1 oom-kill",
            BuildFindingClass::Resource,
        ),
        ("ssh: connect to host failed", BuildFindingClass::Transport),
    ];
    for (log, expected) in cases {
        assert_eq!(
            classify_build_failure("build", "", log, Some(1)),
            expected,
            "{log}"
        );
    }
}

#[test]
fn campaign_state_persists_claims_attempts_and_resume() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/e/eOn");
    let locks = bundle.join("locks");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(&locks).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"eOn","version":"2.16.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        locks.join("default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    std::fs::write(
        recipes.join("eOn-2.16.0-foss-2026.1.eb"),
        "name = 'eOn'\nversion = '2.16.0'\n",
    )
    .expect("recipe");
    let state_path = temp.path().join("campaign.json");

    let failed = run_campaign(&CampaignRequest {
        bundle: bundle.clone(),
        target: target("false"),
        state_path: state_path.clone(),
    })
    .expect("failed campaign state");
    assert_eq!(failed.schema_version, CAMPAIGN_SCHEMA_VERSION);
    assert_eq!(failed.status, CampaignStatus::Failed);
    assert!(failed.claims.resolves);
    assert!(!failed.claims.builds);
    assert_eq!(failed.attempts, 1);
    assert_eq!(failed.findings.len(), 1);
    assert!(state_path.is_file());

    let completed = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: state_path.clone(),
    })
    .expect("resumed campaign");
    assert_eq!(completed.status, CampaignStatus::Completed);
    assert!(completed.claims.resolves);
    assert!(completed.claims.builds);
    assert!(!completed.claims.binary_verified);
    assert_eq!(completed.attempts, 2);
    assert_eq!(completed.findings.len(), 1, "failure history is retained");

    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state_path).expect("read state"))
            .expect("state JSON");
    assert_eq!(persisted["status"], "completed");
    assert_eq!(persisted["claims"]["builds"], true);
}

#[test]
fn campaign_cli_runs_a_named_target_and_reports_status() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/e/eOn");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"eOn","version":"2.16.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    std::fs::write(
        recipes.join("eOn-2.16.0-foss-2026.1.eb"),
        "name = 'eOn'\nversion = '2.16.0'\n",
    )
    .expect("recipe");
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        r#"
schema_version = 1
[[targets]]
name = "test-builder"
[targets.transport]
kind = "local"
[targets.executor]
kind = "direct"
[targets.runtime]
kind = "host"
[targets.easybuild]
command = "true"
robot_paths = ["/tmp"]
work_root = "/tmp"
tmp_root = "/tmp"
"#,
    )
    .expect("config");
    let state = temp.path().join("campaign.json");
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let run = Command::new(binary)
        .args([
            "campaign",
            "run",
            "--bundle",
            bundle.to_str().unwrap(),
            "--config",
            config.to_str().unwrap(),
            "--target",
            "test-builder",
            "--state",
            state.to_str().unwrap(),
        ])
        .output()
        .expect("campaign run");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(state.is_file());

    let status = Command::new(binary)
        .args(["campaign", "status", "--state", state.to_str().unwrap()])
        .output()
        .expect("campaign status");
    assert!(status.status.success());
    let body: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert_eq!(body["status"], "completed");
    assert_eq!(body["claims"]["builds"], true);
}

#[test]
fn staging_failure_is_persisted_as_a_transport_finding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/e/eOn");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"eOn","version":"2.16.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    std::fs::write(recipes.join("eOn.eb"), "name = 'eOn'\n").expect("recipe");
    let mut target = target("true");
    target.transport = TargetTransport::Ssh {
        host: "unused.example.org".into(),
        port: None,
        command: "true".into(),
        sync_command: "false".into(),
    };
    let state_path = temp.path().join("campaign.json");
    let state = run_campaign(&CampaignRequest {
        bundle,
        target,
        state_path: state_path.clone(),
    })
    .expect("staging failure state");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert_eq!(state.findings.len(), 1);
    assert_eq!(state.findings[0].class, BuildFindingClass::Transport);
    assert_eq!(state.findings[0].stage, "stage");
    assert!(state_path.is_file());
}
