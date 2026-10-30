use eb_stack::campaign::{
    claim_finding, classify_build_failure, resolve_finding, run_campaign, BuildFindingClass,
    CampaignRequest, CampaignState, CampaignStatus, ClaimLadder, FindingDisposition,
    FindingResolution, FindingStatus, CAMPAIGN_SCHEMA_VERSION,
};
use eb_stack::target::{
    BuildTarget, EasyBuildWorkload, TargetExecutor, TargetRuntime, TargetTransport,
};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

include!("support/write_executable.rs");

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
    ];
    for (log, expected) in cases {
        assert_eq!(
            classify_build_failure("build", "", log, Some(1)),
            expected,
            "{log}"
        );
    }
}

/// EasyBuild 5.4.1 output recorded from EESSI 2026.06 builds, paths
/// shortened. The robot stops before building anything.
const MISSING_DEPENDENCY_OUTPUT: &str = "\
== found valid index for
/cvmfs/software.eessi.io/versions/2026.06/software/linux/x86_64/amd/zen5/softwar
e/EasyBuild/5.4.1/easybuild/easyconfigs, so using it...
== Running parse hook for FFTW-3.3.10-GCC-15.2.0.eb...


ERROR: Missing dependencies: Boost/1.92.0-gfbf-2026.1 (no easyconfig file or existing module found)

== resolving dependencies ...
== eb rc=31
";

/// A CUDA dependency refused at its fetch step, wrapped at 80 columns.
const EULA_OUTPUT: &str = "\
== processing EasyBuild easyconfig
/build/easyconfigs/c/CUDA/CUDA
-13.3.0.eb
== building and installing CUDA/13.3.0...
  >> installation prefix:
/build/software/CUDA/13.3.0
== fetching files and verifying checksums...
== Running pre-fetch hook...
== ... (took < 1 sec)
== FAILED: Installation ended unsuccessfully: The End User License Agreement
(EULA) for CUDA is currently not accepted!
(see https://docs.nvidia.com/cuda/eula/index.html for more information)
You should either:
- add --accept-eula-for=CUDA to the 'eb' command;
- update your EasyBuild configuration to always accept the EULA for CUDA;
- add 'accept_eula = True' to the easyconfig file you are using;
 (took 0 secs)
== Results of the build can be found in the log file(s)
/build/tmp/easybuild-CUDA-13.3.0-20261029.202708.tDYAy.log
== Running post-easyblock hook...
== Build succeeded for 0 out of 3 (total: 2 secs)
== Summary:
   * [FAILED]  CUDA/13.3.0
   * [FAILED]  UCX-CUDA/1.20.0-GCCcore-15.2.0-CUDA-13.3.0
   * [FAILED]  NCCL/2.32.3-GCCcore-15.2.0-CUDA-13.3.0
== eb rc=1
";

/// A pip install that stopped in the install step; the `ERROR:` banner is
/// EasyBuild's for any failed step.
const INSTALL_STEP_OUTPUT: &str = "\
== testing...
== Running pre-test hook...
== ... (took < 1 sec)
== installing...


ERROR: Shell command failed!
    full command              ->
/software/Python/3.14.2-GCCcore-15.2.0/bin/python -m pip install
--prefix=/build/software/PuLP/4.2.0-foss-2026.1  --verbose --no-deps
--ignore-installed --no-index --no-build-isolation .
    exit code                 ->  2
== ... (took 1 secs)
== FAILED: Installation ended unsuccessfully: shell command 'python ...' failed
with exit code 2 in install step for PuLP-4.2.0-foss-2026.1.eb (took 3 secs)
== Summary:
   * [FAILED]  PuLP/4.2.0-foss-2026.1
== eb rc=1
";

#[test]
fn a_missing_dependency_from_the_robot_is_not_a_compile_failure() {
    assert_eq!(
        classify_build_failure("build", MISSING_DEPENDENCY_OUTPUT, "", Some(31)),
        BuildFindingClass::DependencyMissing
    );
    let backend = "pip._vendor.pyproject_hooks._impl.BackendUnavailable: Cannot import 'maturin'\nERROR: Shell command failed!";
    assert_eq!(
        classify_build_failure("build", backend, "", Some(1)),
        BuildFindingClass::DependencyMissing
    );
}

#[test]
fn an_unaccepted_eula_is_a_license_finding_for_a_human() {
    let class = classify_build_failure("build", EULA_OUTPUT, "", Some(1));
    assert_eq!(class, BuildFindingClass::License);
    assert_eq!(
        serde_json::to_value(class).expect("class"),
        serde_json::json!("license")
    );
}

#[test]
fn the_failed_easybuild_step_names_the_class() {
    assert_eq!(
        classify_build_failure("build", INSTALL_STEP_OUTPUT, "", Some(1)),
        BuildFindingClass::Install
    );
    let configure = INSTALL_STEP_OUTPUT.replace("in install step", "in configure step");
    assert_eq!(
        classify_build_failure("build", &configure, "", Some(1)),
        BuildFindingClass::Configure
    );
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
    write_executable(
        &command,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' 'output (stdout + stderr)  ->  {}'\nprintf '%s\\n' 'ERROR: installation failed'\nexit 1\n",
            nested_log.display()
        ),
    )
    .expect("command");

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: target(command.to_str().expect("command path")),
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign finding");
    assert_eq!(state.findings[0].class, BuildFindingClass::Runtime);
    assert!(state.findings[0].evidence.contains("GLIBC_2.38"));
    assert!(state.findings[0]
        .evidence
        .contains(&nested_log.display().to_string()));
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
        execution_context: Default::default(),
        current_recipe: Some(interrupted_recipe.into()),
        findings: Vec::new(),
        history: Vec::new(),
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
        .filter(|event| event.detail.starts_with("EasyBuild command succeeded"))
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
    let expected = format!("--robot=/tmp:{}", bundle.join("easyconfigs").display());
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
fn campaign_fails_fast_when_easybuild_tmpdir_is_not_usable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipe = bundle.join("easyconfigs/e/eOn/eOn.eb");
    std::fs::create_dir_all(recipe.parent().unwrap()).expect("recipes");
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
    write_valid_recipe(&recipe, "eOn", "2.16.0");
    let missing_tmp = temp.path().join("missing-tmp");
    let mut build_target = target("true");
    build_target.easybuild.tmp_root = missing_tmp.display().to_string();

    let state = run_campaign(&CampaignRequest {
        bundle,
        target: build_target,
        state_path: temp.path().join("campaign.json"),
    })
    .expect("campaign");
    assert_eq!(state.status, CampaignStatus::Failed);
    assert_eq!(state.findings.len(), 1);
    assert_eq!(state.findings[0].class, BuildFindingClass::Resource);
    assert_eq!(state.findings[0].stage, "preflight");
    assert!(state.findings[0].summary.contains("EASYBUILD_TMPDIR"));
}

