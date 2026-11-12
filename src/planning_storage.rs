//! Opt-in phase storage advice from captured scheduler and quota observations.
//!
//! A placement is a planning choice, never storage admission. Execution must
//! verify the selected domain and backing filesystem before its normal gate.

use crate::planning_capacity::{inspect, CapacityRequest};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    schema_version: u64,
    capacity_snapshot: String,
    capacity_sha256: String,
    max_age_seconds: u64,
    node_local_feature: String,
    shared_budget: SharedBudget,
    profiles: BTreeMap<String, Profile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedBudget {
    observed_at: String,
    available_inodes: u64,
    reserved_inodes: u64,
    margin_inodes: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    compile_target: String,
    shared_route: String,
    node_local_route: String,
    shared: CapacityRequest,
    node_local: CapacityRequest,
}

/// One policy and scheduler snapshot captured without subsequent path reads.
pub struct StoragePolicy {
    policy: Policy,
    snapshot: Vec<u8>,
    policy_sha256: String,
    snapshot_sha256: String,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Phase {
    id: String,
    kind: &'static str,
    index: Option<usize>,
    inodes: u64,
    real_disk: bool,
}

fn requires_disk(module: &str, required: &BTreeSet<String>) -> bool {
    if let Some((package, _)) = module.split_once('/') {
        return required.contains(package);
    }
    let name = module.strip_suffix(".eb").unwrap_or(module);
    required.iter().any(|package| {
        name.strip_prefix(package)
            .is_some_and(|rest| rest.starts_with('-'))
    })
}

fn phase_requires_disk(value: &Value, required: &BTreeSet<String>) -> bool {
    ["modules", "roots"].iter().any(|key| {
        value[*key].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item.as_str()
                    .or_else(|| item.get("module").and_then(Value::as_str))
                    .is_some_and(|name| requires_disk(name, required))
            })
        })
    })
}

fn shard_requires_disk(value: &Value, lines: &Value, required: &BTreeSet<String>) -> bool {
    phase_requires_disk(value, required)
        || lines.as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row["shard"] == value["name"]
                    && row["easyconfig"]
                        .as_str()
                        .is_some_and(|name| requires_disk(name, required))
            })
        })
}

