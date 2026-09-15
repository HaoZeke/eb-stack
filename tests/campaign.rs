use eb_stack::campaign::{
    claim_finding, classify_build_failure, failure_signature, is_stuck_on_signature,
    resolve_finding, run_campaign, BuildFinding, BuildFindingClass, CampaignRequest, CampaignState,
    CampaignStatus, ClaimLadder, FindingDisposition, FindingResolution, FindingStatus,
    CAMPAIGN_SCHEMA_VERSION, MAX_ATTEMPTS_PER_SIGNATURE,
};
use eb_stack::target::{
    BuildTarget, EasyBuildWorkload, TargetExecutor, TargetRuntime, TargetTransport,
};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
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

fn write_valid_recipe(path: &Path, name: &str, version: &str) {
    std::fs::write(
        path,
        format!(
            "name = '{name}'\nversion = '{version}'\ntoolchain = SYSTEM\nsources = ['source.tar.gz']\nchecksums = ['deadbeef']\nmoduleclass = 'tools'\n"
        ),
    )
    .expect("recipe");
}

#[test]
fn failure_classifier_preserves_the_build_error_domain() {
    let cases = [
        (
            "Checksum verification for source failed",
            BuildFindingClass::Checksum,
        ),
        (
            "Couldn't find file rustc-1.78.0-src.tar.gz anywhere, and downloading it didn't work either",
            BuildFindingClass::Source,
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
        (
            "g++: fatal error: Killed signal terminated program cc1plus\ncompilation terminated.",
            BuildFindingClass::Resource,
        ),
        ("ssh: connect to host failed", BuildFindingClass::Transport),
        (
            "Failed to download file from https://example.invalid/foo.tar.gz\nURLError: <urlopen error [Errno 110] Connection timed out>\nERROR: installation failed",
            BuildFindingClass::Source,
        ),
        (
            "Couldn't find file foo.tar.gz anywhere, and downloading it didn't work either\nConnection refused",
            BuildFindingClass::Source,
        ),
        (
            "flex: /lib64/libc.so.6: version `GLIBC_2.38' not found",
            BuildFindingClass::Runtime,
        ),
        (
            "CMake Error: could not extract libs\nerror: could not execute process `sccache rustc -vV` (never executed)\nCaused by: No such file or directory",
            BuildFindingClass::Runtime,
        ),
        (
            "checksums = ['abc']\nmake[2]: *** [Makefile:42: all] Error 2\nInstallation failed",
            BuildFindingClass::Compile,
        ),
        (
            "Checksum verification for source successful\nmake[2]: *** [Makefile:42: all] Error 2\nInstallation failed",
            BuildFindingClass::Compile,
        ),
        (
            "patch dependency is installed\nCMake Error at CMakeLists.txt:42\nInstallation failed",
            BuildFindingClass::Configure,
        ),
        (
            "patching file src/lib.rs\nHunk #1 FAILED at 42",
            BuildFindingClass::Patch,
        ),
        (
            "fatal error: hdf5.h: No such file or directory\nmodule env: PATH=/usr/bin",
            BuildFindingClass::DependencyMissing,
        ),
        (
            "env: rustc: No such file or directory",
            BuildFindingClass::Runtime,
        ),
        (
            "ERROR: installation failed",
            BuildFindingClass::Install,
        ),
        (
            "error: undeclared identifier foo\nERROR: installation failed",
            BuildFindingClass::Compile,
        ),
        (
            "==> Error: The following packages failed to install:\nERROR: installation failed",
            BuildFindingClass::Install,
        ),
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
fn campaign_interns_easybuild_command_output_before_classifying() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");

    let nested_log = temp.path().join("easybuild-command.out");
    std::fs::write(
        &nested_log,
        "flex: /lib64/libc.so.6: version `GLIBC_2.38' not found\n",
    )
    .expect("nested log");
    let command = temp.path().join("fake-eb");
    std::fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '    full command              ->  make -j 2'\nprintf '%s\\n' '    working directory         ->  /tmp/eb-build'\nprintf '%s\\n' '    called from               ->  build_step'\nprintf '%s\\n' '    output (stdout + stderr)  ->  {}'\nprintf '%s\\n' '    interactive shell script  ->  /tmp/eb-build/cmd.sh'\nprintf '%s\\n' 'ERROR: installation failed'\nexit 1\n",
            nested_log.display()
        ),
    )
    .expect("command");
    let mut permissions = std::fs::metadata(&command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&command, permissions).expect("permissions");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target(command.to_str().expect("command path")),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign finding");
    assert_eq!(state.findings[0].class, BuildFindingClass::Runtime);
    assert!(state.findings[0].evidence.contains("GLIBC_2.38"));
    assert!(
        state.findings[0]
            .signature
            .as_deref()
            .is_some_and(|signature| signature.contains("GLIBC")),
        "{:?}",
        state.findings[0].signature
    );
    assert!(state.findings[0]
        .evidence
        .contains(&nested_log.display().to_string()));
    assert!(state.findings[0]
        .evidence
        .contains("full command: make -j 2"));
    assert!(state.findings[0]
        .evidence
        .contains("working directory: /tmp/eb-build"));
    assert!(state.findings[0]
        .evidence
        .contains("interactive shell script: /tmp/eb-build/cmd.sh"));
}

