use eb_stack::target::{
    doctor_target, resolve_target_layers, BuildTarget, TargetConfigLayer, TargetRuntime,
    TARGET_CONFIG_SCHEMA_VERSION,
};
use std::process::Command;

#[test]
fn layered_target_config_composes_ssh_slurm_container_and_easybuild() {
    let base = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "site-builder"

[targets.transport]
kind = "ssh"
host = "builder.example.org"

[targets.executor]
kind = "direct"

[targets.runtime]
kind = "host"

[targets.easybuild]
command = "eb"
robot_paths = ["/opt/easybuild/easyconfigs", "/work/overlay"]
work_root = "/work/campaign"
tmp_root = "/work/easybuild-tmp"
"#,
    )
    .expect("base target");
    let site = TargetConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[targets]]
name = "site-builder"

[targets.executor]
kind = "slurm"
partition = "build"
cpus = 8
memory = "32G"
time = "02:00:00"

[targets.runtime]
kind = "podman"
image = "registry.example.org/easybuild:rocky9"
workdir = "/workspace"
mounts = ["/work:/workspace"]
"#,
    )
    .expect("site target");
    let targets = resolve_target_layers(&[base, site]).expect("resolve targets");
    assert_eq!(targets.len(), 1);
    let target = &targets[0];
    assert_eq!(target.name, "site-builder");
    assert!(matches!(target.runtime, TargetRuntime::Podman { .. }));
    assert_eq!(
        target.staged_bundle_path(std::path::Path::new("/control/eon-bundle")),
        "/work/campaign/bundles/eon-bundle"
    );

    let command = target.build_command("/work/campaign/bundles/eon-bundle/QMCPACK.eb");
    assert_eq!(command.program, "ssh");
    assert_eq!(command.args[0], "builder.example.org");
    let remote = command.args.last().expect("remote command");
    for token in [
        "srun",
        "--partition",
        "build",
        "podman",
        "registry.example.org/easybuild:rocky9",
        "EASYBUILD_TMPDIR=/work/easybuild-tmp",
        "eb",
        "--robot=/opt/easybuild/easyconfigs:/work/overlay",
        "/work/campaign/bundles/eon-bundle/QMCPACK.eb",
    ] {
        assert!(remote.contains(token), "missing {token}: {remote}");
    }
}

#[test]
fn local_direct_host_target_doctor_is_executable_without_building() {
    let layer = TargetConfigLayer::from_toml_str(&format!(
        r#"
schema_version = {TARGET_CONFIG_SCHEMA_VERSION}

[[targets]]
name = "local-doctor"

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
"#
    ))
    .expect("local target");
    let mut targets = resolve_target_layers(&[layer]).expect("resolve");
    let target: BuildTarget = targets.pop().expect("target");
    let report = doctor_target(&target).expect("doctor execution");
    assert!(report.ok());
    assert_eq!(report.target, "local-doctor");
    assert!(report.checks.iter().all(|check| check.success));
}

#[test]
fn target_cli_lists_and_doctors_named_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = temp.path().join("targets.toml");
    std::fs::write(
        &config,
        r#"
schema_version = 1
[[targets]]
name = "local-doctor"
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
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let list = Command::new(binary)
        .args(["target", "list", "--config", config.to_str().unwrap()])
        .output()
        .expect("target list");
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(String::from_utf8_lossy(&list.stdout).contains("local-doctor"));

    let doctor = Command::new(binary)
        .args([
            "target",
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--target",
            "local-doctor",
        ])
        .output()
        .expect("target doctor");
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).expect("doctor JSON");
    assert_eq!(report["target"], "local-doctor");
    assert_eq!(report["ok"], true);
}
