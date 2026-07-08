//! Stack selection via resolvo CDCL SAT (fed by parsed easyconfig candidates).

use crate::domain::{LockPackage, Policy, SolverMeta, StackLock, Universe};
use crate::resolvo_provider::solve_with_resolvo;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SelectError {
    #[error("no candidates for package {0}")]
    MissingPackage(String),
    #[error("unsatisfiable stack: {0}")]
    Unsat(String),
}

/// Select a stack using **resolvo** (CDCL SAT) over EasyBuild-derived candidates.
pub fn select_stack(
    universe: &Universe,
    policy: &Policy,
    baseline: Option<&StackLock>,
) -> Result<StackLock, SelectError> {
    let selected = solve_with_resolvo(&universe.candidates, policy, baseline).map_err(|e| {
        let el = e.to_lowercase();
        if el.contains("unsatisfiable") || el.contains("unsat") {
            SelectError::Unsat(e)
        } else if el.contains("no candidates") || el.contains("unknown package") {
            SelectError::MissingPackage(e)
        } else {
            SelectError::Unsat(e)
        }
    })?;

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
    packages_out.sort_by(|a, b| a.name.cmp(&b.name));

    for root in &policy.roots {
        if !packages_out.iter().any(|p| &p.name == root) {
            return Err(SelectError::MissingPackage(root.clone()));
        }
    }

    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    Ok(StackLock {
        schema_version: 1,
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
        let all = parse_easyconfig_tree(&root).expect("parse tree");
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
        assert!(matches!(
            cmp_version(&lock.package("OpenMPI").unwrap().version, "4.1.6"),
            Ordering::Equal | Ordering::Greater
        ));
        assert!(lock.package("FFTW").is_some());
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
        assert!(low.contains("unsatisfiable") || low.contains("unsat"), "{msg}");
        // Human-readable versions, not raw resolvo ranks ("GROMACS 2", "OpenMPI 1 | 2").
        assert!(
            msg.contains("2025.0") && (msg.contains("4.1.6") || msg.contains("5.0.3")),
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
        let all = parse_easyconfig_tree(&root).expect("parse two-root tree");
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
                policy.effective_root_priority(),
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
                policy.effective_root_priority(),
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
            g_first.effective_root_priority(),
            vec!["GROMACS".to_string(), "LAMMPS".to_string()]
        );
        let lock_g = select_stack(&universe, &g_first, None).expect("default gromacs-first");
        assert_eq!(lock_g.package("GROMACS").unwrap().version, "2025.0");
        assert_eq!(lock_g.package("LAMMPS").unwrap().version, "2023.08");

        let l_first = load_two_root_policy("default_priority_lammps_listed_first.json");
        assert!(l_first.root_priority.is_none());
        assert_eq!(
            l_first.effective_root_priority(),
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
        universe.candidates.sort_by(|a, b| b.easyconfig_path.cmp(&a.easyconfig_path));
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
            single.effective_root_priority(),
            vec!["GROMACS".to_string()]
        );
    }
}