#[test]
fn campaign_interns_out_txt_beside_cmd_sh_when_the_banner_omits_stdout() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");

    let cmd_dir = temp.path().join("run-shell-cmd-output/make-xyz");
    std::fs::create_dir_all(&cmd_dir).expect("cmd dir");
    std::fs::write(
        cmd_dir.join("out.txt"),
        "flex: /lib64/libc.so.6: version `GLIBC_2.38' not found\n",
    )
    .expect("out.txt");
    std::fs::write(cmd_dir.join("cmd.sh"), "#!/bin/sh\n").expect("cmd.sh");
    let command = temp.path().join("fake-eb");
    std::fs::write(
        &command,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '    interactive shell script  ->  {}'\nprintf '%s\\n' 'ERROR: installation failed'\nexit 1\n",
            cmd_dir.join("cmd.sh").display()
        ),
    )
    .expect("command");
    let mut permissions = std::fs::metadata(&command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&command, permissions).expect("permissions");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target(command.to_str().expect("command path")),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign finding");
    assert_eq!(state.findings[0].class, BuildFindingClass::Runtime);
    assert!(state.findings[0].evidence.contains("GLIBC_2.38"));
}

#[test]
fn campaign_rejects_missing_checksums_before_easybuild() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/q/QMCPACK");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"QMCPACK","version":"4.3.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    std::fs::write(
        recipes.join("QMCPACK.eb"),
        "name = 'QMCPACK'\nversion = '4.3.0'\ntoolchain = SYSTEM\nchecksums = []\nmoduleclass = 'chem'\n",
    )
    .expect("recipe");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign finding");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert!(!state.claims.builds);
    assert_eq!(state.findings[0].class, BuildFindingClass::Checksum);
    assert_eq!(state.findings[0].stage, "preflight");
    assert!(state.findings[0].evidence.contains("missing checksums"));
}

fn missing_checksum_bundle(root: &Path) -> std::path::PathBuf {
    let bundle = root.join("bundle");
    let recipes = bundle.join("easyconfigs/q/QMCPACK");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"QMCPACK","version":"4.3.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    std::fs::write(
        recipes.join("QMCPACK.eb"),
        "name = 'QMCPACK'\nversion = '4.3.0'\ntoolchain = SYSTEM\nchecksums = []\nmoduleclass = 'chem'\n",
    )
    .expect("recipe");
    bundle
}

