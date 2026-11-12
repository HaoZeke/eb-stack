//! Advisory scheduler capacity from a captured, normalized observation.
//!
//! Allocatable resources are scheduler-effective capacities before allocation.
//! This module performs no scheduler calls and never grants storage admission.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// One single-node resource request, scoped to an exact scheduler route.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityRequest {
    /// Exact partition; no fallback partition is selected.
    pub partition: String,
    /// Physical host target, independent of any guest compiler target.
    pub host_target: String,
    /// Literal features required together, without expression rewriting.
    #[serde(default)]
    pub features: BTreeSet<String>,
    /// CPUs required on one node.
    pub cpus: u64,
    /// Scheduler memory required on one node, in MiB.
    pub memory_mib: u64,
    /// GPUs required on one node; zero is a CPU request.
    #[serde(default)]
    pub gpus: u64,
    /// Optional homogeneous GPU type required by the request.
    #[serde(default)]
    pub gpu_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    schema_version: u64,
    observed_at: String,
    observed_until: Option<String>,
    raw_sha256: Option<String>,
    cluster: Option<String>,
    nodes: Vec<Node>,
    pending_jobs: Option<Vec<Pending>>,
    #[serde(default)]
    pending_complete: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    name: String,
    partitions: BTreeSet<String>,
    target: String,
    available_features: Option<BTreeSet<String>>,
    active_features: Option<BTreeSet<String>>,
    #[serde(default)]
    state_flags: BTreeSet<String>,
    cpus_total: Option<u64>,
    cpus_allocatable: Option<u64>,
    cpus_allocated: Option<u64>,
    memory_mib_total: Option<u64>,
    memory_mib_allocatable: Option<u64>,
    memory_mib_allocated: Option<u64>,
    gpus_total: Option<u64>,
    gpus_allocatable: Option<u64>,
    gpus_allocated: Option<u64>,
    gpu_type: Option<String>,
    cpu_load: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    id: String,
    state: String,
    partitions: BTreeSet<String>,
    target: Option<String>,
    features: Option<BTreeSet<String>>,
    cpus: Option<u64>,
    memory_mib: Option<u64>,
    gpus: Option<u64>,
    gpu_type: Option<String>,
}

#[derive(Clone, Copy, Default, Serialize)]
struct Resources {
    cpus: u64,
    memory_mib: u64,
    gpus: u64,
}

impl Resources {
    fn slots(self, request: &CapacityRequest) -> u64 {
        let slots = (self.cpus / request.cpus).min(self.memory_mib / request.memory_mib);
        if request.gpus == 0 {
            slots
        } else {
            slots.min(self.gpus / request.gpus)
        }
    }

    fn add(&mut self, other: Self) -> Result<()> {
        self.cpus = self
            .cpus
            .checked_add(other.cpus)
            .context("CPU capacity overflows")?;
        self.memory_mib = self
            .memory_mib
            .checked_add(other.memory_mib)
            .context("memory capacity overflows")?;
        self.gpus = self
            .gpus
            .checked_add(other.gpus)
            .context("GPU capacity overflows")?;
        Ok(())
    }
}

fn literal(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-:".contains(&c))
}

fn validate_features(features: &BTreeSet<String>) -> Result<()> {
    if features.iter().any(|feature| !literal(feature)) {
        bail!("features must be literal names combined with AND, not scheduler expressions");
    }
    Ok(())
}

fn resource_pair(
    total: Option<u64>,
    effective: Option<u64>,
    allocated: Option<u64>,
) -> Option<(u64, u64)> {
    let effective = effective?;
    let allocated = allocated?;
    if allocated > effective || total.is_some_and(|total| effective > total) {
        return None;
    }
    Some((effective, effective - allocated))
}

impl Node {
    fn resources(&self) -> Option<(Resources, Resources)> {
        let cpu = resource_pair(self.cpus_total, self.cpus_allocatable, self.cpus_allocated)?;
        let mem = resource_pair(
            self.memory_mib_total,
            self.memory_mib_allocatable,
            self.memory_mib_allocated,
        )?;
        let gpu = resource_pair(self.gpus_total, self.gpus_allocatable, self.gpus_allocated)?;
        Some((
            Resources {
                cpus: cpu.0,
                memory_mib: mem.0,
                gpus: gpu.0,
            },
            Resources {
                cpus: cpu.1,
                memory_mib: mem.1,
                gpus: gpu.1,
            },
        ))
    }

