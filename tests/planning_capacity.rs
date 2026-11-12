use eb_stack::planning_capacity::{inspect, CapacityRequest};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn node(name: &str, state: &[&str]) -> Value {
    json!({"name":name,"partitions":["compute"],"target":"example/host",
           "available_features":["scratch-node"],"active_features":["scratch-node"],
           "state_flags":state,"cpus_total":16,"cpus_allocatable":16,"cpus_allocated":0,
           "memory_mib_total":32768,"memory_mib_allocatable":32768,"memory_mib_allocated":0,
           "gpus_total":4,"gpus_allocatable":4,"gpus_allocated":0,"gpu_type":"example-gpu","cpu_load":90.0})
}
fn snapshot(nodes: Vec<Value>) -> Value {
    json!({"schema_version":1,"observed_at":"2026-01-01T12:00:00Z",
           "partition_inventory_complete":true,"nodes":nodes})
}
fn request() -> CapacityRequest {
    serde_json::from_value(json!({"partition":"compute","host_target":"example/host",
        "features":["scratch-node"],"cpus":8,"memory_mib":8192,"gpus":1,"gpu_type":"example-gpu"}))
    .unwrap()
}
fn report(value: &Value) -> Value {
    inspect(
        &serde_json::to_vec(value).unwrap(),
        &request(),
        "2026-01-01T12:00:10Z",
        60,
    )
    .unwrap()
}

#[test]
fn mixed_resources_are_subtracted_and_unavailable_states_are_excluded() {
    let mut mixed = node("mixed", &["MIXED"]);
    mixed["cpus_allocated"] = json!(8);
    mixed["memory_mib_allocated"] = json!(24576);
    mixed["gpus_allocated"] = json!(3);
    let mut allocated = node("allocated", &["ALLOCATED"]);
    allocated["cpus_allocated"] = json!(16);
    let rows = vec![
        node("idle", &["IDLE"]),
        mixed,
        allocated,
        node("drain", &["IDLE", "DRAIN"]),
        node("down", &["DOWN"]),
        node("reserved", &["IDLE", "RESERVED"]),
        node("unknown", &["UNKNOWN"]),
    ];
    let result = report(&snapshot(rows));
    assert_eq!(result["feature_eligible_nodes"], 7);
    assert_eq!(result["queue_fitting_nodes"], 3);
    assert_eq!(result["idle_nodes"], 1);
    assert_eq!(result["free_fitting_nodes"], 2);
    assert_eq!(result["free_slots"], 3);
    assert_eq!(result["known_free_resources"]["gpus"], 5);
    assert_eq!(result["excluded"]["state:DRAIN"], 1);
    assert_eq!(result["excluded"]["state:RESERVED"], 1);
    assert_eq!(result["excluded"]["state:DOWN"], 1);
    assert_eq!(result["placement_guarantee"], false);
    assert_eq!(result["pending"]["status"], "unknown");
    assert!(result["pending"]["known_demand"].is_null());
    assert!(!result.to_string().contains("example-gpu-private-host"));
    assert!(!result.to_string().contains("\"name\":"));
}

#[test]
fn resources_are_fitted_per_node_instead_of_pooled() {
    let mut cpu = node("cpu-fragment", &["MIXED"]);
    cpu["memory_mib_allocated"] = json!(32767);
    let mut memory = node("memory-fragment", &["MIXED"]);
    memory["cpus_allocated"] = json!(15);
    let result = report(&snapshot(vec![cpu, memory]));
    assert_eq!(result["queue_fitting_nodes"], 2);
    assert_eq!(result["known_free_resources"]["cpus"], 17);
    assert_eq!(result["free_slots"], 0);
    assert_eq!(result["free_fitting_nodes"], 0);
}

#[test]
fn missing_resources_and_inactive_features_never_create_free_capacity() {
    let mut missing = node("unknown-resource", &["IDLE"]);
    missing["memory_mib_allocatable"] = Value::Null;
    let mut inactive = node("inactive-feature", &["IDLE"]);
    inactive["active_features"] = json!([]);
    let result = report(&snapshot(vec![missing, inactive]));
    assert_eq!(result["queue_fitting_nodes"], 1);
    assert_eq!(result["free_slots"], 0);
    assert_eq!(result["excluded"]["resources_unknown_or_inconsistent"], 1);
    assert_eq!(
        result["excluded"]["active_features_unavailable_or_unknown"],
        1
    );
}

