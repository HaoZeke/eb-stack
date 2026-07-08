//! EasyBuild stack lock: parse `.eb` files, resolvo SAT co-select, planned SBOM.

pub mod domain;
pub mod eb_emit;
pub mod eb_parse;
pub mod resolvo_provider;
pub mod sbom;
pub mod select;
pub mod version;

pub use domain::*;
pub use eb_emit::{
    easyconfig_filename, emit_next_generation, emit_next_generation_from_path, EmitError,
    EmitParams, EmitResult,
};
pub use eb_parse::{
    filter_toolchain, lock_from_candidates, parse_easyconfig_file, parse_easyconfig_tree,
    validate_lock_deps,
};
pub use sbom::{
    build_dep_map_from_universe, dep_map_from_universe, lock_to_cyclonedx,
    lock_to_cyclonedx_with_deps,
};
pub use select::{select_stack, SelectError};

use anyhow::{bail, Context, Result};
use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use thiserror::Error;

use crate::version::cmp_version;

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

/// Errors when choosing which toolchain generation is the baseline.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BaselineGenError {
    /// `--baseline-toolchain-version` (or equivalent) is not present in the tree.
    #[error(
        "baseline toolchain version {0} not found among same-family generations in the baseline tree"
    )]
    ExplicitNotFound(String),
    /// Default rule needs a generation strictly lower than the policy target.
    #[error(
        "no toolchain generation lower than target {target} among {found:?}; \
         pass --baseline-toolchain-version or provide a lower generation in the baseline tree"
    )]
    NoLowerGeneration { target: String, found: Vec<String> },
}

/// Choose the baseline toolchain **version** for a same-family multi-generation tree.
///
/// # Rule
/// 1. If `explicit` is set, that version must appear in `versions` and is used as-is.
/// 2. Otherwise, select the **nearest lower** generation than `target_version` under
///    [`cmp_version`] ordering (greatest version strictly less than the target).
///
/// This replaces an earlier “first non-target in `BTreeSet` sort order” pick, which is
/// wrong when more than one non-target generation is present (e.g. `2024b` before `2025a`
/// when the target is `2025b`).
pub fn select_baseline_generation(
    versions: impl IntoIterator<Item = impl AsRef<str>>,
    target_version: &str,
    explicit: Option<&str>,
) -> std::result::Result<String, BaselineGenError> {
    let mut vers: Vec<String> = versions
        .into_iter()
        .map(|v| v.as_ref().to_string())
        .collect();
    vers.sort_by(|a, b| cmp_version(a, b));
    vers.dedup();

    if let Some(ex) = explicit {
        if vers.iter().any(|v| v == ex) {
            return Ok(ex.to_string());
        }
        return Err(BaselineGenError::ExplicitNotFound(ex.to_string()));
    }

    // Nearest lower: greatest version strictly less than target (list is sorted ascending).
    let mut best: Option<String> = None;
    for v in &vers {
        if cmp_version(v, target_version) == Ordering::Less {
            best = Some(v.clone());
        }
    }
    best.ok_or_else(|| BaselineGenError::NoLowerGeneration {
        target: target_version.to_string(),
        found: vers,
    })
}

/// Filter baseline candidates to one generation of the policy toolchain family.
///
/// When the tree only contains the policy target version (single generation), candidates
/// are left unchanged. When other versions of the same toolchain name exist, applies
/// [`select_baseline_generation`] (optional explicit override, else nearest lower).
pub fn filter_baseline_candidates(
    base_cands: &[Candidate],
    policy_toolchain: &Toolchain,
    explicit_baseline_version: Option<&str>,
) -> Result<Vec<Candidate>> {
    let family: Vec<&Candidate> = base_cands
        .iter()
        .filter(|c| c.toolchain.name == policy_toolchain.name)
        .collect();
    if family.is_empty() {
        return Ok(base_cands.to_vec());
    }

    let versions: Vec<String> = {
        let mut v: Vec<String> = family
            .iter()
            .map(|c| c.toolchain.version.clone())
            .collect();
        v.sort_by(|a, b| cmp_version(a, b));
        v.dedup();
        v
    };

    let only_target = versions.len() == 1 && versions[0] == policy_toolchain.version;
    if only_target && explicit_baseline_version.is_none() {
        return Ok(base_cands.to_vec());
    }

    // Multi-generation (or explicit override): pick one version of this family.
    if versions.len() > 1
        || explicit_baseline_version.is_some()
        || versions.iter().any(|v| v != &policy_toolchain.version)
    {
        let bv = select_baseline_generation(
            versions.iter().map(|s| s.as_str()),
            &policy_toolchain.version,
            explicit_baseline_version,
        )?;
        Ok(base_cands
            .iter()
            .filter(|c| {
                c.toolchain.name == policy_toolchain.name && c.toolchain.version == bv
            })
            .cloned()
            .collect())
    } else {
        Ok(base_cands.to_vec())
    }
}

