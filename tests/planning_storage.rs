use eb_stack::planning_storage::StoragePolicy;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::Path;
use tempfile::TempDir;

const TIME: &str = "2026-01-01T12:00:10Z";
fn fixture() -> (TempDir, Value, Value) {
    let dir = TempDir::new().unwrap();
    let snapshot: Value =
        serde_json::from_str(include_str!("planning_capacity_fixtures/snapshot.json")).unwrap();
    let policy: Value = serde_json::from_str(include_str!(
        "planning_capacity_fixtures/storage-policy.json"
    ))
    .unwrap();
    (dir, snapshot, policy)
}
fn write(dir: &Path, snapshot: &Value, mut policy: Value) -> StoragePolicy {
    let bytes = serde_json::to_vec(snapshot).unwrap();
    policy["capacity_sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
    std::fs::write(dir.join("snapshot.json"), bytes).unwrap();
    std::fs::write(
        dir.join("policy.json"),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
    StoragePolicy::capture(&dir.join("policy.json"), 100000).unwrap()
}
fn plan() -> Value {
    json!({"cpu_prerequisites":{"file":"cpu.list","modules":["Small/1"],"inode_footprint":1000},
    "prelude":{"file":"prelude.list","roots":["Medium-1.eb"],"inode_footprint":2000},
    "shards":[{"name":"shard-01","modules":["Large/1"],"inode_footprint":3000},
              {"name":"shard-02","modules":["Huge/1"],"inode_footprint":9000}]})
}
#[test]
fn smallest_phases_use_shared_budget_and_every_overflow_phase_has_exact_storage_advice() {
    let (dir, snapshot, mut policy) = fixture();
    policy["profiles"]["cpu_prerequisites"]["compile_target"] = json!("example/guest");
    policy["profiles"]["cpu_prerequisites"]["node_local_route"] = json!("guest");
    let captured = write(dir.path(), &snapshot, policy);
    let mut result = plan();
    captured
        .annotate(&mut result, &BTreeSet::from(["Huge".into()]), TIME)
        .unwrap();
    assert_eq!(result["cpu_prerequisites"]["placement"], "shared");
    assert_eq!(result["prelude"]["placement"], "shared");
    assert_eq!(result["shards"][0]["placement"], "node-local");
    assert_eq!(result["shards"][1]["placement"], "node-local");
    assert_eq!(result["shards"][1]["storage"]["requires_real_disk"], true);
    assert_eq!(
        result["cpu_prerequisites"]["storage"]["compile_target"],
        "example/guest"
    );
    assert_eq!(
        result["cpu_prerequisites"]["storage"]["request"]["host_target"],
        "example/host"
    );
    assert_eq!(
        result["storage_policy"]["shared_budget"]["assigned_inodes"],
        3000
    );
    assert_eq!(
        result["storage_policy"]["shared_budget"]["remaining_inodes"],
        1000
    );
    assert_eq!(
        result["storage_policy"]["phase_order"],
        json!(["cpu-prerequisites", "prelude", "shard-01", "shard-02"])
    );
    for phase in [
        &result["cpu_prerequisites"],
        &result["prelude"],
        &result["shards"][0],
        &result["shards"][1],
    ] {
        assert_eq!(phase["storage"]["admission_required"], true);
        assert_eq!(phase["storage"]["admission_bypass"], false);
        assert_eq!(
            phase["storage"]["execution_storage_verification_required"],
            true
        );
        assert_eq!(phase["storage"]["placement_guarantee"], false);
    }
}
#[test]
fn allocated_hosts_are_queue_eligible_without_a_start_or_free_slot_promise() {
    let (dir, mut snapshot, policy) = fixture();
    snapshot["nodes"][0]["state_flags"] = json!(["ALLOCATED"]);
    snapshot["nodes"][0]["cpus_allocated"] = json!(16);
    let captured = write(dir.path(), &snapshot, policy);
    let mut result = plan();
    captured
        .annotate(&mut result, &BTreeSet::new(), TIME)
        .unwrap();
    assert_eq!(result["shards"][0]["placement"], "node-local");
    assert_eq!(
        result["storage_policy"]["capacity"]["shards"]["free_slots"],
        0
    );
    assert_eq!(
        result["storage_policy"]["capacity"]["shards"]["queue_fitting_nodes"],
        1
    );
}
#[test]
fn unavailable_or_incomplete_capacity_keeps_overflow_on_shared_admission() {
    for change in ["drained", "incomplete", "missing-feature", "unknown-policy"] {
        let (dir, mut snapshot, mut policy) = fixture();
        match change {
            "drained" => snapshot["nodes"][0]["state_flags"] = json!(["IDLE", "DRAIN"]),
            "incomplete" => snapshot["partition_inventory_complete"] = json!(false),
            "missing-feature" => snapshot["nodes"][0]["available_features"] = json!([]),
            "unknown-policy" => policy["profiles"]["shards"]["node_local"]
                .as_object_mut()
                .unwrap()
                .remove("exclusive_node")
                .map(|_| ())
                .unwrap(),
            _ => unreachable!(),
        }
        let captured = write(dir.path(), &snapshot, policy);
        let mut result = plan();
        captured
            .annotate(&mut result, &BTreeSet::new(), TIME)
            .unwrap();
        assert_eq!(result["shards"][0]["placement"], "shared", "{change}");
        assert_eq!(
            result["shards"][0]["storage"]["decision"], "awaiting_shared_admission",
            "{change}"
        );
        assert_eq!(
            result["storage_policy"]["shared_budget"]["awaiting_inodes"], 12000,
            "{change}"
        );
    }
}
#[test]
fn policy_uses_captured_snapshot_even_when_the_path_changes() {
    let (dir, snapshot, policy) = fixture();
    let captured = write(dir.path(), &snapshot, policy);
    let fingerprint = captured.fingerprint();
    std::fs::write(dir.path().join("snapshot.json"), b"invalid replacement").unwrap();
    std::fs::write(dir.path().join("policy.json"), b"invalid replacement").unwrap();
    let mut result = plan();
    captured
        .annotate(&mut result, &BTreeSet::new(), TIME)
        .unwrap();
    assert_eq!(result["shards"][0]["placement"], "node-local");
    assert_eq!(captured.fingerprint(), fingerprint);
}
#[test]
fn stale_budget_digest_mismatch_and_margin_mismatch_are_rejected() {
    let (dir, snapshot, policy) = fixture();
    let captured = write(dir.path(), &snapshot, policy);
    assert!(captured
        .annotate(&mut plan(), &BTreeSet::new(), "2026-01-01T12:02:00Z")
        .is_err());
    assert!(StoragePolicy::capture(&dir.path().join("policy.json"), 0).is_err());
    std::fs::write(dir.path().join("snapshot.json"), b"{}").unwrap();
    assert!(StoragePolicy::capture(&dir.path().join("policy.json"), 100000).is_err());
}
#[test]
fn absent_phase_profile_and_dropped_feature_are_rejected() {
    let (dir, snapshot, mut policy) = fixture();
    policy["profiles"]
        .as_object_mut()
        .unwrap()
        .remove("prelude");
    let captured = write(dir.path(), &snapshot, policy.clone());
    assert!(captured
        .annotate(&mut plan(), &BTreeSet::new(), TIME)
        .is_err());
    policy["profiles"]["shards"]["node_local"]["features"] = json!([]);
    std::fs::write(
        dir.path().join("policy.json"),
        serde_json::to_vec(&policy).unwrap(),
    )
    .unwrap();
    assert!(StoragePolicy::capture(&dir.path().join("policy.json"), 100000).is_err());
}

#[test]
fn shard_cli_preserves_defaults_and_binds_every_opt_in_list_to_its_bytes() {
    use std::process::Command;
    let (dir, snapshot, policy) = fixture();
    let _captured = write(dir.path(), &snapshot, policy);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/stack_shard_fixtures");
    let run = |out: &str, policy: bool, quota: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        cmd.env_remove("EASYBUILD_FILTER_DEPS")
            .args([
                "stack",
                "shard",
                "--shards",
                "4",
                "--generated-at",
                TIME,
                "--build-list",
            ])
            .arg(fixtures.join("wave1.list"))
            .arg("--easyconfigs")
            .arg(fixtures.join("easyconfigs"))
            .arg("--installed")
            .arg(fixtures.join("installed.txt"))
            .arg("--installed-modules-dir")
            .arg(fixtures.join("modules/all"))
            .arg("--out-dir")
            .arg(dir.path().join(out));
        if policy {
            cmd.arg("--storage-policy")
                .arg(dir.path().join("policy.json"));
        }
        if quota {
            cmd.args(["--inode-quota", "4000000"]);
        }
        cmd.output().unwrap()
    };
    let baseline = run("baseline", false, false);
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    let baseline: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("baseline/plan.json")).unwrap())
            .unwrap();
    assert!(baseline.get("storage_policy").is_none());
    assert!(baseline["prelude"].get("placement").is_none());
    let opted = run("opted", true, false);
    assert!(
        opted.status.success(),
        "{}",
        String::from_utf8_lossy(&opted.stderr)
    );
    let plan: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("opted/plan.json")).unwrap())
            .unwrap();
    for phase in std::iter::once(&plan["prelude"]).chain(plan["shards"].as_array().unwrap()) {
        let filename = phase["file"].as_str().unwrap();
        let bytes = std::fs::read(dir.path().join("opted").join(filename)).unwrap();
        assert_eq!(phase["storage"]["list_file"], filename);
        assert_eq!(
            phase["storage"]["list_sha256"],
            format!("{:x}", Sha256::digest(bytes))
        );
        assert_eq!(phase["storage"]["admission_required"], true);
        assert_eq!(phase["storage"]["admission_bypass"], false);
        assert_eq!(phase["placement"], "node-local");
    }
    let conflict = run("conflict", true, true);
    assert!(!conflict.status.success());
    assert!(!dir.path().join("conflict").exists());
}