    fn state(&self) -> Result<&str, String> {
        let mut base = None;
        for flag in &self.state_flags {
            match flag.as_str() {
                "IDLE" | "MIXED" | "ALLOCATED" if base.is_none() => base = Some(flag.as_str()),
                "DYNAMIC" | "DYNAMIC_NORM" => (),
                flag => return Err(format!("state:{flag}")),
            }
        }
        base.ok_or_else(|| "state:UNKNOWN".into())
    }
}

/// Summarize one captured normalized snapshot without exposing node names.
///
/// Unknown resources, unknown states and inactive requested features contribute
/// no free slots. Queue-eligible configured capacity is separate from capacity
/// free at the observation. Pending demand is not subtracted as a placement or
/// queue-time prediction.
///
/// # Errors
///
/// The document, resource request or timestamp is invalid, the observation is
/// stale/future-dated, or resource sums overflow.
pub fn inspect(
    bytes: &[u8],
    request: &CapacityRequest,
    evaluated_at: &str,
    max_age_seconds: u64,
) -> Result<Value> {
    let snapshot: Snapshot =
        serde_json::from_slice(bytes).context("capacity snapshot is not schema JSON")?;
    if snapshot.schema_version != 1 {
        bail!("capacity snapshot schema_version must be 1");
    }
    if request.partition.trim().is_empty() || request.host_target.trim().is_empty() {
        bail!("capacity partition and physical host target are required");
    }
    if request.cpus == 0 || request.memory_mib == 0 {
        bail!("capacity CPUs and memory_mib must be positive");
    }
    if request.gpu_type.is_some() && request.gpus == 0 {
        bail!("gpu_type requires a nonzero GPU request");
    }
    validate_features(&request.features)?;
    let observed = chrono::DateTime::parse_from_rfc3339(&snapshot.observed_at)
        .context("invalid capacity observed_at")?;
    let evaluated = chrono::DateTime::parse_from_rfc3339(evaluated_at)
        .context("invalid capacity evaluated_at")?;
    if observed > evaluated {
        bail!("capacity observation is in the future");
    }
    let max_age = i64::try_from(max_age_seconds).context("capacity max age is too large")?;
    let age = evaluated.signed_duration_since(observed);
    if age > chrono::Duration::try_seconds(max_age).context("capacity max age is too large")? {
        bail!("capacity snapshot is stale: exceeds {max_age_seconds} seconds");
    }
    if let Some(until) = &snapshot.observed_until {
        let until = chrono::DateTime::parse_from_rfc3339(until)
            .context("invalid capacity observed_until")?;
        if until < observed || until > evaluated {
            bail!("capacity observation interval is invalid");
        }
    }
    if snapshot
        .raw_sha256
        .as_ref()
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        bail!("capacity raw_sha256 must be a SHA256 hex digest");
    }
    if snapshot.pending_jobs.is_none() && snapshot.pending_complete {
        bail!("absent pending data cannot be complete");
    }
    let _cluster = &snapshot.cluster;
    let mut identities = BTreeSet::new();
    let mut scoped = Vec::new();
    let mut partition_target_nodes = 0u64;
    let mut eligible = 0u64;
    let mut queue_fitting = 0u64;
    let mut free_fitting = 0u64;
    let mut free_slots = 0u64;
    let mut idle_nodes = 0u64;
    let mut free = Resources::default();
    let mut excluded: BTreeMap<String, u64> = BTreeMap::new();
    let mut states: BTreeMap<String, u64> = BTreeMap::new();
    for node in &snapshot.nodes {
        if node.name.is_empty() || !identities.insert((&node.name, &node.target)) {
            bail!("duplicate or empty capacity node identity");
        }
        if node
            .cpu_load
            .is_some_and(|load| !load.is_finite() || load < 0.0)
        {
            bail!("invalid capacity CPU load");
        }
        if !node.partitions.contains(&request.partition) || node.target != request.host_target {
            continue;
        }
        partition_target_nodes += 1;
        for state in &node.state_flags {
            *states.entry(state.clone()).or_default() += 1;
        }
        if let Some(features) = &node.available_features {
            validate_features(features)?;
        }
        if let Some(features) = &node.active_features {
            validate_features(features)?;
        }
        let reason = if !node
            .available_features
            .as_ref()
            .is_some_and(|features| request.features.is_subset(features))
        {
            Some("features_unavailable_or_unknown".to_string())
        } else if request
            .gpu_type
            .as_ref()
            .is_some_and(|kind| node.gpu_type.as_ref() != Some(kind))
        {
            Some("gpu_type_unavailable_or_unknown".to_string())
        } else {
            None
        };
        if let Some(reason) = reason {
            *excluded.entry(reason).or_default() += 1;
            continue;
        }
        eligible += 1;
        scoped.push(node);
        let state = match node.state() {
            Ok(state) => state,
            Err(reason) => {
                *excluded.entry(reason).or_default() += 1;
                continue;
            }
        };
        let Some((configured, remaining)) = node.resources() else {
            *excluded
                .entry("resources_unknown_or_inconsistent".into())
                .or_default() += 1;
            continue;
        };
        if configured.slots(request) > 0 {
            queue_fitting += 1;
        }
        if state == "IDLE" {
            idle_nodes += 1;
        }
        if state == "ALLOCATED" {
            continue;
        }
        if !node
            .active_features
            .as_ref()
            .is_some_and(|features| request.features.is_subset(features))
        {
            *excluded
                .entry("active_features_unavailable_or_unknown".into())
                .or_default() += 1;
            continue;
        }
        free.add(remaining)?;
        let slots = remaining.slots(request);
        if slots > 0 {
            free_fitting += 1;
        }
        free_slots = free_slots
            .checked_add(slots)
            .context("capacity slot sum overflows")?;
    }
    let pending = if let Some(jobs) = &snapshot.pending_jobs {
        let mut ids = BTreeSet::new();
        let mut confirmed = 0u64;
        let mut possible = 0u64;
        let mut unknown_resources = 0u64;
        let mut demand = Resources::default();
        for job in jobs {
            if job.id.is_empty() || !ids.insert(&job.id) {
                bail!("duplicate or empty pending identity");
            }
            if job.state != "PENDING"
                || !job.partitions.contains(&request.partition)
                || job
                    .target
                    .as_ref()
                    .is_some_and(|target| target != &request.host_target)
            {
                continue;
            }
            if let Some(features) = &job.features {
                validate_features(features)?;
                if !scoped.iter().any(|node| {
                    node.available_features
                        .as_ref()
                        .is_some_and(|available| features.is_subset(available))
                        && job
                            .gpu_type
                            .as_ref()
                            .is_none_or(|kind| node.gpu_type.as_ref() == Some(kind))
                }) {
                    continue;
                }
            }
            if job.target.is_some() && job.features.is_some() {
                confirmed += 1;
            } else {
                possible += 1;
            }
            if job.cpus.is_none() || job.memory_mib.is_none() || job.gpus.is_none() {
                unknown_resources += 1;
            }
            demand.add(Resources {
                cpus: job.cpus.unwrap_or(0),
                memory_mib: job.memory_mib.unwrap_or(0),
                gpus: job.gpus.unwrap_or(0),
            })?;
        }
        json!({"status": if snapshot.pending_complete { "complete" } else { "incomplete" },
               "complete": snapshot.pending_complete, "confirmed_matching_jobs": confirmed,
               "possibly_competing_jobs": possible, "jobs_with_unknown_resources": unknown_resources,
               "known_demand": demand, "demand_is_not_a_start_prediction": true})
    } else {
        json!({"status":"unknown", "complete":false, "confirmed_matching_jobs":null,
               "possibly_competing_jobs":null, "known_demand":null})
    };
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(
        json!({"schema_version":1,"advisory":true,"placement_guarantee":false,
              "snapshot":{"observed_at":snapshot.observed_at,"observed_until":snapshot.observed_until,
                          "evaluated_at":evaluated_at,"age_seconds":age.num_milliseconds() as f64 / 1000.0,
                          "max_age_seconds":max_age_seconds,"sha256":digest,"raw_sha256":snapshot.raw_sha256},
              "request":request,"partition_target_nodes":partition_target_nodes,
              "feature_eligible_nodes":eligible,"queue_fitting_nodes":queue_fitting,
              "idle_nodes":idle_nodes,"free_fitting_nodes":free_fitting,"free_slots":free_slots,
              "known_free_resources":free,"state_counts":states,"excluded":excluded,"pending":pending}),
    )
}
