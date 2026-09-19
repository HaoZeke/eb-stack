//! Resolvo treats `exts_list` entries as provides of the parent bundle.

use eb_stack::{
    expand_extension_provides, parse_easyconfig_tree, select_stack, solve_from_easyconfigs,
    Candidate, DepReq, Policy, Toolchain, Universe,
};
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/extension_provides")
}

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn dep(name: &str, requirement: &str) -> DepReq {
    DepReq {
        name: name.into(),
        version_req: requirement.into(),
        versionsuffix: None,
        toolchain: None,
    }
}

fn candidate(name: &str, version: &str, dependencies: Vec<DepReq>) -> Candidate {
    Candidate {
        name: name.into(),
        version: version.into(),
        toolchain: toolchain(),
        versionsuffix: None,
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        dependencies,
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
        moduleclass: None,
    }
}

#[test]
fn parse_threads_exts_list_onto_candidates() {
    let tree = parse_easyconfig_tree(&fixture_root().join("easyconfigs")).expect("parse");
    let bundle = tree
        .candidates
        .iter()
        .find(|candidate| candidate.name == "SciPy-bundle")
        .expect("bundle");
    assert_eq!(bundle.exts_list.len(), 2);
    assert_eq!(bundle.exts_list[0].name, "numpy");
    assert_eq!(bundle.exts_list[0].version, "2.3.1");
}

#[test]
fn stack_solve_satisfies_numpy_from_scipy_bundle() {
    let easyconfigs = fixture_root().join("easyconfigs");
    let policy = fixture_root().join("policies/app_numpy.json");
    let tmp = tempfile::tempdir().unwrap();
    let lock = solve_from_easyconfigs(
        &[easyconfigs.as_path()],
        &policy,
        None,
        &tmp.path().join("lock.json"),
        None,
    )
    .expect("numpy must be provided by SciPy-bundle");

    assert_eq!(lock.package("App").unwrap().version, "1.0");
    assert_eq!(lock.package("SciPy-bundle").unwrap().version, "2025.06");
    let numpy = lock
        .package("numpy")
        .expect("virtual numpy provide in lock");
    assert!(
        numpy.easyconfig_path.contains("#ext:numpy"),
        "numpy must be the synthetic provide, got {}",
        numpy.easyconfig_path
    );
}

#[test]
fn standalone_numpy_recipe_still_competes_with_provide() {
    let mut universe = parse_easyconfig_tree(&fixture_root().join("easyconfigs"))
        .expect("parse")
        .candidates;
    universe.push(candidate("numpy", "2.3.1", Vec::new()));
    let expanded = expand_extension_provides(&universe);
    let numpy_idents: Vec<_> = expanded
        .iter()
        .filter(|candidate| candidate.name == "numpy")
        .map(|candidate| candidate.is_extension_provide())
        .collect();
    assert_eq!(numpy_idents.len(), 2);
    assert!(numpy_idents.contains(&true));
    assert!(numpy_idents.contains(&false));

    let policy = Policy {
        prefer_installed: false,
        toolchain: toolchain(),
        roots: vec!["App".into()],
        root_priority: None,
        pins: Vec::new(),
        forbid: Vec::new(),
        objective: "prefer_newer".into(),
        require_upgrade: Vec::new(),
        criteria: Vec::new(),
    };
    let lock = select_stack(
        &Universe {
            toolchain: toolchain(),
            generation_label: None,
            candidates: universe,
        },
        &policy,
        None,
    )
    .expect("solve with both numpy identities");
    assert!(lock.package("App").is_some());
    assert!(lock.package("numpy").is_some());
}

#[test]
fn missing_provide_still_unsat_when_no_bundle() {
    let policy = Policy {
        prefer_installed: false,
        toolchain: toolchain(),
        roots: vec!["App".into()],
        root_priority: None,
        pins: Vec::new(),
        forbid: Vec::new(),
        objective: "prefer_newer".into(),
        require_upgrade: Vec::new(),
        criteria: Vec::new(),
    };
    let err = select_stack(
        &Universe {
            toolchain: toolchain(),
            generation_label: None,
            candidates: vec![candidate("App", "1.0", vec![dep("numpy", "==2.3.1")])],
        },
        &policy,
        None,
    )
    .expect_err("no numpy and no bundle");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("unsat")
            || msg.contains("missing")
            || msg.contains("no candidates")
            || msg.contains("unknown"),
        "unexpected error: {err}"
    );
}
