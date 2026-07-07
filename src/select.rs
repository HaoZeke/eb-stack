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
}