#[test]
fn preflight_repeats_record_stuck_on_signature() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = missing_checksum_bundle(temp.path());
    let state_path = temp.path().join("campaign.json");
    let request = CampaignRequest {
        bundle: bundle.clone(),
        target: target("true"),
        state_path: state_path.clone(),
    };
    run_campaign(&request).expect("first preflight");
    let second = run_campaign(&request).expect("second preflight");
    assert!(
        second
            .history
            .iter()
            .any(|event| event.detail.contains("stuck on signature after")),
        "{:?}",
        second.history
    );
}

#[test]
fn successful_retry_supersedes_preflight() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = missing_checksum_bundle(temp.path());
    let state_path = temp.path().join("campaign.json");
    run_campaign(&CampaignRequest {
        bundle: bundle.clone(),
        target: target("true"),
        state_path: state_path.clone(),
    })
    .expect("preflight");
    write_valid_recipe(
        &bundle.join("easyconfigs/q/QMCPACK/QMCPACK.eb"),
        "QMCPACK",
        "4.3.0",
    );
    let completed = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path,
    })
    .expect("retry");
    assert_eq!(completed.status, CampaignStatus::Completed);
    assert!(completed.claims.builds);
    assert!(
        completed
            .findings
            .iter()
            .any(|finding| finding.stage == "preflight"
                && finding.status == FindingStatus::Superseded),
        "{:?}",
        completed.findings
    );
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
    write_valid_recipe(&recipes.join("eOn-2.16.0-foss-2026.1.eb"), "eOn", "2.16.0");
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
    assert_eq!(completed.findings[0].status, FindingStatus::Superseded);

    let persisted: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state_path).expect("read state"))
            .expect("state JSON");
    assert_eq!(persisted["status"], "completed");
    assert_eq!(persisted["claims"]["builds"], true);
}

#[test]
fn campaign_resume_records_an_abandoned_running_attempt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipe = bundle.join("easyconfigs/a/Alpha/Alpha.eb");
    std::fs::create_dir_all(recipe.parent().unwrap()).expect("recipe directory");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"Alpha","version":"1.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    write_valid_recipe(&recipe, "Alpha", "1.0");

    let state_path = temp.path().join("campaign.json");
    let interrupted_recipe = "easyconfigs/a/Alpha/Alpha.eb";
    let interrupted = CampaignState {
        schema_version: CAMPAIGN_SCHEMA_VERSION,
        package: "Alpha".into(),
        version: "1.0".into(),
        bundle: bundle.display().to_string(),
        target: "test-builder".into(),
        status: CampaignStatus::Running,
        attempts: 3,
        claims: ClaimLadder {
            resolves: true,
            builds: false,
            binary_verified: false,
        },
        current_recipe: Some(interrupted_recipe.into()),
        findings: Vec::new(),
        history: Vec::new(),
        lock_identity: None,
        lock_epoch_start_attempt: 0,
    };
    std::fs::write(
        &state_path,
        serde_json::to_string_pretty(&interrupted).expect("state JSON"),
    )
    .expect("interrupted state");

    let resumed = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path,
    })
    .expect("resumed campaign");

    assert_eq!(resumed.status, CampaignStatus::Completed);
    assert_eq!(resumed.attempts, 4);
    assert_eq!(resumed.findings.len(), 1);
    let finding = &resumed.findings[0];
    assert_eq!(finding.attempt, 3);
    assert_eq!(finding.class, BuildFindingClass::Interrupted);
    assert_eq!(finding.disposition, FindingDisposition::Retryable);
    assert_eq!(finding.status, FindingStatus::Superseded);
    assert_eq!(finding.recipe, interrupted_recipe);
    assert!(finding.resolution.as_ref().is_some_and(|resolution| {
        resolution.action == "resumed campaign after controller interruption"
    }));
}