impl StoragePolicy {
    /// Capture a policy and its expected scheduler snapshot once.
    ///
    /// # Errors
    ///
    /// A file, schema, resource request, margin or captured digest is invalid.
    pub fn capture(path: &Path, admission_margin: u64) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("--storage-policy {}: not readable", path.display()))?;
        let policy: Policy =
            serde_json::from_slice(&bytes).context("storage policy is not schema JSON")?;
        if policy.schema_version != 1 {
            bail!("storage policy schema_version must be 1");
        }
        if policy.shared_budget.margin_inodes != admission_margin {
            bail!(
                "storage policy margin must equal the configured --inode-margin {admission_margin}"
            );
        }
        if policy.node_local_feature.is_empty() {
            bail!("storage policy requires a node-local feature");
        }
        for (kind, profile) in &policy.profiles {
            if !["cpu_prerequisites", "prelude", "shards"].contains(&kind.as_str()) {
                bail!("unknown storage phase profile {kind}");
            }
            profile.shared.validate()?;
            profile.node_local.validate()?;
            if profile.compile_target.trim().is_empty()
                || profile.shared_route.trim().is_empty()
                || profile.node_local_route.trim().is_empty()
            {
                bail!(
                    "storage profile {kind} requires explicit compile target and execution routes"
                );
            }
            if !profile
                .node_local
                .features
                .contains(&policy.node_local_feature)
            {
                bail!("storage profile {kind} does not preserve the required node-local feature");
            }
        }
        let source = Path::new(&policy.capacity_snapshot);
        let source = if source.is_absolute() {
            source.to_path_buf()
        } else {
            path.parent().unwrap_or_else(|| Path::new(".")).join(source)
        };
        let snapshot =
            std::fs::read(&source).context("storage capacity snapshot is not readable")?;
        let snapshot_sha256 = digest(&snapshot);
        if policy.capacity_sha256 != snapshot_sha256 {
            bail!("storage capacity snapshot digest differs from the policy");
        }
        Ok(Self {
            policy,
            snapshot,
            policy_sha256: digest(&bytes),
            snapshot_sha256,
        })
    }

    /// Digests of the exact policy and snapshot bytes that decide placement.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        format!(
            "storage_policy={}\ncapacity_snapshot={}\n",
            self.policy_sha256, self.snapshot_sha256
        )
    }

    /// Attach advice to every nonempty CPU-prerequisite, prelude and shard phase.
    ///
    /// # Errors
    ///
    /// The shared observation is stale, a phase lacks a configured profile,
    /// generated phase metadata is malformed, or accounting overflows.
    pub fn annotate(
        &self,
        document: &mut Value,
        required: &BTreeSet<String>,
        evaluated_at: &str,
    ) -> Result<()> {
        let observed = chrono::DateTime::parse_from_rfc3339(&self.policy.shared_budget.observed_at)
            .context("invalid shared-budget observed_at")?;
        let evaluated = chrono::DateTime::parse_from_rfc3339(evaluated_at)
            .context("invalid storage evaluation time")?;
        let bound =
            i64::try_from(self.policy.max_age_seconds).context("storage max age is too large")?;
        if observed > evaluated
            || evaluated.signed_duration_since(observed)
                > chrono::Duration::try_seconds(bound).context("storage max age is too large")?
        {
            bail!("shared-budget observation is future-dated or stale");
        }
        let mut phases = Vec::new();
        for (field, id) in [
            ("cpu_prerequisites", "cpu-prerequisites"),
            ("prelude", "prelude"),
        ] {
            let Some(value) = document.get(field) else {
                continue;
            };
            let count = ["modules", "roots"]
                .iter()
                .filter_map(|key| value[*key].as_array())
                .map(Vec::len)
                .sum::<usize>();
            if count == 0 || value["file"].is_null() {
                continue;
            }
            phases.push(Phase {
                id: id.into(),
                kind: field,
                index: None,
                inodes: value["inode_footprint"]
                    .as_u64()
                    .context("phase inode footprint is missing")?,
                real_disk: phase_requires_disk(value, required),
            });
        }
        for (index, value) in document["shards"]
            .as_array()
            .context("plan shards are missing")?
            .iter()
            .enumerate()
        {
            phases.push(Phase {
                id: value["name"]
                    .as_str()
                    .context("shard name is missing")?
                    .into(),
                kind: "shards",
                index: Some(index),
                inodes: value["inode_footprint"]
                    .as_u64()
                    .context("shard inode footprint is missing")?,
                real_disk: shard_requires_disk(value, &document["lines"], required),
            });
        }
        phases.sort_by(|a, b| (a.inodes, &a.id).cmp(&(b.inodes, &b.id)));
        let shared = &self.policy.shared_budget;
        let spendable = shared
            .available_inodes
            .saturating_sub(shared.reserved_inodes)
            .saturating_sub(shared.margin_inodes);
        let mut remaining = spendable;
        let mut assigned = 0u64;
        let mut awaiting = 0u64;
        let mut advice = BTreeMap::new();
        for phase in &phases {
            let profile = self
                .policy
                .profiles
                .get(phase.kind)
                .with_context(|| format!("storage profile {} is required", phase.kind))?;
            if !advice.contains_key(phase.kind) {
                let report = inspect(&self.snapshot, &profile.node_local, evaluated_at, self.policy.max_age_seconds)
                    .unwrap_or_else(|error| json!({"advisory":true,"usable_for_placement":false,"status":"unknown","reason":"snapshot_rejected","error":error.to_string()}));
                advice.insert(phase.kind.to_string(), report);
            }
            let local = &advice[phase.kind];
            let eligible = local["usable_for_placement"] == true
                && local["queue_fitting_nodes"]
                    .as_u64()
                    .is_some_and(|count| count > 0);
            let (domain, decision, request, route) = if phase.inodes <= remaining {
                remaining -= phase.inodes;
                assigned = assigned
                    .checked_add(phase.inodes)
                    .context("shared phase accounting overflows")?;
                (
                    "shared",
                    "fits_observed_shared_budget",
                    &profile.shared,
                    &profile.shared_route,
                )
            } else if eligible {
                (
                    "node-local",
                    "shared_budget_exhausted_queue_eligible",
                    &profile.node_local,
                    &profile.node_local_route,
                )
            } else {
                awaiting = awaiting
                    .checked_add(phase.inodes)
                    .context("awaiting phase accounting overflows")?;
                (
                    "shared",
                    "awaiting_shared_admission",
                    &profile.shared,
                    &profile.shared_route,
                )
            };
            let target = if let Some(index) = phase.index {
                &mut document["shards"][index]
            } else {
                &mut document[phase.kind]
            };
            target["placement"] = json!(domain);
            target["storage"] = json!({"phase_id":phase.id,"decision":decision,"compile_target":profile.compile_target,
                "request":request,"execution_route":route,"requires_real_disk":phase.real_disk,
                "requires_node_local_disk":domain == "node-local","admission_required":true,"admission_bypass":false,
                "execution_storage_verification_required":true,"placement_guarantee":false});
        }
        document["storage_policy"] = json!({"schema_version":1,"advisory":true,
            "policy_sha256":self.policy_sha256,"capacity_snapshot_sha256":self.snapshot_sha256,
            "node_local_feature":self.policy.node_local_feature,"evaluated_at":evaluated_at,
            "shared_budget":{"observed_at":shared.observed_at,"available_inodes":shared.available_inodes,
               "reserved_inodes":shared.reserved_inodes,"margin_inodes":shared.margin_inodes,
               "spendable_inodes":spendable,"assigned_inodes":assigned,"remaining_inodes":remaining,"awaiting_inodes":awaiting},
            "capacity":advice,"phase_order":phases.iter().map(|phase| phase.id.clone()).collect::<Vec<_>>(),
            "execution_receipt_required":true,"admission_unchanged":true});
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_shard_roots_retain_captured_storage_requirements() {
        let required = BTreeSet::from(["DiskTool".to_owned()]);
        let phase = json!({"name":"shard-01","modules":[]});
        let lines = json!([
            {"shard":"shard-01","easyconfig":"DiskTool-1.0.eb"},
            {"shard":"shard-02","easyconfig":"OtherTool-1.0.eb"}
        ]);
        assert!(shard_requires_disk(&phase, &lines, &required));
        assert!(!shard_requires_disk(
            &json!({"name":"shard-02","modules":[]}),
            &lines,
            &required
        ));
        assert!(!shard_requires_disk(&phase, &json!([]), &required));
    }
}
