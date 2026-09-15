//! Stack selection via resolvo CDCL SAT (fed by parsed easyconfig candidates).
//!
//! Also provides [`resolvo_resolve_dep_versions`]: joint SAT co-select of
//! dependency pins for a single recipe (used by canonical package planning
//! and bumps), so version selection is not hierarchy-lookup-only.

use crate::domain::{
    Candidate, DepReq, LockPackage, Policy, SolverMeta, StackLock, Toolchain, Universe,
    STACK_LOCK_SCHEMA_VERSION,
};
use crate::hierarchy::{
    filter_candidates_in_hierarchy, is_system_toolchain, toolchains_match, SourceDepSpec,
    ToolchainHierarchy,
};
use crate::resolvo_provider::solve_with_resolvo;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
/// Why a co-selection could not be produced.
pub enum SelectError {
    /// No candidate in the tree provides the named package.
    #[error("no candidates for package {0}")]
    MissingPackage(String),
    /// The requested set has no simultaneously satisfiable solution.
    #[error("unsatisfiable stack: {0}")]
    Unsat(String),
    /// The solver returned a diagnostic; the text is already complete.
    #[error("{0}")]
    Solver(String),
}

/// Select a stack using **resolvo** (CDCL SAT) over EasyBuild-derived candidates.
pub fn select_stack(
    universe: &Universe,
    policy: &Policy,
    baseline: Option<&StackLock>,
) -> Result<StackLock, SelectError> {
    let selected =
        solve_with_resolvo(&universe.candidates, policy, baseline).map_err(SelectError::Solver)?;

    let mut packages_out: Vec<LockPackage> = selected
        .into_iter()
        .map(|c| LockPackage {
            name: c.name,
            version: c.version,
            toolchain: c.toolchain,
            versionsuffix: c.versionsuffix,
            easyconfig_path: c.easyconfig_path,
        })
        .collect();
    packages_out.sort_by(lock_package_identity_cmp);

    for root in &policy.roots {
        if !packages_out.iter().any(|p| &p.name == root) {
            return Err(SelectError::MissingPackage(root.clone()));
        }
    }

    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    Ok(StackLock {
        schema_version: STACK_LOCK_SCHEMA_VERSION,
        toolchain: policy.toolchain.clone(),
        generation_label: universe.generation_label.clone(),
        packages: packages_out,
        solver: SolverMeta {
            engine: "resolvo_cdcl_sat".into(),
            engine_version: format!("resolvo+eb_stack-{}", env!("CARGO_PKG_VERSION")),
            timestamp: ts,
        },
    })
}

