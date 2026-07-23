//! eOn 2.17.10 core + rgpot recipes under fixtures/eon_core_rgpot.
//!
//! Frozen snapshot of the upstream draft PR set (easybuild-easyconfigs
//! #26480): eOn plus CapnProto, quill, readcon-core, and rgpot companions,
//! all on the single foss-2026.1 / GCCcore-15.2.0 generation. This is the
//! shape maintainers accept: conventional parameters, no staging scripts,
//! no cross-generation dependencies.

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_core_rgpot")
}

fn drafts() -> PathBuf {
    root().join("easyconfigs")
}

#[test]
fn resolve_eon_core_rgpot_recipe() {
    let p = drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve eOn core+rgpot");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.17.10");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2026.1");
    assert_eq!(r.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(r.moduleclass.as_deref(), Some("chem"));

    let opts = r.configopts.as_deref().unwrap_or("");
    assert!(opts.contains("-Dwith_rgpot=true"), "configopts: {opts}");
    for fat in ["-Dwith_metatomic", "-Dwith_xtb", "-Dwith_serve"] {
        assert!(!opts.contains(fat), "core product must not set {fat}");
    }
    packaging_gate(&r, &["-Dwith_rgpot=true"]).expect("packaging_gate");

    let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in [
        "Python",
        "SciPy-bundle",
        "PyYAML",
        "Eigen",
        "Highway",
        "inih",
        "nlohmann_json",
        "quill",
        "readcon-core",
        "rgpot",
    ] {
        assert!(names.contains(&need), "missing dep {need} in {names:?}");
    }

    // On foss apps, do not hard-code GCCcore dep toolchains (casparvl
    // #26480). Robot walks foss subtoolchains and finds GCCcore modules.
    for dep in &r.dependencies {
        assert!(
            dep.toolchain.is_none(),
            "do not hard-code dep toolchain on foss app: {} {:?}",
            dep.name,
            dep.toolchain
        );
    }
}

#[test]
fn readcon_core_no_patchelf_uses_check_readelf_rpath() {
    let text = std::fs::read_to_string(
        drafts().join("r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb"),
    )
    .unwrap();
    assert!(
        !text.contains("('patchelf'") && !text.contains("patchelf --"),
        "cargo-c install must not depend on or invoke patchelf"
    );
    assert!(
        text.contains("check_readelf_rpath = False"),
        "cargo-c skips EB RPATH wrappers; disable readelf RPATH presence check"
    );
    assert!(
        !text.contains("postinstallcmds"),
        "no postinstall RPATH rewrite"
    );
}