#[test]
fn campaign_uses_declared_closure_build_order() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let root_recipe = bundle.join("easyconfigs/a/Alpha/Alpha.eb");
    let companion_recipe = bundle.join("easyconfigs/z/Zulu/Zulu.eb");
    std::fs::create_dir_all(root_recipe.parent().unwrap()).expect("root recipe directory");
    std::fs::create_dir_all(companion_recipe.parent().unwrap())
        .expect("companion recipe directory");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"Alpha","version":"1.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    write_valid_recipe(&root_recipe, "Alpha", "1.0");
    write_valid_recipe(&companion_recipe, "Zulu", "2.0");
    std::fs::write(
        bundle.join("build-order.json"),
        r#"{
  "schema_version": 1,
  "recipes": [
    "easyconfigs/z/Zulu/Zulu.eb",
    "easyconfigs/a/Alpha/Alpha.eb"
  ]
}"#,
    )
    .expect("build order");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign");
    let built = state
        .history
        .iter()
        .filter(|event| event.detail == "EasyBuild command succeeded")
        .filter_map(|event| event.recipe.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(
        built,
        ["easyconfigs/z/Zulu/Zulu.eb", "easyconfigs/a/Alpha/Alpha.eb"]
    );
}

#[test]
fn campaign_adds_the_staged_bundle_overlay_to_robot_paths() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipe = bundle.join("easyconfigs/a/Alpha/Alpha.eb");
    std::fs::create_dir_all(recipe.parent().unwrap()).expect("recipe directory");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{"package":{"name":"Alpha","version":"1.0"}}"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    write_valid_recipe(&recipe, "Alpha", "1.0");

    let state = run_campaign(&CampaignRequest {
        bundle: bundle.clone(),
        target: target("false"),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign");
    let finding = state.findings.last().expect("build finding");
    let expected = format!("--robot={}:/tmp", bundle.join("easyconfigs").display());
    assert!(
        finding
            .command
            .args
            .iter()
            .any(|argument| argument == &expected),
        "expected {expected:?} in {:?}",
        finding.command
    );
}

#[test]
fn finding_queue_enforces_ownership_and_records_resolution() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let state_path = temp.path().join("campaign.json");
    let failed = run_campaign(&CampaignRequest {
        bundle,
        target: target("false"),
        state_path: state_path.clone(),
    })
    .expect("failed campaign");
    let finding = failed.findings[0].id.clone();

    let claimed = claim_finding(&state_path, &finding, "omp-worker-1").expect("claim finding");
    assert_eq!(claimed.findings[0].status, FindingStatus::InProgress);
    assert_eq!(claimed.findings[0].owner.as_deref(), Some("omp-worker-1"));
    let conflict = claim_finding(&state_path, &finding, "omp-worker-2")
        .expect_err("second worker cannot steal finding");
    assert!(conflict.to_string().contains("omp-worker-1"), "{conflict}");

    let resolved = resolve_finding(
        &state_path,
        &finding,
        "omp-worker-1",
        FindingResolution {
            action: "corrected the package config".into(),
            evidence: "recipe check exits successfully".into(),
            changes: vec!["packages/eon.toml".into()],
        },
    )
    .expect("resolve finding");
    assert_eq!(resolved.findings[0].status, FindingStatus::Resolved);
    assert_eq!(
        resolved.findings[0]
            .resolution
            .as_ref()
            .map(|resolution| resolution.action.as_str()),
        Some("corrected the package config")
    );
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
    write_valid_recipe(&recipes.join("eOn-2.16.0-foss-2026.1.eb"), "eOn", "2.16.0");
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
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

#[test]
fn executor_spawn_failure_is_persisted_as_a_typed_finding() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let mut build_target = target("true");
    build_target.executor = TargetExecutor::Slurm {
        command: "/definitely/missing/eb-stack-srun".into(),
        partition: None,
        account: None,
        cpus: None,
        memory: None,
        time: None,
        gres: None,
    };
    let state_path = temp.path().join("campaign.json");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: build_target,
        state_path: state_path.clone(),
    })
    .expect("spawn failure is campaign evidence");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert_eq!(state.findings.len(), 1);
    assert_eq!(state.findings[0].class, BuildFindingClass::Executor);
    assert_eq!(state.findings[0].stage, "build");
    assert_eq!(state.findings[0].exit_code, None);
    assert!(state.findings[0].evidence.contains("eb-stack-srun"));
    assert!(state_path.is_file());
}