/// Joint resolvo co-select of dependency versions for one recipe.
///
/// Builds a synthetic root candidate whose dependencies are the foreign or
/// source-recipe deps that exist under the generation hierarchy, solves with
/// [`select_stack`], and returns name→version for co-selected deps plus a
/// human note (engine id). Hierarchy membership is already enforced by
/// [`filter_candidates_in_hierarchy`]; candidate toolchains stay as they
/// were so SAT identity keeps GCCcore vs foss of the same name distinct.
///
/// When `preferred_pins` is set (typically hierarchy consensus), those packages
/// are **exact pins** in the policy: resolvo joint-checks feasibility under
/// generation-native versions rather than free prefer_newer overriding
/// consensus (which would re-select SYSTEM/out-of-generation newest).
/// Unpinned resolvable specs still free-select under residual floors.
///
/// Specs with SYSTEM toolchain or a non-empty versionsuffix are skipped
/// (frozen pins). Pure SYSTEM install candidates are dropped when a non-SYSTEM
/// hierarchy candidate for the same name exists.
pub fn resolvo_resolve_dep_versions(
    specs: &[SourceDepSpec],
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
    toolchain: &Toolchain,
    root_name: &str,
    root_version: &str,
    preferred_pins: Option<&HashMap<String, String>>,
) -> Result<(HashMap<String, String>, String), String> {
    let universe_cands = filter_candidates_in_hierarchy(cands, hierarchy);
    if universe_cands.is_empty() {
        return Err("no candidates under hierarchy members".into());
    }
    let mut by_name: HashMap<&str, Vec<&Candidate>> = HashMap::new();
    for candidate in &universe_cands {
        by_name
            .entry(candidate.name.as_str())
            .or_default()
            .push(candidate);
    }
    let mut dep_reqs: Vec<DepReq> = Vec::new();
    let mut resolvable: HashSet<String> = HashSet::new();
    let mut pins: Vec<crate::domain::Pin> = Vec::new();
    let mut optional_names: HashSet<String> = HashSet::new();
    let mut parent_floors: HashMap<String, String> = HashMap::new();
    for s in specs {
        if s.system_toolchain {
            continue;
        }
        if s.versionsuffix.as_deref().is_some_and(|vs| !vs.is_empty()) {
            continue;
        }
        let Some(named) = by_name.get(s.name.as_str()) else {
            if s.optional {
                continue;
            }
            return Err(format!(
                "no hierarchy candidate for required dep {}",
                s.name
            ));
        };

        let (mut version_req, pin_exact) =
            if let Some(pref) = preferred_pins.and_then(|m| m.get(&s.name)) {
                // Hierarchy consensus: exact pin, joint SAT under that version.
                let req = format!("=={pref}");
                (req.clone(), Some(pref.clone()))
            } else if s.version == "0.0.0" || s.version.is_empty() {
                (">=0".into(), None)
            } else {
                (format!(">={}", s.version), None)
            };

        let unsuffixed =
            |candidate: &&Candidate| candidate.versionsuffix.as_deref().unwrap_or("").is_empty();
        let parsed = crate::version::parse_requirement(&version_req).ok();
        let floor_match = named.iter().any(|candidate| {
            unsuffixed(&candidate)
                && parsed
                    .as_ref()
                    .is_some_and(|requirement| requirement.matches(&candidate.version))
        });
        let parent_match = named.iter().any(|candidate| {
            unsuffixed(&candidate) && toolchains_match(&hierarchy.parent, &candidate.toolchain)
        });
        if !floor_match && !parent_match {
            if s.optional {
                continue;
            }
            return Err(format!(
                "no candidate matching {} {} without a versionsuffix",
                s.name, version_req
            ));
        }
        if pin_exact.is_none() && parent_match {
            // Parent-toolchain builds are generation-authoritative, not a
            // downgrade. SAT must not keep the source floor as a hard req
            // on those rows; other members of the name stay on the floor.
            parent_floors.insert(s.name.clone(), version_req.clone());
            version_req = ">=0".into();
        }
        if let Some(ver) = pin_exact {
            pins.push(crate::domain::Pin {
                name: s.name.clone(),
                version_req: format!("=={ver}"),
            });
        }
        dep_reqs.push(DepReq {
            name: s.name.clone(),
            version_req,
            // Empty suffix is the unsuffixed module. None would let a CUDA
            // variant win prefer_newer at the same version.
            versionsuffix: Some(String::new()),
            toolchain: None,
        });
        resolvable.insert(s.name.clone());
        if s.optional {
            optional_names.insert(s.name.clone());
        }
    }
    if dep_reqs.is_empty() {
        return Ok((
            HashMap::new(),
            "no unfrozen deps; caller keeps source pins".into(),
        ));
    }

    let admitted: Vec<Candidate> = universe_cands
        .into_iter()
        .filter(|candidate| {
            if let Some(floor) = parent_floors.get(&candidate.name) {
                if toolchains_match(&hierarchy.parent, &candidate.toolchain) {
                    return true;
                }
                return crate::version::parse_requirement(floor)
                    .map(|requirement| requirement.matches(&candidate.version))
                    .unwrap_or(false);
            }
            if let Some(dep) = dep_reqs.iter().find(|dep| dep.name == candidate.name) {
                return crate::version::parse_requirement(&dep.version_req)
                    .map(|requirement| requirement.matches(&candidate.version))
                    .unwrap_or(true);
            }
            true
        })
        .collect();
    // Drop SYSTEM only among floor/parent-eligible rows, the same order as
    // prefer_non_system_candidates. A below-floor GCCcore sibling must not
    // hide a SYSTEM install that meets the source floor.
    let admitted = drop_system_when_non_system_exists(admitted);
    let solve = |dep_reqs: Vec<DepReq>,
                 resolvable: HashSet<String>,
                 pins: Vec<crate::domain::Pin>|
     -> Result<(HashMap<String, String>, String), String> {
        let mut keep: HashSet<String> = resolvable.iter().cloned().collect();
        let mut pending: Vec<String> = keep.iter().cloned().collect();
        while let Some(name) = pending.pop() {
            for candidate in admitted.iter().filter(|candidate| candidate.name == name) {
                for dep in candidate
                    .dependencies
                    .iter()
                    .chain(&candidate.builddependencies)
                {
                    if keep.insert(dep.name.clone()) {
                        pending.push(dep.name.clone());
                    }
                }
            }
        }
        let mut universe_cands: Vec<Candidate> = admitted
            .iter()
            .filter(|candidate| keep.contains(&candidate.name))
            .cloned()
            .collect();

        let synthetic = if universe_cands.iter().any(|c| c.name == root_name) {
            format!("__bump__{root_name}")
        } else {
            root_name.to_string()
        };

        universe_cands.push(Candidate {
            name: synthetic.clone(),
            version: root_version.to_string(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            easyconfig_path: format!("__bump__/{synthetic}-{root_version}.eb"),
            dependencies: dep_reqs,
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        });

        let universe = Universe {
            toolchain: toolchain.clone(),
            generation_label: Some(format!("bump-{}-{}", toolchain.name, toolchain.version)),
            candidates: universe_cands,
        };
        let policy = Policy {
            prefer_installed: false,
            toolchain: toolchain.clone(),
            roots: vec![synthetic.clone()],
            root_priority: None,
            pins,
            forbid: Vec::new(),
            objective: "prefer_newer".into(),
            require_upgrade: Vec::new(),
        };

        let lock = select_stack(&universe, &policy, None).map_err(|e| e.to_string())?;
        let mut map = HashMap::new();
        for p in &lock.packages {
            if p.name == synthetic {
                continue;
            }
            if resolvable.contains(&p.name) {
                if p.versionsuffix.as_deref().is_some_and(|vs| !vs.is_empty()) {
                    continue;
                }
                map.insert(p.name.clone(), p.version.clone());
            }
        }
        for name in &resolvable {
            if !map.contains_key(name) {
                return Err(format!(
                    "selected {name} but the spec asked for the unsuffixed module"
                ));
            }
        }
        if map.is_empty() {
            return Err("resolvo lock had no co-selected deps".into());
        }
        let note = format!(
            "resolvo joint co-selected {} dep(s) via {} ({})",
            map.len(),
            lock.solver.engine,
            lock.solver.engine_version
        );
        Ok((map, note))
    };

    match solve(dep_reqs.clone(), resolvable.clone(), pins.clone()) {
        Ok(ok) => Ok(ok),
        Err(_) if !optional_names.is_empty() => {
            let required_reqs: Vec<DepReq> = dep_reqs
                .iter()
                .filter(|dep| !optional_names.contains(&dep.name))
                .cloned()
                .collect();
            let required_names: HashSet<String> = resolvable
                .iter()
                .filter(|name| !optional_names.contains(*name))
                .cloned()
                .collect();
            let required_pins: Vec<crate::domain::Pin> = pins
                .iter()
                .filter(|pin| !optional_names.contains(&pin.name))
                .cloned()
                .collect();
            let (mut kept, mut trial_reqs, mut trial_names, mut trial_pins) =
                if required_reqs.is_empty() {
                    (
                        (HashMap::new(), "optional extras were unsatisfiable".into()),
                        Vec::new(),
                        HashSet::new(),
                        Vec::new(),
                    )
                } else {
                    let kept = solve(
                        required_reqs.clone(),
                        required_names.clone(),
                        required_pins.clone(),
                    )?;
                    (kept, required_reqs, required_names, required_pins)
                };
            let mut extras: Vec<String> = optional_names.into_iter().collect();
            extras.sort();
            for extra in extras {
                let Some(dep) = dep_reqs.iter().find(|dep| dep.name == extra).cloned() else {
                    continue;
                };
                let mut next_reqs = trial_reqs.clone();
                next_reqs.push(dep);
                let mut next_names = trial_names.clone();
                next_names.insert(extra.clone());
                let mut next_pins = trial_pins.clone();
                if let Some(pin) = pins.iter().find(|pin| pin.name == extra) {
                    next_pins.push(pin.clone());
                }
                if let Ok(map) = solve(next_reqs.clone(), next_names.clone(), next_pins.clone()) {
                    trial_reqs = next_reqs;
                    trial_names = next_names;
                    trial_pins = next_pins;
                    kept = map;
                }
            }
            Ok(kept)
        }
        Err(error) => Err(error),
    }
}

fn lock_package_identity_cmp(a: &LockPackage, b: &LockPackage) -> std::cmp::Ordering {
    let left = a.identity_key();
    let right = b.identity_key();
    left.name
        .cmp(&right.name)
        .then_with(|| left.toolchain.cmp(&right.toolchain))
        .then_with(|| left.version.cmp(&right.version))
        .then_with(|| left.versionsuffix.cmp(&right.versionsuffix))
}

/// Prefer non-SYSTEM install candidates when both SYSTEM and non-SYSTEM exist
/// for the same package name (EasyBuild generation installs over bare SYSTEM).
fn drop_system_when_non_system_exists(cands: Vec<Candidate>) -> Vec<Candidate> {
    let mut has_non_sys: HashSet<(String, String)> = HashSet::new();
    for c in &cands {
        if !is_system_toolchain(&c.toolchain) {
            has_non_sys.insert((c.name.clone(), c.versionsuffix.clone().unwrap_or_default()));
        }
    }
    cands
        .into_iter()
        .filter(|c| {
            if is_system_toolchain(&c.toolchain)
                && has_non_sys
                    .contains(&(c.name.clone(), c.versionsuffix.clone().unwrap_or_default()))
            {
                return false;
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::eb_parse::{filter_toolchain, lock_from_candidates, parse_easyconfig_tree};
    use crate::version::cmp_version;
    use std::cmp::Ordering;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next")
    }

    fn load_policy(name: &str) -> Policy {
        let p = fixture_root().join("policies").join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn universe_next_from_eb() -> (Universe, StackLock) {
        let root = fixture_root().join("easyconfigs");
        let all = parse_easyconfig_tree(&root).expect("parse tree").candidates;
        let policy_tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let next = filter_toolchain(&all, &policy_tc);
        let base_tc = Toolchain {
            name: "foss".into(),
            version: "2025a".into(),
        };
        let base = filter_toolchain(&all, &base_tc);
        let baseline = lock_from_candidates(&base, Some("2025a-baseline".into()), "eb_parse");
        let universe = Universe {
            toolchain: policy_tc,
            generation_label: Some("foss-2025b".into()),
            candidates: next,
        };
        (universe, baseline)
    }

    #[test]
    fn parse_then_resolvo_upgrades_gromacs_from_2025a_to_2025b() {
        let (universe, baseline) = universe_next_from_eb();
        assert_eq!(baseline.package("GROMACS").unwrap().version, "2024.1");
        let policy = load_policy("prefer_newer.json");
        assert_eq!(policy.roots, vec!["GROMACS".to_string()]);
        let lock = select_stack(&universe, &policy, Some(&baseline)).expect("resolvo");
        assert_eq!(lock.solver.engine, "resolvo_cdcl_sat");
        let g = lock.package("GROMACS").unwrap();
        assert_eq!(g.version, "2025.0");
        assert!(g.easyconfig_path.ends_with(".eb"));
        assert_eq!(cmp_version(&g.version, "2024.1"), Ordering::Greater);
        // co-selected from parsed dependencies, not hard-coded roots
        assert_eq!(lock.package("OpenBLAS").unwrap().version, "0.3.27");
        assert_eq!(lock.package("OpenMPI").unwrap().version, "5.0.3");
        assert!(lock.package("FFTW").is_some());
        assert_eq!(lock.package("Python").unwrap().version, "3.12.3");
    }

    #[test]
    fn parse_then_pin_changes_solution() {
        let (universe, baseline) = universe_next_from_eb();
        let free = load_policy("prefer_newer.json");
        let pin = load_policy("pin_openblas.json");
        let a = select_stack(&universe, &free, Some(&baseline)).unwrap();
        let b = select_stack(&universe, &pin, Some(&baseline)).unwrap();
        assert_ne!(
            a.package("GROMACS").unwrap().version,
            b.package("GROMACS").unwrap().version
        );
        assert_eq!(b.package("GROMACS").unwrap().version, "2024.4");
        assert_eq!(b.package("OpenBLAS").unwrap().version, "0.3.24");
    }

    #[test]
    fn parse_then_unsat() {
        let (universe, _) = universe_next_from_eb();
        let policy = load_policy("unsat.json");
        let err = select_stack(&universe, &policy, None).unwrap_err();
        let msg = err.to_string();
        let low = msg.to_lowercase();
        assert!(
            low.contains("unsatisfiable") || low.contains("unsat"),
            "{msg}"
        );
        // Human-readable versions, not raw resolvo ranks ("GROMACS 2", "OpenMPI 1 | 2").
        assert!(
            msg.contains("2025.0") && (msg.contains("4.1.5") || msg.contains("5.0.3")),
            "unsat must name real package versions, got: {msg}"
        );
        assert!(
            msg.contains("GROMACS") && msg.contains("OpenMPI"),
            "unsat must name packages, got: {msg}"
        );
        assert!(
            !msg.contains("OpenMPI 1 | 2") && !msg.contains("GROMACS 2 cannot"),
            "unsat must not leak rank ids: {msg}"
        );
    }

    /// Co-select a package that is only linked via `builddependencies` (no runtime edge).
    #[test]
    fn co_select_via_builddependencies_only() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        // Leaf has no deps; root links to it only as a build dep.
        let leaf = Candidate {
            name: "BuildTool".into(),
            version: "2.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "BuildTool-2.0-foss-2025b.eb".into(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        };
        let root = Candidate {
            name: "App".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "App-1.0-foss-2025b.eb".into(),
            dependencies: vec![],
            builddependencies: vec![DepReq {
                name: "BuildTool".into(),
                version_req: ">=2.0".into(),
                versionsuffix: None,
                toolchain: None,
            }],
            exts_list: vec![],
            moduleclass: None,
        };
        let universe = Universe {
            toolchain: tc.clone(),
            generation_label: Some("builddep-co-select".into()),
            candidates: vec![root, leaf],
        };
        let policy = Policy {
            prefer_installed: false,
            toolchain: tc,
            roots: vec!["App".into()],
            root_priority: None,
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: vec![],
        };
        let lock = select_stack(&universe, &policy, None).expect("solve via builddependencies");
        assert!(
            lock.package("BuildTool").is_some(),
            "BuildTool must co-select from builddependencies only; packages={:?}",
            lock.packages
                .iter()
                .map(|p| format!("{}={}", p.name, p.version))
                .collect::<Vec<_>>()
        );
        assert_eq!(lock.package("BuildTool").unwrap().version, "2.0");
        assert_eq!(lock.package("App").unwrap().version, "1.0");
    }

    #[test]
    fn unsat_when_builddependency_missing() {
        let tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let root = Candidate {
            name: "App".into(),
            version: "1.0".into(),
            toolchain: tc.clone(),
            versionsuffix: None,
            easyconfig_path: "App-1.0-foss-2025b.eb".into(),
            dependencies: vec![],
            builddependencies: vec![DepReq {
                name: "MissingTool".into(),
                version_req: "==1.0".into(),
                versionsuffix: None,
                toolchain: None,
            }],
            exts_list: vec![],
            moduleclass: None,
        };
        let universe = Universe {
            toolchain: tc.clone(),
            generation_label: None,
            candidates: vec![root],
        };
        let policy = Policy {
            prefer_installed: false,
            toolchain: tc,
            roots: vec!["App".into()],
            root_priority: None,
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: vec![],
        };
        let err = select_stack(&universe, &policy, None).unwrap_err();
        let msg = err.to_string();
        let low = msg.to_lowercase();
        assert!(
            low.contains("unsatisfiable") || low.contains("unsat") || low.contains("missing"),
            "builddep miss should fail like runtime dep miss: {msg}"
        );
    }

    #[test]
    fn fixture_builddep_root_co_selects_fftw() {
        let (universe, _) = universe_next_from_eb();
        assert!(
            universe.candidates.iter().any(|c| c.name == "BuildDepRoot"),
            "BuildDepRoot fixture must be in parsed universe"
        );
        let root = universe
            .candidates
            .iter()
            .find(|c| c.name == "BuildDepRoot")
            .unwrap();
        assert!(
            root.builddependencies.iter().any(|d| d.name == "FFTW"),
            "fixture must declare FFTW as builddependency"
        );
        assert!(
            !root.dependencies.iter().any(|d| d.name == "FFTW"),
            "FFTW must not be a runtime dep on the fixture"
        );
        let policy = Policy {
            prefer_installed: false,
            toolchain: universe.toolchain.clone(),
            roots: vec!["BuildDepRoot".into()],
            root_priority: None,
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: vec![],
        };
        let lock = select_stack(&universe, &policy, None).expect("BuildDepRoot solve");
        assert!(
            lock.package("FFTW").is_some(),
            "FFTW co-selected via builddependencies; packages={:?}",
            lock.packages.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        assert!(lock.package("OpenBLAS").is_some());
    }

    // --- Multi-root shared-dep conflict: priority, not list order, decides ---

    fn two_root_fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/two_root_openmpi_conflict")
    }

    fn load_two_root_policy(name: &str) -> Policy {
        let p = two_root_fixture_root().join("policies").join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn two_root_universe() -> Universe {
        let root = two_root_fixture_root().join("easyconfigs");
        let all = parse_easyconfig_tree(&root)
            .expect("parse two-root tree")
            .candidates;
        let policy_tc = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let cands = filter_toolchain(&all, &policy_tc);
        Universe {
            toolchain: policy_tc,
            generation_label: Some("foss-2025b-two-root".into()),
            candidates: cands,
        }
    }

    /// Newest GROMACS pins OpenMPI==4.1.6; newest LAMMPS needs OpenMPI>=5.0.3.
    /// Declared priority for GROMACS must keep GROMACS 2025.0 and yield LAMMPS,
    /// for both root list orders.
    #[test]
    fn multi_root_priority_gromacs_stable_under_root_list_order() {
        let universe = two_root_universe();
        for policy_name in [
            "priority_gromacs_roots_gromacs_first.json",
            "priority_gromacs_roots_lammps_first.json",
        ] {
            let policy = load_two_root_policy(policy_name);
            assert_eq!(
                policy.effective_root_priority().expect("priority"),
                vec!["GROMACS".to_string(), "LAMMPS".to_string()],
                "{policy_name}"
            );
            let lock = select_stack(&universe, &policy, None)
                .unwrap_or_else(|e| panic!("{policy_name}: {e}"));
            assert_eq!(
                lock.package("GROMACS").unwrap().version,
                "2025.0",
                "{policy_name}: prioritized GROMACS must keep newest"
            );
            assert_eq!(
                lock.package("LAMMPS").unwrap().version,
                "2023.08",
                "{policy_name}: non-prioritized LAMMPS must yield"
            );
            assert_eq!(
                lock.package("OpenMPI").unwrap().version,
                "4.1.6",
                "{policy_name}: shared dep forced by GROMACS exact pin"
            );
        }
    }

    /// Declared priority for LAMMPS must keep LAMMPS 2024.08 and yield GROMACS,
    /// for both root list orders.
    #[test]
    fn multi_root_priority_lammps_stable_under_root_list_order() {
        let universe = two_root_universe();
        for policy_name in [
            "priority_lammps_roots_gromacs_first.json",
            "priority_lammps_roots_lammps_first.json",
        ] {
            let policy = load_two_root_policy(policy_name);
            assert_eq!(
                policy.effective_root_priority().expect("priority"),
                vec!["LAMMPS".to_string(), "GROMACS".to_string()],
                "{policy_name}"
            );
            let lock = select_stack(&universe, &policy, None)
                .unwrap_or_else(|e| panic!("{policy_name}: {e}"));
            assert_eq!(
                lock.package("LAMMPS").unwrap().version,
                "2024.08",
                "{policy_name}: prioritized LAMMPS must keep newest"
            );
            assert_eq!(
                lock.package("GROMACS").unwrap().version,
                "2024.4",
                "{policy_name}: non-prioritized GROMACS must yield"
            );
            assert_eq!(
                lock.package("OpenMPI").unwrap().version,
                "5.0.3",
                "{policy_name}: shared dep forced by newest LAMMPS"
            );
        }
    }

    /// Omitting `root_priority` defaults priority to roots list order.
    #[test]
    fn multi_root_default_priority_follows_roots_list_order() {
        let universe = two_root_universe();

        let g_first = load_two_root_policy("default_priority_gromacs_listed_first.json");
        assert!(g_first.root_priority.is_none());
        assert_eq!(
            g_first.effective_root_priority().expect("priority"),
            vec!["GROMACS".to_string(), "LAMMPS".to_string()]
        );
        let lock_g = select_stack(&universe, &g_first, None).expect("default gromacs-first");
        assert_eq!(lock_g.package("GROMACS").unwrap().version, "2025.0");
        assert_eq!(lock_g.package("LAMMPS").unwrap().version, "2023.08");

        let l_first = load_two_root_policy("default_priority_lammps_listed_first.json");
        assert!(l_first.root_priority.is_none());
        assert_eq!(
            l_first.effective_root_priority().expect("priority"),
            vec!["LAMMPS".to_string(), "GROMACS".to_string()]
        );
        let lock_l = select_stack(&universe, &l_first, None).expect("default lammps-first");
        assert_eq!(lock_l.package("LAMMPS").unwrap().version, "2024.08");
        assert_eq!(lock_l.package("GROMACS").unwrap().version, "2024.4");
    }

    /// Candidate list order must not change the priority-optimal selection.
    #[test]
    fn multi_root_priority_stable_under_candidate_shuffle() {
        let mut universe = two_root_universe();
        // Reverse candidate order (incidental input ordering).
        universe.candidates.reverse();
        let policy = load_two_root_policy("priority_gromacs_roots_lammps_first.json");
        let lock = select_stack(&universe, &policy, None).expect("shuffled candidates");
        assert_eq!(lock.package("GROMACS").unwrap().version, "2025.0");
        assert_eq!(lock.package("LAMMPS").unwrap().version, "2023.08");
        assert_eq!(lock.package("OpenMPI").unwrap().version, "4.1.6");

        // Also reverse again after partial sort by path to vary HashMap insert order.
        universe
            .candidates
            .sort_by(|a, b| b.easyconfig_path.cmp(&a.easyconfig_path));
        let lock2 = select_stack(&universe, &policy, None).expect("resorted candidates");
        assert_eq!(lock2.package("GROMACS").unwrap().version, "2025.0");
        assert_eq!(lock2.package("LAMMPS").unwrap().version, "2023.08");
    }

    #[test]
    fn policy_root_priority_deserializes_and_defaults() {
        let with = load_two_root_policy("priority_lammps_roots_gromacs_first.json");
        assert_eq!(
            with.root_priority.as_ref().unwrap(),
            &vec!["LAMMPS".to_string(), "GROMACS".to_string()]
        );
        // Existing single-root policies omit the field and still deserialize.
        let single = load_policy("prefer_newer.json");
        assert!(single.root_priority.is_none());
        assert_eq!(
            single.effective_root_priority().expect("priority"),
            vec!["GROMACS".to_string()]
        );
    }
}

#[cfg(test)]
mod prefer_installed_tests {
    use super::*;
    use crate::domain::*;

    /// Two versions of one package, nothing else, so the only question the
    /// solver answers is which of them to take.
    fn universe_with(versions: &[&str]) -> (Universe, StackLock) {
        let toolchain = Toolchain {
            name: "foss".into(),
            version: "2025a".into(),
        };
        let candidates: Vec<Candidate> = versions
            .iter()
            .map(|version| Candidate {
                name: "Alpha".into(),
                version: (*version).to_string(),
                toolchain: toolchain.clone(),
                versionsuffix: None,
                dependencies: Vec::new(),
                builddependencies: Vec::new(),
                easyconfig_path: format!("a/Alpha/Alpha-{version}-foss-2025a.eb"),
                exts_list: Vec::new(),
                moduleclass: None,
            })
            .collect();
        let universe = Universe {
            toolchain: toolchain.clone(),
            generation_label: Some("foss-2025a".into()),
            candidates: candidates.clone(),
        };
        let installed = StackLock {
            schema_version: STACK_LOCK_SCHEMA_VERSION,
            toolchain,
            generation_label: Some("installed".into()),
            packages: vec![LockPackage {
                name: "Alpha".into(),
                version: versions[0].to_string(),
                toolchain: universe.toolchain.clone(),
                versionsuffix: None,
                easyconfig_path: candidates[0].easyconfig_path.clone(),
            }],
            solver: SolverMeta {
                engine: "eb_parse".into(),
                engine_version: "0".into(),
                timestamp: "2026-08-12T00:00:00Z".into(),
            },
        };
        (universe, installed)
    }

    fn policy(prefer_installed: bool) -> Policy {
        Policy {
            prefer_installed,
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2025a".into(),
            },
            roots: vec!["Alpha".into()],
            root_priority: None,
            pins: Vec::new(),
            forbid: Vec::new(),
            objective: "prefer_newer".into(),
            require_upgrade: Vec::new(),
        }
    }

    #[test]
    fn the_default_objective_still_takes_the_newest() {
        let (universe, installed) = universe_with(&["1.0", "2.0"]);
        let lock = select_stack(&universe, &policy(false), Some(&installed)).expect("solve");
        assert_eq!(lock.package("Alpha").unwrap().version, "2.0");
    }

    /// The point of the flag: a package nobody asked to move does not move,
    /// because on this site moving it costs a rebuild.
    #[test]
    fn prefer_installed_keeps_what_is_already_there() {
        let (universe, installed) = universe_with(&["1.0", "2.0"]);
        let lock = select_stack(&universe, &policy(true), Some(&installed)).expect("solve");
        assert_eq!(lock.package("Alpha").unwrap().version, "1.0");
    }

    #[test]
    fn with_no_baseline_the_flag_changes_nothing() {
        let (universe, _) = universe_with(&["1.0", "2.0"]);
        let lock = select_stack(&universe, &policy(true), None).expect("solve");
        assert_eq!(lock.package("Alpha").unwrap().version, "2.0");
    }

    /// Same version, different build. A versionsuffix makes a different
    /// module, so preferring it would keep nothing that is actually installed.
    #[test]
    fn a_variant_that_differs_only_by_versionsuffix_is_not_what_is_installed() {
        let (universe, mut installed) = universe_with(&["1.0", "2.0"]);
        installed.packages[0].versionsuffix = Some("-CUDA-12.8.0".into());
        let lock = select_stack(&universe, &policy(true), Some(&installed)).expect("solve");
        assert_eq!(lock.package("Alpha").unwrap().version, "2.0");
    }

    #[test]
    fn an_empty_versionsuffix_is_the_same_as_none_when_preferring_installed() {
        let (mut universe, mut installed) = universe_with(&["1.0", "2.0"]);
        universe.candidates[0].versionsuffix = Some(String::new());
        installed.packages[0].versionsuffix = None;
        let lock = select_stack(&universe, &policy(true), Some(&installed)).expect("solve");
        assert_eq!(lock.package("Alpha").unwrap().version, "1.0");
    }

    /// A version that is no longer a candidate cannot be preferred, and the
    /// solve must still succeed rather than hold out for it.
    #[test]
    fn an_installed_version_that_is_gone_falls_back_to_the_newest() {
        let (universe, mut installed) = universe_with(&["1.0", "2.0"]);
        installed.packages[0].version = "0.9".into();
        let lock = select_stack(&universe, &policy(true), Some(&installed)).expect("solve");
        assert_eq!(lock.package("Alpha").unwrap().version, "2.0");
    }

    #[test]
    fn prefer_installed_keeps_a_multi_level_package() {
        let foss = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "14.3.0".into(),
        };
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let perl = |version: &str, toolchain: &Toolchain| Candidate {
            name: "Perl".into(),
            version: version.into(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            easyconfig_path: format!("p/Perl/Perl-{version}.eb"),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let universe = Universe {
            toolchain: foss.clone(),
            generation_label: Some("foss-2025b".into()),
            candidates: vec![
                perl("5.38.0", &gcccore),
                perl("5.42.0", &gcccore),
                perl("5.38.0", &system),
                Candidate {
                    name: "App".into(),
                    version: "1.0".into(),
                    toolchain: foss.clone(),
                    versionsuffix: None,
                    dependencies: vec![DepReq {
                        name: "Perl".into(),
                        version_req: String::new(),
                        toolchain: Some(gcccore.clone()),
                        versionsuffix: None,
                    }],
                    builddependencies: Vec::new(),
                    easyconfig_path: "a/App/App-1.0.eb".into(),
                    exts_list: Vec::new(),
                    moduleclass: None,
                },
            ],
        };
        let installed = StackLock {
            schema_version: STACK_LOCK_SCHEMA_VERSION,
            toolchain: foss.clone(),
            generation_label: Some("installed".into()),
            packages: vec![LockPackage {
                name: "Perl".into(),
                version: "5.38.0".into(),
                toolchain: gcccore,
                versionsuffix: None,
                easyconfig_path: "p/Perl/Perl-5.38.0.eb".into(),
            }],
            solver: SolverMeta {
                engine: "eb_parse".into(),
                engine_version: "0".into(),
                timestamp: "2026-08-12T00:00:00Z".into(),
            },
        };
        let mut pol = policy(true);
        pol.toolchain = foss;
        pol.roots = vec!["App".into()];
        let lock = select_stack(&universe, &pol, Some(&installed)).expect("solve");
        assert_eq!(
            lock.package("Perl").unwrap().version,
            "5.38.0",
            "multi-level Perl must stay at the installed GCCcore build"
        );
    }

    /// Two lock rows for the same name make `StackLock::package` return None.
    /// The root trial must still see the installed version.
    #[test]
    fn prefer_installed_keeps_a_multi_level_root() {
        let foss = Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        };
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "14.3.0".into(),
        };
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let perl = |version: &str, toolchain: &Toolchain| Candidate {
            name: "Perl".into(),
            version: version.into(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            easyconfig_path: format!("p/Perl/Perl-{version}.eb"),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let universe = Universe {
            toolchain: foss.clone(),
            generation_label: Some("foss-2025b".into()),
            candidates: vec![
                perl("5.38.0", &gcccore),
                perl("5.42.0", &gcccore),
                perl("5.38.0", &system),
                perl("5.42.0", &system),
            ],
        };
        let installed = StackLock {
            schema_version: STACK_LOCK_SCHEMA_VERSION,
            toolchain: foss.clone(),
            generation_label: Some("installed".into()),
            packages: vec![
                LockPackage {
                    name: "Perl".into(),
                    version: "5.38.0".into(),
                    toolchain: gcccore,
                    versionsuffix: None,
                    easyconfig_path: "p/Perl/Perl-5.38.0.eb".into(),
                },
                LockPackage {
                    name: "Perl".into(),
                    version: "5.38.0".into(),
                    toolchain: system,
                    versionsuffix: None,
                    easyconfig_path: "p/Perl/Perl-5.38.0.eb".into(),
                },
            ],
            solver: SolverMeta {
                engine: "eb_parse".into(),
                engine_version: "0".into(),
                timestamp: "2026-08-12T00:00:00Z".into(),
            },
        };
        let mut pol = policy(true);
        pol.toolchain = foss;
        pol.roots = vec!["Perl".into()];
        let lock = select_stack(&universe, &pol, Some(&installed)).expect("solve");
        let perl_versions: Vec<&str> = lock
            .packages
            .iter()
            .filter(|package| package.name == "Perl")
            .map(|package| package.version.as_str())
            .collect();
        assert!(
            !perl_versions.is_empty(),
            "Perl must appear in the solved lock"
        );
        assert!(
            perl_versions.iter().all(|version| *version == "5.38.0"),
            "root prefer_installed must keep 5.38 when both levels are in the baseline, got {perl_versions:?}"
        );
    }

    #[test]
    fn prefer_installed_matches_system_spelling_variants() {
        let (mut universe, mut installed) = universe_with(&["1.0", "2.0"]);
        for candidate in &mut universe.candidates {
            candidate.toolchain = Toolchain {
                name: "system".into(),
                version: "system".into(),
            };
        }
        universe.candidates[0].versionsuffix = Some(String::new());
        installed.packages[0].toolchain = Toolchain {
            name: "dummy".into(),
            version: String::new(),
        };
        installed.packages[0].versionsuffix = None;
        let lock = select_stack(&universe, &policy(true), Some(&installed)).expect("solve");
        assert_eq!(
            lock.package("Alpha").unwrap().version,
            "1.0",
            "SYSTEM dummy/empty must match system/system"
        );
    }
}

#[cfg(test)]
mod lock_identity_and_bump_pin_tests {
    use super::*;
    use crate::domain::*;
    use crate::hierarchy::{SourceDepSpec, ToolchainHierarchy};

    fn foss() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2025a".into(),
        }
    }

    fn hierarchy() -> ToolchainHierarchy {
        ToolchainHierarchy {
            parent: foss(),
            members: vec![foss()],
        }
    }

    fn candidate(name: &str, version: &str, suffix: Option<&str>) -> Candidate {
        let toolchain = foss();
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain,
            versionsuffix: suffix.map(str::to_string),
            easyconfig_path: format!("{name}-{version}.eb"),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        }
    }

    #[test]
    fn lock_packages_sort_by_name_toolchain_version_and_suffix() {
        let foss_tc = foss();
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let mut packages = vec![
            LockPackage {
                name: "zlib".into(),
                version: "1.3".into(),
                toolchain: foss_tc.clone(),
                versionsuffix: None,
                easyconfig_path: "zlib-foss.eb".into(),
            },
            LockPackage {
                name: "zlib".into(),
                version: "1.2".into(),
                toolchain: system.clone(),
                versionsuffix: None,
                easyconfig_path: "zlib-system.eb".into(),
            },
            LockPackage {
                name: "zlib".into(),
                version: "1.3".into(),
                toolchain: foss_tc,
                versionsuffix: Some("-CUDA-12.8.0".into()),
                easyconfig_path: "zlib-cuda.eb".into(),
            },
        ];
        packages.sort_by(lock_package_identity_cmp);
        assert_eq!(packages[0].easyconfig_path, "zlib-foss.eb");
        assert_eq!(packages[1].easyconfig_path, "zlib-cuda.eb");
        assert_eq!(packages[2].easyconfig_path, "zlib-system.eb");
    }

    #[test]
    fn lock_identity_collapses_system_spelling_and_empty_suffix() {
        let pkg = |toolchain: Toolchain, version: &str, suffix: Option<&str>| LockPackage {
            name: "binutils".into(),
            version: version.into(),
            toolchain,
            versionsuffix: suffix.map(str::to_string),
            easyconfig_path: format!("binutils-{version}.eb"),
        };
        let dummy = Toolchain {
            name: "dummy".into(),
            version: "dummy".into(),
        };
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        assert_eq!(
            lock_package_identity_cmp(
                &pkg(dummy.clone(), "2.40", Some("")),
                &pkg(system.clone(), "2.40", None)
            ),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            lock_package_identity_cmp(&pkg(dummy, "2.40", None), &pkg(system, "2.42", Some(""))),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn a_cuda_lock_row_does_not_abort_an_unsuffixed_coselect() {
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let foss_tc = foss();
        let mut python = candidate("Python", "3.12.0", None);
        python.toolchain = system.clone();
        python.easyconfig_path = "Python-3.12.0.eb".into();
        let mut python_cuda = candidate("Python", "3.12.0", Some("-CUDA-12.8.0"));
        python_cuda.toolchain = foss_tc.clone();
        let mut scipy = candidate("SciPy", "1.0.0", None);
        scipy.dependencies = vec![DepReq {
            name: "Python".into(),
            version_req: "==3.12.0".into(),
            versionsuffix: Some("-CUDA-12.8.0".into()),
            toolchain: None,
        }];
        let hierarchy = ToolchainHierarchy {
            parent: foss_tc.clone(),
            members: vec![
                Toolchain {
                    name: "system".into(),
                    version: String::new(),
                },
                foss_tc,
            ],
        };
        let specs = [
            SourceDepSpec::plain("Python", "3.12.0"),
            SourceDepSpec::plain("SciPy", "1.0.0"),
        ];
        let (map, _) = resolvo_resolve_dep_versions(
            &specs,
            &[python, python_cuda, scipy],
            &hierarchy,
            &foss(),
            "App",
            "1.0",
            None,
        )
        .expect("unsuffixed Python must survive a CUDA sibling");
        assert_eq!(map.get("Python").map(String::as_str), Some("3.12.0"));
        assert_eq!(map.get("SciPy").map(String::as_str), Some("1.0.0"));
    }

    #[test]
    fn bump_pins_keep_the_unsuffixed_module() {
        let cands = vec![
            candidate("Lib", "1.0", None),
            candidate("Lib", "1.0", Some("-CUDA-12.8.0")),
        ];
        let specs = [SourceDepSpec::plain("Lib", "1.0")];
        let (map, _) =
            resolvo_resolve_dep_versions(&specs, &cands, &hierarchy(), &foss(), "App", "1.0", None)
                .expect("resolve");
        assert_eq!(map.get("Lib").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn unmatched_required_dep_is_an_error() {
        let cands = vec![candidate("Lib", "1.0", None)];
        let specs = [SourceDepSpec::plain("Missing", "1.0")];
        let err =
            resolvo_resolve_dep_versions(&specs, &cands, &hierarchy(), &foss(), "App", "1.0", None)
                .expect_err("required miss");
        assert!(err.contains("Missing"), "{err}");
    }

    #[test]
    fn solver_errors_are_not_reprefixed() {
        let err = SelectError::Solver("unsatisfiable stack (resolvo SAT): GROMACS 2025.0".into());
        let text = err.to_string();
        assert_eq!(text.matches("unsatisfiable stack").count(), 1, "{text}");
        let missing = SelectError::MissingPackage("GROMACS".into());
        assert_eq!(missing.to_string(), "no candidates for package GROMACS");
    }

    #[test]
    fn a_cuda_sibling_does_not_drop_the_unsuffixed_system_module() {
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let mut system_lib = candidate("Lib", "1.0", None);
        system_lib.toolchain = system.clone();
        system_lib.easyconfig_path = "Lib-1.0.eb".into();
        let mut cuda = candidate("Lib", "1.0", Some("-CUDA-12.8.0"));
        cuda.easyconfig_path = "Lib-1.0-foss-2025a-CUDA-12.8.0.eb".into();
        let hierarchy = ToolchainHierarchy {
            parent: foss(),
            members: vec![
                Toolchain {
                    name: "system".into(),
                    version: String::new(),
                },
                foss(),
            ],
        };
        let specs = [SourceDepSpec::plain("Lib", "1.0")];
        let (map, _) = resolvo_resolve_dep_versions(
            &specs,
            &[system_lib, cuda],
            &hierarchy,
            &foss(),
            "App",
            "1.0",
            None,
        )
        .expect("unsuffixed SYSTEM must survive a CUDA sibling");
        assert_eq!(map.get("Lib").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn unmatched_optional_dep_is_skipped() {
        let cands = vec![candidate("Lib", "1.0", None)];
        let mut optional = SourceDepSpec::plain("Extra", "1.0");
        optional.optional = true;
        let specs = [SourceDepSpec::plain("Lib", "1.0"), optional];
        let (map, _) =
            resolvo_resolve_dep_versions(&specs, &cands, &hierarchy(), &foss(), "App", "1.0", None)
                .expect("optional miss is soft");
        assert_eq!(map.get("Lib").map(String::as_str), Some("1.0"));
        assert!(!map.contains_key("Extra"));
    }

    #[test]
    fn all_frozen_specs_return_an_empty_map() {
        let cands = vec![candidate("Lib", "1.0", None)];
        let mut frozen = SourceDepSpec::plain("Lib", "1.0");
        frozen.system_toolchain = true;
        let (map, _) = resolvo_resolve_dep_versions(
            &[frozen],
            &cands,
            &hierarchy(),
            &foss(),
            "App",
            "1.0",
            None,
        )
        .expect("frozen-only specs are not a floor miss");
        assert!(map.is_empty(), "{map:?}");
    }

    #[test]
    fn unsatisfiable_optional_extra_is_dropped() {
        let mut extra = candidate("Extra", "1.0", None);
        extra.dependencies.push(DepReq {
            name: "MissingTool".into(),
            version_req: "==1.0".into(),
            versionsuffix: None,
            toolchain: None,
        });
        let cands = vec![candidate("Lib", "1.0", None), extra];
        let mut optional = SourceDepSpec::plain("Extra", "1.0");
        optional.optional = true;
        let specs = [SourceDepSpec::plain("Lib", "1.0"), optional];
        let (map, _) =
            resolvo_resolve_dep_versions(&specs, &cands, &hierarchy(), &foss(), "App", "1.0", None)
                .expect("optional extra must not fail the resolve");
        assert_eq!(map.get("Lib").map(String::as_str), Some("1.0"));
        assert!(!map.contains_key("Extra"));
    }

    #[test]
    fn a_satisfiable_optional_survives_an_unsatisfiable_sibling() {
        let mut extra = candidate("Extra", "1.0", None);
        extra.dependencies.push(DepReq {
            name: "MissingTool".into(),
            version_req: "==1.0".into(),
            versionsuffix: None,
            toolchain: None,
        });
        let extra2 = candidate("Extra2", "1.0", None);
        let cands = vec![candidate("Lib", "1.0", None), extra, extra2];
        let mut optional = SourceDepSpec::plain("Extra", "1.0");
        optional.optional = true;
        let mut optional2 = SourceDepSpec::plain("Extra2", "1.0");
        optional2.optional = true;
        let specs = [SourceDepSpec::plain("Lib", "1.0"), optional, optional2];
        let (map, _) =
            resolvo_resolve_dep_versions(&specs, &cands, &hierarchy(), &foss(), "App", "1.0", None)
                .expect("one unsat extra must not strip the sat sibling");
        assert_eq!(map.get("Lib").map(String::as_str), Some("1.0"));
        assert!(!map.contains_key("Extra"));
        assert_eq!(map.get("Extra2").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn all_optional_unsat_still_keeps_a_sat_extra() {
        let mut extra = candidate("Extra", "1.0", None);
        extra.dependencies.push(DepReq {
            name: "MissingTool".into(),
            version_req: "==1.0".into(),
            versionsuffix: None,
            toolchain: None,
        });
        let extra2 = candidate("Extra2", "1.0", None);
        let mut optional = SourceDepSpec::plain("Extra", "1.0");
        optional.optional = true;
        let mut optional2 = SourceDepSpec::plain("Extra2", "1.0");
        optional2.optional = true;
        let (map, _) = resolvo_resolve_dep_versions(
            &[optional, optional2],
            &[extra, extra2],
            &hierarchy(),
            &foss(),
            "App",
            "1.0",
            None,
        )
        .expect("a sat extra must survive when every spec is optional");
        assert!(!map.contains_key("Extra"));
        assert_eq!(map.get("Extra2").map(String::as_str), Some("1.0"));
    }

    #[test]
    fn parent_toolchain_candidate_is_exempt_from_the_source_floor() {
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "13.3.0".into(),
        };
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let binutils = |version: &str, toolchain: &Toolchain| Candidate {
            name: "binutils".into(),
            version: version.into(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            easyconfig_path: format!("binutils-{version}.eb"),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let hierarchy = ToolchainHierarchy {
            parent: gcccore.clone(),
            members: vec![gcccore.clone(), system.clone()],
        };
        let cands = vec![binutils("2.42", &gcccore), binutils("2.46.1", &system)];
        let specs = [SourceDepSpec::plain("binutils", "2.45")];
        let (map, _) =
            resolvo_resolve_dep_versions(&specs, &cands, &hierarchy, &gcccore, "App", "1.0", None)
                .expect("parent-toolchain 2.42 is not a downgrade");
        assert_eq!(map.get("binutils").map(String::as_str), Some("2.42"));
    }

    #[test]
    fn a_below_floor_non_system_sibling_does_not_hide_system() {
        let foss = Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        };
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "13.3.0".into(),
        };
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let at = |version: &str, toolchain: &Toolchain| Candidate {
            name: "CMake".into(),
            version: version.into(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            easyconfig_path: format!("CMake-{version}.eb"),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let hierarchy = ToolchainHierarchy {
            parent: foss.clone(),
            members: vec![system.clone(), gcccore.clone(), foss.clone()],
        };
        let (map, _) = resolvo_resolve_dep_versions(
            &[SourceDepSpec::plain("CMake", "3.30")],
            &[at("3.29.3", &gcccore), at("3.31.8", &system)],
            &hierarchy,
            &foss,
            "App",
            "1.0",
            None,
        )
        .expect("SYSTEM 3.31.8 meets the floor");
        assert_eq!(map.get("CMake").map(String::as_str), Some("3.31.8"));
    }

    #[test]
    fn a_parent_floor_exemption_does_not_admit_a_below_floor_sibling() {
        let foss = foss();
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "14.2.0".into(),
        };
        let mut parent_lib = candidate("Lib", "1.0", None);
        parent_lib.toolchain = foss.clone();
        parent_lib.dependencies.push(DepReq {
            name: "MissingTool".into(),
            version_req: "==1.0".into(),
            versionsuffix: None,
            toolchain: None,
        });
        let mut sibling = candidate("Lib", "0.9", None);
        sibling.toolchain = gcccore.clone();
        sibling.easyconfig_path = "Lib-0.9-GCCcore-14.2.0.eb".into();
        let hierarchy = ToolchainHierarchy {
            parent: foss.clone(),
            members: vec![gcccore, foss.clone()],
        };
        let error = resolvo_resolve_dep_versions(
            &[SourceDepSpec::plain("Lib", "1.0")],
            &[parent_lib, sibling],
            &hierarchy,
            &foss,
            "App",
            "1.0",
            None,
        )
        .expect_err("below-floor GCCcore must not satisfy a 1.0 spec");
        assert!(
            !error.contains("0.9") || error.contains("unsatisfiable") || error.contains("Lib"),
            "{error}"
        );
    }
}