#[test]
fn partition_and_physical_target_are_exact_and_missing_views_stay_unknown() {
    let mut other_partition = node("other-partition", &["IDLE"]);
    other_partition["partitions"] = json!(["other"]);
    let mut other_target = node("other-target", &["IDLE"]);
    other_target["target"] = json!("example/guest");
    let mut value = snapshot(vec![other_partition, other_target]);
    assert_eq!(report(&value)["partition_target_nodes"], 0);
    value["partition_inventory_complete"] = json!(false);
    let result = report(&value);
    assert_eq!(result["status"], "unknown");
    assert!(result["free_slots"].is_null());
    let mut empty = snapshot(vec![]);
    assert_eq!(report(&empty)["reason"], "requested_partition_unknown");
    empty["selected_partitions"] = json!(["compute"]);
    assert_eq!(report(&empty)["free_slots"], 0);
}

#[test]
fn timestamps_are_bounded_and_digest_covers_the_original_bytes() {
    let value = snapshot(vec![node("node-a", &["IDLE"])]);
    let bytes = serde_json::to_vec(&value).unwrap();
    let r = inspect(&bytes, &request(), "2026-01-01T12:00:10Z", 10).unwrap();
    let expected: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(r["snapshot"]["sha256"], expected);
    assert!(inspect(&bytes, &request(), "2026-01-01T12:00:11Z", 10)
        .unwrap_err()
        .to_string()
        .contains("stale"));
    assert!(inspect(&bytes, &request(), "2026-01-01T11:59:59Z", 10).is_err());
    let mut altered = bytes.clone();
    altered.push(b'\n');
    assert_ne!(
        inspect(&altered, &request(), "2026-01-01T12:00:10Z", 10).unwrap()["snapshot"]["sha256"],
        r["snapshot"]["sha256"]
    );
}

#[test]
fn pending_demand_is_scoped_and_unknown_inputs_are_visible() {
    let mut value = snapshot(vec![node("node-a", &["IDLE"])]);
    value["pending_complete"] = json!(false);
    value["pending_jobs"] = json!([
      {"id":"one","state":"PENDING","partitions":["compute"],"target":"example/host","features":["scratch-node"],"cpus":8,"memory_mib":8192,"gpus":1},
      {"id":"broad","state":"PENDING","partitions":["compute"],"target":null,"features":null,"cpus":4,"memory_mib":null,"gpus":0},
      {"id":"other","state":"PENDING","partitions":["elsewhere"],"target":"example/host","features":[],"cpus":900,"memory_mib":900,"gpus":9}]);
    let result = report(&value);
    assert_eq!(result["pending"]["status"], "incomplete");
    assert_eq!(result["pending"]["confirmed_matching_jobs"], 1);
    assert_eq!(result["pending"]["possibly_competing_jobs"], 1);
    assert_eq!(result["pending"]["jobs_with_unknown_resources"], 1);
    assert_eq!(result["pending"]["known_demand"]["cpus"], 12);
    assert_eq!(result["free_slots"], 2);
}

#[test]
fn malformed_constraints_duplicates_and_inconsistent_resources_are_not_accepted() {
    let value = snapshot(vec![node("one", &["IDLE"]), node("one", &["IDLE"])]);
    assert!(inspect(
        &serde_json::to_vec(&value).unwrap(),
        &request(),
        "2026-01-01T12:00:10Z",
        60
    )
    .is_err());
    let mut req = request();
    req.features.insert("scratch-node|other".into());
    assert!(inspect(
        &serde_json::to_vec(&snapshot(vec![])).unwrap(),
        &req,
        "2026-01-01T12:00:10Z",
        60
    )
    .is_err());
    let mut value = node("bad", &["MIXED"]);
    value["cpus_allocated"] = json!(17);
    assert_eq!(report(&snapshot(vec![value]))["free_slots"], 0);
}

#[test]
fn cpu_requests_do_not_require_gpu_observations() {
    let mut n = node("cpu-only", &["IDLE"]);
    n["gpus_allocatable"] = Value::Null;
    n["gpus_allocated"] = Value::Null;
    let mut req = request();
    req.gpus = 0;
    req.gpu_type = None;
    let value = inspect(
        &serde_json::to_vec(&snapshot(vec![n])).unwrap(),
        &req,
        "2026-01-01T12:00:10Z",
        60,
    )
    .unwrap();
    assert_eq!(value["free_slots"], 2);
    assert_eq!(value["gpu_capacity_unknown_nodes"], 1);
    assert!(value["known_free_resources"]["gpus"].is_null());
    req.memory_mib = 0;
    assert!(inspect(
        &serde_json::to_vec(&snapshot(vec![])).unwrap(),
        &req,
        "2026-01-01T12:00:10Z",
        60
    )
    .is_err());
}