#[test]
fn missing_easybuild_program_is_persisted_as_a_runtime_finding() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let state_path = temp.path().join("campaign.json");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target("/definitely/missing/eb-stack-easybuild"),
        state_path: state_path.clone(),
    })
    .expect("missing workload command is campaign evidence");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert_eq!(state.findings.len(), 1);
    assert_eq!(state.findings[0].class, BuildFindingClass::Runtime);
    assert_eq!(state.findings[0].stage, "build");
    assert_eq!(state.findings[0].exit_code, Some(127));
    assert!(state.findings[0].evidence.contains("eb-stack-easybuild"));
    assert!(state_path.is_file());
}

#[test]
fn campaign_runs_profile_verification_and_sets_the_binary_claim() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/e/eOn");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{
          "package":{"name":"eOn","version":"2.16.0"},
          "build":{"toolchain":{"name":"foss","version":"2026.1"}},
          "profiles":[{
            "name":"default",
            "default":true,
            "versionsuffix":[],
            "verification_commands":[{"program":"true","args":[]}]
          }]
        }"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign");
    assert_eq!(state.status, CampaignStatus::Completed);
    assert!(state.claims.builds);
    assert!(state.claims.binary_verified);
    assert!(state
        .history
        .iter()
        .any(|event| event.detail.contains("binary verification succeeded")));
}

#[test]
fn failed_profile_verification_preserves_the_build_claim_and_finding() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/q/QMCPACK");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{
          "package":{"name":"QMCPACK","version":"4.3.0"},
          "build":{"toolchain":{"name":"foss","version":"2026.1"}},
          "profiles":[{
            "name":"complex",
            "default":false,
            "versionsuffix":["-complex"],
            "verification_commands":[{"program":"false","args":[]}]
          }]
        }"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/complex.lock.json"),
        r#"{"profile":"complex","solver":"resolvo"}"#,
    )
    .expect("lock");
    write_valid_recipe(&recipes.join("QMCPACK.eb"), "QMCPACK", "4.3.0");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert!(state.claims.builds);
    assert!(!state.claims.binary_verified);
    assert_eq!(state.findings.len(), 1);
    assert_eq!(state.findings[0].stage, "verify");
    assert_eq!(state.findings[0].class, BuildFindingClass::Sanity);
    assert!(state.findings[0]
        .evidence
        .contains("module=QMCPACK/4.3.0-foss-2026.1-complex"));
}

#[test]
fn system_verification_loads_the_unsuffixed_module() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipes = bundle.join("easyconfigs/q/QMCPACK");
    std::fs::create_dir_all(&recipes).expect("recipes");
    std::fs::create_dir_all(bundle.join("locks")).expect("locks");
    std::fs::write(
        bundle.join("package.plan.json"),
        r#"{
          "package":{"name":"QMCPACK","version":"4.3.0"},
          "build":{"toolchain":{"name":"system","version":"system"}},
          "profiles":[{
            "name":"default",
            "default":true,
            "versionsuffix":[],
            "verification_commands":[{"program":"false","args":[]}]
          }]
        }"#,
    )
    .expect("manifest");
    std::fs::write(
        bundle.join("locks/default.lock.json"),
        r#"{"profile":"default","solver":"resolvo"}"#,
    )
    .expect("lock");
    write_valid_recipe(&recipes.join("QMCPACK.eb"), "QMCPACK", "4.3.0");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert!(state.findings[0].evidence.contains("module=QMCPACK/4.3.0"));
    assert!(
        !state.findings[0]
            .evidence
            .contains("module=QMCPACK/4.3.0-system-system"),
        "{}",
        state.findings[0].evidence
    );
}