/// Parse easyconfigs dir, filter to policy toolchain, solve with resolvo, write lock+SBOM.
///
/// When `baseline_easyconfigs` is set and that tree holds multiple generations of the
/// policy toolchain family, the baseline generation is chosen by
/// [`select_baseline_generation`]: optional `baseline_toolchain_version` override, else
/// the nearest lower generation than the policy target.
pub fn solve_from_easyconfigs(
    easyconfigs_root: &Path,
    policy_path: &Path,
    baseline_easyconfigs: Option<&Path>,
    lock_out: &Path,
    sbom_out: &Path,
) -> Result<StackLock> {
    solve_from_easyconfigs_with_baseline_version(
        easyconfigs_root,
        policy_path,
        baseline_easyconfigs,
        None,
        lock_out,
        sbom_out,
    )
}

/// Like [`solve_from_easyconfigs`], with an optional explicit baseline toolchain version.
pub fn solve_from_easyconfigs_with_baseline_version(
    easyconfigs_root: &Path,
    policy_path: &Path,
    baseline_easyconfigs: Option<&Path>,
    baseline_toolchain_version: Option<&str>,
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
        let base_cands = filter_baseline_candidates(
            &base_all,
            &policy.toolchain,
            baseline_toolchain_version,
        )?;
        if base_cands.is_empty() {
            bail!(
                "no baseline easyconfigs for toolchain family {} after generation filter under {}",
                policy.toolchain.name,
                base_root.display()
            );
        }
        Some(lock_from_candidates(
            &base_cands,
            Some(format!(
                "baseline-from-eb-{}-{}",
                base_cands[0].toolchain.name, base_cands[0].toolchain.version
            )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn multi_gen_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/multi_gen_baseline")
    }

    fn two_gen_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next")
    }

    #[test]
    fn select_baseline_nearest_lower_not_first_in_sort_order() {
        // BTree / lexicographic first non-target would be 2024b; nearest lower is 2025a.
        let chosen = select_baseline_generation(["2024b", "2025a", "2025b"], "2025b", None)
            .expect("nearest lower");
        assert_eq!(chosen, "2025a");
        assert_ne!(chosen, "2024b");
    }

    #[test]
    fn select_baseline_explicit_override() {
        let chosen =
            select_baseline_generation(["2024b", "2025a", "2025b"], "2025b", Some("2024b"))
                .expect("explicit");
        assert_eq!(chosen, "2024b");
    }

    #[test]
    fn select_baseline_explicit_missing_errors() {
        let err = select_baseline_generation(["2025a", "2025b"], "2025b", Some("2024b"))
            .unwrap_err();
        assert!(matches!(err, BaselineGenError::ExplicitNotFound(_)));
    }

    #[test]
    fn select_baseline_no_lower_errors() {
        let err = select_baseline_generation(["2025b", "2026a"], "2025b", None).unwrap_err();
        assert!(matches!(err, BaselineGenError::NoLowerGeneration { .. }));
    }

    #[test]
    fn filter_baseline_candidates_picks_nearest_lower_on_multi_gen_tree() {
        let root = multi_gen_root().join("easyconfigs");
        let all = parse_easyconfig_tree(&root).expect("parse multi-gen");
        let policy_tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let filtered = filter_baseline_candidates(&all, &policy_tc, None).expect("filter");
        assert!(
            !filtered.is_empty(),
            "expected baseline candidates for nearest lower gen"
        );
        assert!(
            filtered
                .iter()
                .all(|c| c.toolchain.name == "foss" && c.toolchain.version == "2025a"),
            "expected only foss-2025a, got {:?}",
            filtered
                .iter()
                .map(|c| format!("{}-{}", c.toolchain.name, c.toolchain.version))
                .collect::<Vec<_>>()
        );
        // Must not be the first-in-sort-order generation (2024b).
        assert!(filtered.iter().all(|c| c.toolchain.version != "2024b"));
    }

    /// Full solve-with-baseline path on a tree with three foss generations.
    ///
    /// Fixture design: `foss-2024b` is first in sort order among non-targets and carries
    /// GROMACS 2025.0 (same as the newest target candidate). Selecting 2024b as baseline
    /// would make `require_upgrade` relative_to_baseline unsatisfiable. Nearest lower
    /// (`2025a`, GROMACS 2024.1) allows the upgrade to 2025.0 — so a successful solve pins
    /// the documented default rule, not first-non-target-in-sort-order.
    #[test]
    fn solve_multi_gen_baseline_uses_nearest_lower_generation() {
        let root = multi_gen_root();
        let easyconfigs = root.join("easyconfigs");
        let policy = root.join("policies/prefer_newer.json");
        let tmp = tempfile::tempdir().unwrap();
        let lock_out = tmp.path().join("stack.lock.json");
        let sbom_out = tmp.path().join("stack.cdx.json");

        let lock = solve_from_easyconfigs_with_baseline_version(
            &easyconfigs,
            &policy,
            Some(&easyconfigs),
            None,
            &lock_out,
            &sbom_out,
        )
        .expect("solve with multi-gen baseline must pick nearest lower (2025a), not 2024b");

        assert_eq!(lock.toolchain.version, "2025b");
        assert_eq!(lock.package("GROMACS").unwrap().version, "2025.0");
        assert_eq!(lock.solver.engine, "resolvo_cdcl_sat");

        // Explicitly re-drive the same generation filter the solve path used and pin it.
        let all = parse_easyconfig_tree(&easyconfigs).unwrap();
        let filtered = filter_baseline_candidates(
            &all,
            &Toolchain {
                name: "foss".into(),
                version: "2025b".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            filtered.first().map(|c| c.toolchain.version.as_str()),
            Some("2025a")
        );
        assert_eq!(
            filtered
                .iter()
                .find(|c| c.name == "GROMACS")
                .map(|c| c.version.as_str()),
            Some("2024.1"),
            "baseline GROMACS must be from 2025a, not 2024b's 2025.0"
        );
    }

    /// Explicit override selects a named generation on the real solve path.
    /// Overriding to 2024b (GROMACS already at 2025.0) must fail require_upgrade.
    #[test]
    fn solve_multi_gen_explicit_baseline_override_to_poisoned_gen_fails() {
        let root = multi_gen_root();
        let easyconfigs = root.join("easyconfigs");
        let policy = root.join("policies/prefer_newer.json");
        let tmp = tempfile::tempdir().unwrap();
        let err = solve_from_easyconfigs_with_baseline_version(
            &easyconfigs,
            &policy,
            Some(&easyconfigs),
            Some("2024b"),
            &tmp.path().join("lock.json"),
            &tmp.path().join("sbom.json"),
        )
        .expect_err("baseline 2024b has GROMACS 2025.0; require_upgrade should unsat");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("unsatisfiable")
                || msg.contains("unsat")
                || msg.contains("no candidate")
                || msg.contains("newer than baseline"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn solve_multi_gen_explicit_baseline_2025a_succeeds() {
        let root = multi_gen_root();
        let easyconfigs = root.join("easyconfigs");
        let policy = root.join("policies/prefer_newer.json");
        let tmp = tempfile::tempdir().unwrap();
        let lock = solve_from_easyconfigs_with_baseline_version(
            &easyconfigs,
            &policy,
            Some(&easyconfigs),
            Some("2025a"),
            &tmp.path().join("lock.json"),
            &tmp.path().join("sbom.json"),
        )
        .expect("explicit 2025a baseline");
        assert_eq!(lock.package("GROMACS").unwrap().version, "2025.0");
    }

    /// Two-generation tree (existing fixture) still solves under the nearest-lower rule.
    #[test]
    fn solve_two_gen_baseline_still_works() {
        let root = two_gen_root();
        let easyconfigs = root.join("easyconfigs");
        let policy = root.join("policies/prefer_newer.json");
        let tmp = tempfile::tempdir().unwrap();
        let lock = solve_from_easyconfigs(
            &easyconfigs,
            &policy,
            Some(&easyconfigs),
            &tmp.path().join("lock.json"),
            &tmp.path().join("sbom.json"),
        )
        .expect("two-gen tree");
        assert_eq!(lock.package("GROMACS").unwrap().version, "2025.0");
        assert_eq!(lock.package("OpenBLAS").unwrap().version, "0.3.27");
    }
}
