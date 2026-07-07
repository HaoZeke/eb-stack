//! EasyBuild stack lock: parse `.eb` files, resolvo SAT co-select, planned SBOM.

pub mod domain;
pub mod eb_parse;
pub mod resolvo_provider;
pub mod sbom;
pub mod select;
pub mod version;

pub use domain::*;
pub use eb_parse::{
    filter_toolchain, lock_from_candidates, parse_easyconfig_file, parse_easyconfig_tree,
    validate_lock_deps,
};
pub use sbom::{dep_map_from_universe, lock_to_cyclonedx, lock_to_cyclonedx_with_deps};
pub use select::{select_stack, SelectError};

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

pub fn load_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let s = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&s).with_context(|| format!("parse {}", path.display()))?)
}

pub fn write_json_pretty(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let s = serde_json::to_string_pretty(value)?;
    fs::write(path, s + "\n")?;
    Ok(())
}

/// Parse easyconfigs dir, filter to policy toolchain, solve with resolvo, write lock+SBOM.
pub fn solve_from_easyconfigs(
    easyconfigs_root: &Path,
    policy_path: &Path,
    baseline_easyconfigs: Option<&Path>,
    lock_out: &Path,
    sbom_out: &Path,
) -> Result<StackLock> {
    let policy: Policy = load_json_file(policy_path)?;
    let all = parse_easyconfig_tree(easyconfigs_root).map_err(|e| anyhow::anyhow!(e))?;
    let universe_cands = filter_toolchain(&all, &policy.toolchain);
    if universe_cands.is_empty() {
        bail!(
            "no easyconfigs for toolchain {}-{} under {}",
            policy.toolchain.name,
            policy.toolchain.version,
            easyconfigs_root.display()
        );
    }
    let universe = Universe {
        toolchain: policy.toolchain.clone(),
        generation_label: Some(format!(
            "{}-{}",
            policy.toolchain.name, policy.toolchain.version
        )),
        candidates: universe_cands.clone(),
    };

    let baseline = if let Some(base_root) = baseline_easyconfigs {
        let base_all = parse_easyconfig_tree(base_root).map_err(|e| anyhow::anyhow!(e))?;
        // If baseline path is parent of both gens, filter 2025a from same tree using
        // baseline_toolchain? For simplicity: if same root as easyconfigs, filter foss-2025a
        // from full tree by reading a sibling generation — here baseline_easyconfigs is the
        // directory containing the baseline generation easyconfigs only OR full tree.
        // Prefer: if any candidate has different toolchain, use baseline generation from policy? 
        // We take all packages from baseline path filtered to names we care about for upgrade.
        let mut base_cands = base_all;
        // If baseline tree includes multiple toolchains, prefer older generation than policy.
        if base_cands.iter().any(|c| c.toolchain.version != policy.toolchain.version) {
            // use the non-target toolchain versions present (e.g. 2025a)
            let versions: std::collections::BTreeSet<_> = base_cands
                .iter()
                .filter(|c| c.toolchain.name == policy.toolchain.name)
                .map(|c| c.toolchain.version.clone())
                .collect();
            if let Some(bv) = versions.iter().find(|v| *v != &policy.toolchain.version) {
                base_cands.retain(|c| {
                    c.toolchain.name == policy.toolchain.name && c.toolchain.version == *bv
                });
            }
        }
        Some(lock_from_candidates(
            &base_cands,
            Some("baseline-from-eb".into()),
            "eb_parse_baseline",
        ))
    } else {
        None
    };

    let lock = select_stack(&universe, &policy, baseline.as_ref()).map_err(|e| anyhow::anyhow!(e))?;
    validate_lock_deps(&lock, &universe.candidates).map_err(|e| anyhow::anyhow!(e))?;
    write_json_pretty(lock_out, &lock)?;
    let dep_map = dep_map_from_universe(&lock, &universe);
    let sbom = lock_to_cyclonedx_with_deps(&lock, Some(&dep_map));
    write_json_pretty(sbom_out, &sbom)?;
    Ok(lock)
}

/// Backward-compatible path: pre-baked universe JSON (still supported for tests).
pub fn solve_to_files(
    universe_path: &Path,
    policy_path: &Path,
    baseline_path: Option<&Path>,
    lock_out: &Path,
    sbom_out: &Path,
) -> Result<StackLock> {
    let universe: Universe = load_json_file(universe_path)?;
    let policy: Policy = load_json_file(policy_path)?;
    let baseline = match baseline_path {
        Some(p) => Some(load_json_file::<StackLock>(p)?),
        None => None,
    };
    let lock =
        select_stack(&universe, &policy, baseline.as_ref()).map_err(|e| anyhow::anyhow!(e))?;
    validate_lock_deps(&lock, &universe.candidates).map_err(|e| anyhow::anyhow!(e))?;
    write_json_pretty(lock_out, &lock)?;
    let dep_map = dep_map_from_universe(&lock, &universe);
    let sbom = lock_to_cyclonedx_with_deps(&lock, Some(&dep_map));
    write_json_pretty(sbom_out, &sbom)?;
    Ok(lock)
}
