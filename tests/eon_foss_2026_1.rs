//! eOn 2.16.0 on foss-2026.1 regression recipes under fixtures/eon_foss_2026_1.
//!
//! Distinct from fixtures/eon_packaging (foss-2024a site/feedstock parity).
//! Drive the real shipped parse, resolve, and recipe-check path.

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_trees, resolve_easyconfig_file,
};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_foss_2026_1")
}

fn drafts() -> PathBuf {
    root().join("easyconfigs")
}

#[test]
fn resolve_eon_foss_2026_1_fixture() {
    let p = drafts().join("e/eOn/eOn-2.16.0-foss-2026.1.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve eOn foss-2026.1");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.16.0");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2026.1");
    assert_eq!(r.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(r.moduleclass.as_deref(), Some("chem"));
    assert!(
        r.checksums.len() >= 3,
        "multi-source checksums expected, got {}",
        r.checksums.len()
    );
    let opts = r.configopts.as_deref().unwrap_or("");
    for flag in [
        "-Dwith_metatomic=true",
        "-Dwith_xtb=true",
        "-Dwith_serve=true",
        "-Dwith_rgpot=true",
    ] {
        assert!(opts.contains(flag), "configopts missing {flag}: {opts}");
    }
    packaging_gate(
        &r,
        &[
            "-Dwith_metatomic=true",
            "-Dwith_xtb=true",
            "-Dwith_serve=true",
            "-Dwith_rgpot=true",
        ],
    )
    .expect("packaging_gate");

    let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in [
        "Python",
        "SciPy-bundle",
        "Eigen",
        "Highway",
        "quill",
        "xtb",
        "CapnProto",
        "PyTorch",
        "metatensor",
        "metatensor-torch",
        "metatomic-torch",
    ] {
        assert!(names.contains(&need), "missing dep {need} in {names:?}");
    }

    // Documented cross-generation residual pins (robot lacks 2026.1 recipes).
    let xtb = r.dependencies.iter().find(|d| d.name == "xtb").unwrap();
    assert!(
        xtb.toolchain
            .as_ref()
            .is_some_and(|t| t.name == "gfbf" && t.version == "2024a"),
        "xtb should pin gfbf-2024a residual: {:?}",
        xtb.toolchain
    );
    let torch = r.dependencies.iter().find(|d| d.name == "PyTorch").unwrap();
    assert!(
        torch
            .toolchain
            .as_ref()
            .is_some_and(|t| t.name == "foss" && t.version == "2024a"),
        "PyTorch should pin foss-2024a residual: {:?}",
        torch.toolchain
    );

    let eon_txt = std::fs::read_to_string(&p).unwrap();
    assert!(
        eon_txt.contains("eOn-%(version)s_safemath-eigen5-core-guard.patch")
            || eon_txt.contains("safemath-eigen5"),
        "must reference eigen5 safemath patch"
    );
    assert!(
        drafts()
            .join("e/eOn/eOn-2.16.0_safemath-eigen5-core-guard.patch")
            .is_file(),
        "patch file must ship with fixture"
    );
    assert!(
        eon_txt.contains("$EBROOTMETATENSORMINTORCH")
            && eon_txt.contains("$EBROOTMETATOMICMINTORCH"),
        "hyphenated EBROOT convert_name forms required"
    );
}

#[test]
fn resolve_eon_2026_1_companions() {
    let cases = [
        (
            "m/metatensor/metatensor-0.2.2-GCCcore-15.2.0.eb",
            "metatensor",
            "0.2.2",
            "GCCcore",
            "15.2.0",
        ),
        (
            "m/metatensor-torch/metatensor-torch-0.10.0-foss-2026.1.eb",
            "metatensor-torch",
            "0.10.0",
            "foss",
            "2026.1",
        ),
        (
            "m/metatomic-torch/metatomic-torch-0.1.15-foss-2026.1.eb",
            "metatomic-torch",
            "0.1.15",
            "foss",
            "2026.1",
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
        (
            "c/cargo-c/cargo-c-0.10.23-GCCcore-15.2.0.eb",
            "cargo-c",
            "0.10.23",
            "GCCcore",
            "15.2.0",
        ),
        (
            "p/PyTorch/PyTorch-2.9.1-foss-2024a.eb",
            "PyTorch",
            "2.9.1",
            "foss",
            "2024a",
        ),
        (
            "m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb",
            "Meson",
            "1.8.2",
            "GCCcore",
            "13.3.0",
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
fn rust_wrappers_are_reset_without_revealing_host_config() {
    for rel in [
        "e/eOn/eOn-2.16.0-foss-2026.1.eb",
        "m/metatensor/metatensor-0.2.2-GCCcore-15.2.0.eb",
    ] {
        let text = std::fs::read_to_string(drafts().join(rel)).unwrap();
        assert!(
            text.contains("RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER="),
            "{rel} must reset Cargo wrappers to empty values"
        );
        assert!(
            !text.contains("unset RUSTC_WRAPPER")
                && !text.contains("unset CARGO_BUILD_RUSTC_WRAPPER"),
            "{rel} must not reveal wrappers from an ancestor Cargo config"
        );
    }
}

#[test]
fn eon_2026_1_check_recipe_drafts_plus_robot() {
    let recipe =
        resolve_easyconfig_file(&drafts().join("e/eOn/eOn-2.16.0-foss-2026.1.eb")).unwrap();
    let drafts_root = drafts();
    let draft_tree = parse_easyconfig_trees(&[drafts_root.as_path()]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_tree.candidates);
    assert!(
        !incomplete.ok(),
        "drafts alone cannot supply Python/foss stack"
    );
    for companion in ["quill", "metatensor", "metatensor-torch", "metatomic-torch"] {
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
        "eOn foss-2026.1 robot check: found={} missing={:?} coverage={:.2}%",
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
        "eOn foss-2026.1 fixture must resolve with overlay and robot: missing={:?}",
        check.missing
    );
    assert!(check.found.iter().any(|f| f.contains("metatomic-torch")));
    assert!(check.found.iter().any(|f| f.contains("quill")));
}

#[test]
fn eon_2024a_and_2026_1_fixtures_coexist() {
    // Explicit generation split: both trees must stay distinct.
    let a24 = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/eon_packaging/easyconfigs/e/eOn/eOn-2.16.0-foss-2024a.eb");
    let a26 = drafts().join("e/eOn/eOn-2.16.0-foss-2026.1.eb");
    assert!(a24.is_file(), "2024a site-parity fixture missing");
    assert!(a26.is_file(), "2026.1 regression fixture missing");
    let r24 = resolve_easyconfig_file(&a24).unwrap();
    let r26 = resolve_easyconfig_file(&a26).unwrap();
    assert_eq!(r24.toolchain.version, "2024a");
    assert_eq!(r26.toolchain.version, "2026.1");
}