#[test]
fn campaign_cli_claims_and_resolves_findings_for_omp_workers() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let state_path = temp.path().join("campaign.json");
    let failed = run_campaign(&CampaignRequest {
        bundle,
        target: target("false"),
        state_path: state_path.clone(),
    })
    .expect("failed campaign");
    let finding = &failed.findings[0].id;
    let binary = env!("CARGO_BIN_EXE_eb-stack");

    let claim = Command::new(binary)
        .args([
            "campaign",
            "finding",
            "claim",
            "--state",
            state_path.to_str().unwrap(),
            "--id",
            finding,
            "--owner",
            "omp-worker-1",
        ])
        .output()
        .expect("claim command");
    assert!(
        claim.status.success(),
        "{}",
        String::from_utf8_lossy(&claim.stderr)
    );

    let resolve = Command::new(binary)
        .args([
            "campaign",
            "finding",
            "resolve",
            "--state",
            state_path.to_str().unwrap(),
            "--id",
            finding,
            "--owner",
            "omp-worker-1",
            "--action",
            "corrected package config",
            "--evidence",
            "recipe check exits successfully",
            "--change",
            "packages/eon.toml",
        ])
        .output()
        .expect("resolve command");
    assert!(
        resolve.status.success(),
        "{}",
        String::from_utf8_lossy(&resolve.stderr)
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state_path).expect("state"))
            .expect("state JSON");
    assert_eq!(state["findings"][0]["status"], "resolved");
    assert_eq!(state["findings"][0]["owner"], "omp-worker-1");
}

#[test]
fn failure_signature_prefers_the_causal_line_to_the_install_footer() {
    assert_eq!(
        failure_signature(
            "/usr/bin/llvm-config: No such file or directory\nERROR: installation failed\n"
        ),
        "/usr/bin/llvm-config: No such file or directory"
    );
    assert_eq!(
        failure_signature(
            "1 out of 3 hunks FAILED -- saving rejects to file package.py.rej\nERROR: installation failed\n"
        ),
        "1 out of 3 hunks FAILED -- saving rejects to file package.py.rej"
    );
    assert_eq!(
        failure_signature("ERROR: installation failed\n"),
        "verification failed without a recognized error"
    );
    assert_eq!(
        failure_signature("FAILED: Installation ended unsuccessfully\n"),
        "verification failed without a recognized error"
    );
}

#[test]
fn failure_signature_prefers_interned_easybuild_output_to_the_v5_footer() {
    let evidence = "\
FAILED: Installation ended unsuccessfully: shell command 'make ...' failed with exit code 2
ERROR: installation failed
EasyBuild command output /tmp/eb-xxx/out.txt:
error: undeclared identifier foo
";
    assert_eq!(
        failure_signature(evidence),
        "error: undeclared identifier foo"
    );
}