#[test]
fn campaign_uses_a_fresh_installpath_per_attempt() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipe = bundle.join("easyconfigs/e/eOn/eOn.eb");
    std::fs::create_dir_all(recipe.parent().unwrap()).expect("recipes");
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
    write_valid_recipe(&recipe, "eOn", "2.16.0");
    let state_path = temp.path().join("campaign.json");

    let first = run_campaign(&CampaignRequest {
        bundle: bundle.clone(),
        target: target("false"),
        state_path: state_path.clone(),
    })
    .expect("first attempt");
    let first_command = &first.findings[0].command.args;
    assert!(
        first_command
            .iter()
            .any(|arg| arg == "--installpath=/tmp/install-attempts/eOn-2.16.0/attempt-1"),
        "{first_command:?}"
    );

    let second = run_campaign(&CampaignRequest {
        bundle,
        target: target("false"),
        state_path,
    })
    .expect("second attempt");
    let second_command = &second.findings[1].command.args;
    assert!(
        second_command
            .iter()
            .any(|arg| arg == "--installpath=/tmp/install-attempts/eOn-2.16.0/attempt-2"),
        "{second_command:?}"
    );
}

#[test]
fn campaign_records_execution_context_on_results() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bundle = temp.path().join("bundle");
    let recipe = bundle.join("easyconfigs/e/eOn/eOn.eb");
    std::fs::create_dir_all(recipe.parent().unwrap()).expect("recipes");
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
    write_valid_recipe(&recipe, "eOn", "2.16.0");

    let failed = run_campaign(&CampaignRequest {
        bundle: bundle.clone(),
        target: target("false"),
        state_path: temp.path().join("campaign-failed.json"),
    })
    .expect("failed campaign");
    assert_eq!(
        failed.findings[0].execution_context, failed.execution_context,
        "findings must carry the same qualifying context as the campaign run"
    );
    assert!(!failed.execution_context.arch.is_empty());
    assert!(!failed.execution_context.cpu_model.is_empty());
    assert!(!failed.execution_context.optarch.is_empty());

    let completed = run_campaign(&CampaignRequest {
        bundle,
        target: target("true"),
        state_path: temp.path().join("campaign-ok.json"),
    })
    .expect("completed campaign");
    let done = completed.history.last().expect("completion event");
    assert!(done.detail.contains("optarch="), "{}", done.detail);
    assert!(done.detail.contains("arch="), "{}", done.detail);
    assert!(done.detail.contains("cpu="), "{}", done.detail);
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
    assert_eq!(state.findings[0].stage, "preflight");
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
    assert_eq!(
        state.findings[0].summary,
        "runtime failure from EasyBuild command (exit 127)"
    );
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
        .contains("module=QMCPACK/4.3.0-complex-foss-2026.1"));
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
fn campaign_claim_of_a_missing_state_leaves_no_lock_guard() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state = temp.path().join("nope.json");
    let claim = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "campaign",
            "finding",
            "claim",
            "--state",
            state.to_str().unwrap(),
            "--id",
            "FINDING",
            "--owner",
            "worker",
        ])
        .output()
        .expect("claim command");
    let stderr = String::from_utf8_lossy(&claim.stderr);
    assert_eq!(claim.status.code(), Some(1), "{stderr}");
    assert!(
        !std::path::PathBuf::from(format!("{}.lock", state.display())).exists(),
        "metadata left behind"
    );
    assert!(
        !std::path::PathBuf::from(format!("{}.lock.guard", state.display())).exists(),
        "guard left behind"
    );
}
