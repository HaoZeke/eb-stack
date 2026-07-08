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
            }],
        };
        let universe = Universe {
            toolchain: tc.clone(),
            generation_label: Some("builddep-co-select".into()),
            candidates: vec![root, leaf],
        };
        let policy = Policy {
            toolchain: tc,
            roots: vec!["App".into()],
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: None,
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
            }],
        };
        let universe = Universe {
            toolchain: tc.clone(),
            generation_label: None,
            candidates: vec![root],
        };
        let policy = Policy {
            toolchain: tc,
            roots: vec!["App".into()],
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: None,
        };
        let err = select_stack(&universe, &policy, None).unwrap_err();
        let msg = err.to_string();
        let low = msg.to_lowercase();
        assert!(
            low.contains("unsatisfiable")
                || low.contains("unsat")
                || low.contains("missing"),
            "builddep miss should fail like runtime dep miss: {msg}"
        );
    }

    #[test]
    fn fixture_builddep_root_co_selects_fftw() {
        let (universe, _) = universe_next_from_eb();
        assert!(
            universe
                .candidates
                .iter()
                .any(|c| c.name == "BuildDepRoot"),
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
            toolchain: universe.toolchain.clone(),
            roots: vec!["BuildDepRoot".into()],
            pins: vec![],
            forbid: vec![],
            objective: "prefer_newer".into(),
            require_upgrade: None,
        };
        let lock = select_stack(&universe, &policy, None).expect("BuildDepRoot solve");
        assert!(
            lock.package("FFTW").is_some(),
            "FFTW co-selected via builddependencies; packages={:?}",
            lock.packages
                .iter()
                .map(|p| &p.name)
                .collect::<Vec<_>>()
        );
        assert!(lock.package("OpenBLAS").is_some());
    }
}