#[test]
fn eon_core_recipe_stays_conventional() {
    let text = std::fs::read_to_string(drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb")).unwrap();
    for banned in [
        "preconfigopts",
        "postinstallcmds",
        "readcon-stage",
        "patchelf",
        "EBROOTPYTORCH",
        "patches =",
    ] {
        assert!(
            !text.contains(banned),
            "core eOn recipe must not need {banned}"
        );
    }
}

#[test]
fn resolve_core_rgpot_companions() {
    let cases = [
        (
            "r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb",
            "rgpot",
            "2.5.3",
            "GCCcore",
            "15.2.0",
        ),
        (
            "r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb",
            "readcon-core",
            "0.13.1",
            "GCCcore",
            "15.2.0",
        ),
        (
            "c/CapnProto/CapnProto-1.4.0-GCCcore-15.2.0.eb",
            "CapnProto",
            "1.4.0",
            "GCCcore",
            "15.2.0",
        ),
        (
            "c/cargo-c/cargo-c-0.10.23-GCCcore-15.2.0.eb",
            "cargo-c",
            "0.10.23",
            "GCCcore",
            "15.2.0",
        ),
        (
            "q/quill/quill-11.1.0-GCCcore-15.2.0.eb",
            "quill",
            "11.1.0",
            "GCCcore",
            "15.2.0",
        ),
        (
            "i/inih/inih-62-GCCcore-15.2.0.eb",
            "inih",
            "62",
            "GCCcore",
            "15.2.0",
        ),
    ];
    for (rel, name, ver, tc_name, tc_ver) in cases {
        let r = resolve_easyconfig_file(&drafts().join(rel))
            .unwrap_or_else(|e| panic!("resolve {rel}: {e}"));
        assert_eq!(r.name, name, "{rel}");
        assert_eq!(r.version, ver, "{rel}");
        assert_eq!(r.toolchain.name, tc_name, "{rel}");
        assert_eq!(r.toolchain.version, tc_ver, "{rel}");
    }
}

#[test]
fn eon_core_check_recipe_drafts_plus_robot() {
    let recipe =
        resolve_easyconfig_file(&drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb")).unwrap();
    let drafts_root = drafts();
    let draft_tree = parse_easyconfig_trees(&[drafts_root.as_path()]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_tree.candidates);
    assert!(
        !incomplete.ok(),
        "drafts alone cannot supply Python/foss stack"
    );
    for companion in ["rgpot", "readcon-core", "quill", "inih"] {
        assert!(
            incomplete.found.iter().any(|f| f.contains(companion)),
            "drafts must supply {companion}: found={:?}",
            incomplete.found
        );
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let real = PathBuf::from(&home).join(".venvs/easybuild/easybuild/easyconfigs");
    if !real.is_dir() {
        eprintln!("skip full robot overlay: {real:?} missing");
        return;
    }
    let drafts_root2 = drafts();
    let merged =
        parse_easyconfig_trees(&[real.as_path(), drafts_root2.as_path()]).expect("overlay");
    let check = check_recipe_deps(&recipe, &merged.candidates);
    eprintln!(
        "eOn core+rgpot robot check: found={} missing={:?} coverage={:.2}%",
        check.found.len(),
        check
            .missing
            .iter()
            .map(|m| format!("{}-{}", m.name, m.version))
            .collect::<Vec<_>>(),
        100.0 * merged.coverage()
    );
    assert!(
        check.ok(),
        "eOn core fixture must resolve with overlay and robot: missing={:?}",
        check.missing
    );
    assert!(check.found.iter().any(|f| f.contains("rgpot")));
    assert!(check.found.iter().any(|f| f.contains("readcon-core")));
}

#[test]
fn eon_fat_2024a_and_core_2026_1_fixtures_coexist() {
    // Generation split stays explicit: the 2024a site-parity surface keeps
    // the full product, the 2026.1 tree carries the core + rgpot product.
    let a24 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/eon_packaging/easyconfigs/e/eOn/eOn-2.16.0-foss-2024a.eb");
    let a26 = drafts().join("e/eOn/eOn-2.17.10-foss-2026.1.eb");
    assert!(a24.is_file(), "2024a site-parity fixture missing");
    assert!(a26.is_file(), "2026.1 core fixture missing");
    let r24 = resolve_easyconfig_file(&a24).unwrap();
    let r26 = resolve_easyconfig_file(&a26).unwrap();
    assert_eq!(r24.toolchain.version, "2024a");
    assert_eq!(r26.toolchain.version, "2026.1");
}

#[test]
fn eon_core_recipe_copies_do_not_drift() {
    // The same recipe seeds the tutorial overlay tree, and the catalog
    // companions under examples/packages/companions close robot holes in
    // package plans. Every copy must stay identical to the draft-PR
    // snapshot.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pairs = [
        (
            "fixtures/eon_core_rgpot/easyconfigs/e/eOn/eOn-2.17.10-foss-2026.1.eb",
            "fixtures/eon_foss_2026_1/easyconfigs/e/eOn/eOn-2.17.10-foss-2026.1.eb",
        ),
        (
            "fixtures/eon_core_rgpot/easyconfigs/r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb",
            "examples/packages/companions/r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb",
        ),
        (
            "fixtures/eon_core_rgpot/easyconfigs/r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb",
            "examples/packages/companions/r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb",
        ),
    ];
    for (canonical_rel, copy_rel) in pairs {
        let canonical = std::fs::read_to_string(manifest.join(canonical_rel)).unwrap();
        let copy = std::fs::read_to_string(manifest.join(copy_rel)).unwrap();
        assert_eq!(canonical, copy, "{copy_rel} drifted from {canonical_rel}");
    }
}