#[test]
fn failure_signature_prefers_interned_body_without_error_to_the_v5_banner() {
    let glibc = "\
ERROR: Shell command failed!
EasyBuild command output /tmp/eb-xxx/out.txt:
flex: /lib64/libc.so.6: version `GLIBC_2.38' not found
";
    let make = "\
ERROR: Shell command failed!
EasyBuild command output /tmp/eb-xxx/out.txt:
make: *** [foo.o] Error 2
";
    let glibc_signature = failure_signature(glibc);
    let make_signature = failure_signature(make);
    assert!(
        !glibc_signature.eq_ignore_ascii_case("ERROR: Shell command failed!"),
        "{glibc_signature}"
    );
    assert!(
        !glibc_signature.contains("Shell command failed"),
        "{glibc_signature}"
    );
    assert!(glibc_signature.contains("GLIBC"), "{glibc_signature}");
    assert_ne!(glibc_signature, make_signature);
    assert!(make_signature.contains("***"), "{make_signature}");
}

#[test]
fn failure_signature_strips_tmp_paths_hashes_and_positions() {
    let first = failure_signature(
        "/tmp/eb-aaa/work/src/foo.c:12:3: error: undeclared identifier deadbeef\nERROR: installation failed\n",
    );
    let second = failure_signature(
        "/tmp/eb-bbb/work/src/foo.c:44:9: error: undeclared identifier cafebabe\nERROR: installation failed\n",
    );
    assert_eq!(first, second);
    assert_eq!(first, "<tmp> error: undeclared identifier <hash>");
}

#[test]
fn failure_signature_collapses_site_tmp_roots_and_eb_hash_paths() {
    let work_a = failure_signature(
        "/work/eb-aaa/src/foo.c:12:3: error: undeclared identifier deadbeef\nERROR: installation failed\n",
    );
    let work_b = failure_signature(
        "/work/eb-bbb/src/foo.c:44:9: error: undeclared identifier cafebabe\nERROR: installation failed\n",
    );
    assert_eq!(work_a, work_b);
    assert_eq!(work_a, "<tmp> error: undeclared identifier <hash>");

    let scratch = failure_signature(
        "/scratch/eb-aaa/src/foo.c:12:3: error: undeclared identifier deadbeef\n",
    );
    let shm = failure_signature(
        "/dev/shm/eb-bbb/src/foo.c:44:9: error: undeclared identifier cafebabe\n",
    );
    let hashed =
        failure_signature("/var/eb-aaa/src/foo.c:12:3: error: undeclared identifier deadbeef\n");
    assert_eq!(scratch, work_a);
    assert_eq!(shm, work_a);
    assert_eq!(hashed, work_a);
}

#[test]
fn distinct_cmake_messages_do_not_share_a_signature() {
    let taskflow = failure_signature(
        "CMake Error at CMakeLists.txt:100 (message):\n\n  Taskflow currently supports the following compilers:\n\nERROR: installation failed\n",
    );
    let cgal = failure_signature(
        "CMake Error at cmake/macros/macro_configure_feature.cmake:111 (message):\n\n  Could not find the cgal library!\n\nERROR: installation failed\n",
    );
    assert_eq!(
        taskflow,
        "CMake Error: Taskflow currently supports the following compilers:"
    );
    assert_eq!(cgal, "CMake Error: Could not find the cgal library!");
    assert_ne!(taskflow, cgal);
}

#[test]
fn two_identical_signatures_are_stuck() {
    let signature = failure_signature(
        "/tmp/eb-aaa/foo.c:12:3: error: undeclared identifier deadbeef\nERROR: installation failed\n",
    );
    let finding = |attempt: u32| BuildFinding {
        id: format!("attempt:{attempt}:finding:1"),
        class: BuildFindingClass::Compile,
        disposition: FindingDisposition::RequiresJudgment,
        stage: "build".into(),
        recipe: "easyconfigs/e/eOn/eOn.eb".into(),
        target: "test-builder".into(),
        summary: "compile".into(),
        evidence: String::new(),
        command: eb_stack::target::CommandPlan {
            program: "eb".into(),
            args: vec![],
        },
        exit_code: Some(1),
        attempt,
        signature: Some(signature.clone()),
        status: FindingStatus::Open,
        owner: None,
        resolution: None,
    };
    let mut state = CampaignState {
        schema_version: CAMPAIGN_SCHEMA_VERSION,
        package: "eOn".into(),
        version: "2.16.0".into(),
        bundle: "bundle".into(),
        target: "test-builder".into(),
        status: CampaignStatus::Failed,
        attempts: 1,
        claims: ClaimLadder {
            resolves: true,
            builds: false,
            binary_verified: false,
        },
        current_recipe: None,
        findings: vec![finding(1)],
        history: Vec::new(),
        lock_identity: None,
        lock_epoch_start_attempt: 0,
    };
    assert_eq!(MAX_ATTEMPTS_PER_SIGNATURE, 2);
    assert!(!is_stuck_on_signature(&state, &signature));
    state.findings.push(finding(2));
    assert!(is_stuck_on_signature(&state, &signature));
}

#[test]
fn campaign_records_a_signature_and_stuck_history_on_repeat() {
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
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let command = temp.path().join("fake-eb");
    std::fs::write(
        &command,
        "#!/bin/sh\nprintf '%s\\n' 'error: undeclared identifier foo'\nprintf '%s\\n' 'ERROR: installation failed'\nexit 1\n",
    )
    .expect("command");
    let mut permissions = std::fs::metadata(&command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&command, permissions).expect("permissions");
    let request = CampaignRequest {
        bundle,
        target: target(command.to_str().expect("command path")),
        state_path: temp.path().join("campaign.json"),
    };
    let first = run_campaign(&request).expect("first attempt");
    assert_eq!(first.findings[0].class, BuildFindingClass::Compile);
    assert_eq!(
        first.findings[0].signature.as_deref(),
        Some("error: undeclared identifier foo")
    );
    assert!(first
        .history
        .iter()
        .all(|event| !event.detail.contains("stuck on signature")));
    let second = run_campaign(&request).expect("second attempt");
    assert!(
        second.history.iter().any(|event| event
            .detail
            .contains("stuck on signature after 2 identical failures")),
        "{:?}",
        second.history
    );
}

#[test]
fn campaign_records_a_lock_identity_change_on_resume() {
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
    let lock = bundle.join("locks/default.lock.json");
    std::fs::write(&lock, r#"{"profile":"default","solver":"resolvo"}"#).expect("lock");
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let request = CampaignRequest {
        bundle: bundle.clone(),
        target: target("true"),
        state_path: temp.path().join("campaign.json"),
    };
    let first = run_campaign(&request).expect("first campaign");
    assert_eq!(first.status, CampaignStatus::Completed);
    let original = first.lock_identity.clone().expect("lock identity");
    std::fs::write(&lock, r#"{"profile":"default","solver":"resolvo","x":1}"#).expect("rewrite");
    let second = run_campaign(&request).expect("resumed campaign");
    let changed = second.lock_identity.clone().expect("new lock identity");
    assert_ne!(original, changed);
    assert!(
        second
            .history
            .iter()
            .any(|event| event.detail.contains("lock identity changed")),
        "{:?}",
        second.history
    );
}

#[test]
fn lock_identity_change_does_not_count_prior_signatures_as_stuck() {
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
    let lock = bundle.join("locks/default.lock.json");
    std::fs::write(&lock, r#"{"profile":"default","solver":"resolvo"}"#).expect("lock");
    write_valid_recipe(&recipes.join("eOn.eb"), "eOn", "2.16.0");
    let command = temp.path().join("fake-eb");
    std::fs::write(
        &command,
        "#!/bin/sh\nprintf '%s\\n' 'error: undeclared identifier foo'\nprintf '%s\\n' 'ERROR: installation failed'\nexit 1\n",
    )
    .expect("command");
    let mut permissions = std::fs::metadata(&command).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&command, permissions).expect("permissions");
    let request = CampaignRequest {
        bundle,
        target: target(command.to_str().expect("command path")),
        state_path: temp.path().join("campaign.json"),
    };
    let first = run_campaign(&request).expect("first attempt");
    assert!(first
        .history
        .iter()
        .all(|event| !event.detail.contains("stuck on signature")));
    let second = run_campaign(&request).expect("second attempt");
    assert!(
        second.history.iter().any(|event| event
            .detail
            .contains("stuck on signature after 2 identical failures")),
        "{:?}",
        second.history
    );
    std::fs::write(&lock, r#"{"profile":"default","solver":"resolvo","x":1}"#).expect("rewrite");
    let third = run_campaign(&request).expect("first post-replan attempt");
    assert!(
        third
            .history
            .iter()
            .any(|event| event.detail.contains("lock identity changed")),
        "{:?}",
        third.history
    );
    assert!(
        third
            .history
            .iter()
            .filter(|event| event.attempt == third.attempts)
            .all(|event| !event.detail.contains("stuck on signature")),
        "{:?}",
        third.history
    );
    assert_eq!(
        third
            .findings
            .last()
            .and_then(|finding| finding.signature.as_deref()),
        Some("error: undeclared identifier foo")
    );
}
