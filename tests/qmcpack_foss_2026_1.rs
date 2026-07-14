//! QMCPACK 4.3.0 on foss-2026.1 (PR #26437): fixtures/qmcpack_foss_2026_1.
//! Real parse/resolve/check-recipe path.

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/qmcpack_foss_2026_1")
}

fn recipe_path() -> PathBuf {
    root().join("easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb")
}

#[test]
fn resolve_qmcpack_foss_2026_1() {
    let r = resolve_easyconfig_file(&recipe_path()).expect("resolve QMCPACK");
    assert_eq!(r.name, "QMCPACK");
    assert_eq!(r.version, "4.3.0");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2026.1");
    assert_eq!(r.easyblock.as_deref(), Some("CMakeNinja"));
    assert_eq!(r.moduleclass.as_deref(), Some("chem"));
    assert!(!r.checksums.is_empty(), "checksum required");

    let opts = r.configopts.as_deref().unwrap_or("");
    for flag in [
        "-DQMC_MPI=ON",
        "-DQMC_OMP=ON",
        "-DQMC_MIXED_PRECISION=OFF",
        "-DQMC_COMPLEX=OFF",
        "-DBUILD_AFQMC=OFF",
    ] {
        assert!(opts.contains(flag), "configopts missing {flag}: {opts}");
    }
    packaging_gate(
        &r,
        &["-DQMC_MPI=ON", "-DQMC_OMP=ON", "-DQMC_MIXED_PRECISION=OFF"],
    )
    .expect("packaging_gate");

    let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in ["HDF5", "Boost", "libxml2", "Python"] {
        assert!(names.contains(&need), "missing dep {need} in {names:?}");
    }

    let txt = std::fs::read_to_string(recipe_path()).unwrap();
    assert!(
        txt.contains("test_cmd") && txt.contains("ctest"),
        "must set test_cmd=ctest (CMakeNinja ninja no-op trap)"
    );
    assert!(
        txt.contains("modextrapaths") && txt.contains("PYTHONPATH"),
        "Nexus PYTHONPATH required"
    );
    assert!(
        txt.contains("start_dir") || txt.contains("namelower"),
        "GitHub archive dir layout (qmcpack-ver) must be handled"
    );
}

#[test]
fn qmcpack_check_recipe_against_robot() {
    let recipe = resolve_easyconfig_file(&recipe_path()).unwrap();
    // Fixture alone has no robot deps.
    let drafts = root().join("easyconfigs");
    let alone = parse_easyconfig_trees(&[drafts.as_path()]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &alone.candidates);
    assert!(
        !incomplete.ok(),
        "fixture alone must not close HDF5/Boost/…: found={:?}",
        incomplete.found
    );

    let home = std::env::var("HOME").unwrap_or_default();
    let real = PathBuf::from(&home).join(".venvs/easybuild/easybuild/easyconfigs");
    if !real.is_dir() {
        eprintln!("skip robot check: {real:?} missing");
        return;
    }
    let robot = parse_easyconfig_trees(&[real.as_path()]).expect("robot");
    let check = check_recipe_deps(&recipe, &robot.candidates);
    eprintln!(
        "QMCPACK foss-2026.1 robot check: found={} missing={:?}",
        check.found.len(),
        check
            .missing
            .iter()
            .map(|m| format!("{}-{}", m.name, m.version))
            .collect::<Vec<_>>()
    );
    assert!(
        check.ok(),
        "QMCPACK must resolve against robot alone (no companions): missing={:?}",
        check.missing
    );
    assert!(check.found.iter().any(|f| f.contains("HDF5")));
    assert!(check.found.iter().any(|f| f.contains("Boost")));
}
