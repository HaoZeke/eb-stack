//! eOn EasyBuild packaging: drive the real shipped parser/resolve path on the
//! recipes under fixtures/eon_packaging (mirrors eOn/easybuild drafts).
//!
//! eOn 2.16.0 `meson.build` requires `meson_version: '>= 1.8.0'`. The 2024a
//! robot only ships Meson-1.4.0-GCCcore-13.3.0; drafts supply 1.8.2.

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_file, parse_easyconfig_trees,
    resolve_easyconfig_file,
};
use std::path::PathBuf;

/// Minimum Meson version required by eOn 2.16.0 (from project meson_version).
const EON_MESON_FLOOR: (u64, u64, u64) = (1, 8, 0);

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_packaging")
}

/// Parse `X.Y.Z` (or `X.Y`) into a comparable triple; non-numeric → None.
fn parse_semver_triple(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts
        .next()
        .map(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().ok())
        .unwrap_or(Some(0))?;
    Some((major, minor, patch))
}

fn version_meets_floor(version: &str, floor: (u64, u64, u64)) -> bool {
    parse_semver_triple(version).is_some_and(|v| v >= floor)
}

#[test]
fn resolve_eon_easyconfig_fields_via_shipped_parser() {
    let p = root().join("easyconfigs/e/eOn/eOn-2.16.0-gfbf-2024a.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve eOn.eb");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.16.0");
    assert_eq!(r.toolchain.name, "gfbf");
    assert_eq!(r.toolchain.version, "2024a");
    // Packaging metadata (what eb-stack used to drop).
    assert_eq!(r.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(r.moduleclass.as_deref(), Some("chem"));
    assert!(!r.checksums.is_empty(), "checksum required for packaging");
    let opts = r.configopts.as_deref().unwrap_or("");
    for flag in ["-Dwith_fortran=true", "-Dwith_tests=false", "-Dwith_metatomic=false"] {
        assert!(opts.contains(flag), "configopts missing {flag}: {opts}");
    }
    packaging_gate(&r, &["-Dwith_fortran=true", "-Dwith_tests=false"]).expect("gate");
    // Exact co-pins, not ranges.
    let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
    for need in ["Python", "SciPy-bundle", "Eigen", "Highway", "inih", "quill"] {
        assert!(names.contains(&need), "missing dep {need} in {names:?}");
    }
    let quill = r.dependencies.iter().find(|d| d.name == "quill").unwrap();
    assert_eq!(quill.version, "11.1.0");
    assert_eq!(
        quill.toolchain.as_ref().map(|t| (t.name.as_str(), t.version.as_str())),
        Some(("GCCcore", "13.3.0"))
    );
    // Meson builddep must satisfy eOn's meson_version >= 1.8.0 (not 1.4.0),
    // and pin GCCcore-13.3.0 so GCCcore-14.3.0's Meson-1.8.2 cannot satisfy.
    let meson = r
        .builddependencies
        .iter()
        .find(|d| d.name == "Meson")
        .expect("Meson builddep required");
    assert_eq!(meson.version, "1.8.2", "pin companion Meson for 2024a/GCCcore-13.3.0");
    assert_eq!(
        meson.toolchain.as_ref().map(|t| (t.name.as_str(), t.version.as_str())),
        Some(("GCCcore", "13.3.0"))
    );
    assert!(
        version_meets_floor(&meson.version, EON_MESON_FLOOR),
        "Meson {} below eOn floor {:?}",
        meson.version,
        EON_MESON_FLOOR
    );
    let c = parse_easyconfig_file(&p).expect("to candidate");
    assert_eq!(c.dependencies.iter().find(|d| d.name == "Python").unwrap().version_req, "==3.12.3");
    assert!(
        c.builddependencies.iter().any(|d| d.name == "Rust"),
        "Rust builddep for readcon-core"
    );
    let meson_c = c
        .builddependencies
        .iter()
        .find(|d| d.name == "Meson")
        .unwrap();
    assert_eq!(meson_c.version_req, "==1.8.2");
}

#[test]
fn resolve_quill_companion_easyconfig() {
    let p = root().join("easyconfigs/q/quill/quill-11.1.0-GCCcore-13.3.0.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve quill.eb");
    assert_eq!(r.name, "quill");
    assert_eq!(r.version, "11.1.0");
    assert_eq!(r.toolchain.label(), "GCCcore-13.3.0");
    assert_eq!(r.easyblock.as_deref(), Some("CMakeMake"));
    assert_eq!(r.moduleclass.as_deref(), Some("lib"));
}

#[test]
fn resolve_meson_companion_meets_eon_floor() {
    let p = root().join("easyconfigs/m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve Meson companion .eb");
    assert_eq!(r.name, "Meson");
    assert_eq!(r.version, "1.8.2");
    assert_eq!(r.toolchain.label(), "GCCcore-13.3.0");
    assert_eq!(r.easyblock.as_deref(), Some("PythonPackage"));
    assert_eq!(r.moduleclass.as_deref(), Some("tools"));
    assert!(
        version_meets_floor(&r.version, EON_MESON_FLOOR),
        "companion Meson {} must be >= {:?}",
        r.version,
        EON_MESON_FLOOR
    );
    // 1.4.0 (what 2024a ships alone) must NOT satisfy the floor check used here.
    assert!(!version_meets_floor("1.4.0", EON_MESON_FLOOR));
    assert!(version_meets_floor("1.8.0", EON_MESON_FLOOR));
}

#[test]
fn eon_recipe_deps_found_in_drafts_plus_real_robot() {
    let drafts = root().join("easyconfigs");
    let recipe = resolve_easyconfig_file(&drafts.join("e/eOn/eOn-2.16.0-gfbf-2024a.eb")).unwrap();
    // Drafts alone: quill + Meson companions; runtime deps (Python/Eigen/…) missing.
    let draft_only = parse_easyconfig_trees(&[&drafts]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_only.candidates);
    assert!(!incomplete.ok(), "drafts alone cannot supply Python/Eigen/…");
    assert!(
        incomplete.missing.iter().any(|m| m.name == "Python"),
        "expected Python missing: {:?}",
        incomplete.missing
    );
    // Companion deps must still match when only drafts are loaded.
    assert!(
        incomplete.found.iter().any(|f| f.contains("quill")),
        "quill on GCCcore must match from drafts: {:?}",
        incomplete.found
    );
    assert!(
        incomplete.found.iter().any(|f| f.contains("Meson-1.8.2") && f.contains("GCCcore-13.3.0")),
        "Meson 1.8.2/GCCcore-13.3.0 companion must match from drafts: {:?}",
        incomplete.found
    );

    let home = std::env::var("HOME").unwrap_or_default();
    let real = PathBuf::from(&home).join(".venvs/easybuild/easybuild/easyconfigs");
    if !real.is_dir() {
        eprintln!("skip full robot check: {real:?} missing");
        return;
    }
    // Real tree alone: Meson-1.8.2 on GCCcore-13.3.0 is absent (1.8.2 only on 14.3+;
    // 13.3 has 1.4.0 only). Bare name/version pin would wrongly accept 14.3.
    let real_only = parse_easyconfig_trees(&[real.as_path()]).expect("real robot");
    let without_drafts = check_recipe_deps(&recipe, &real_only.candidates);
    assert!(
        without_drafts.missing.iter().any(|m| {
            m.name == "Meson"
                && m.version == "1.8.2"
                && m.toolchain.as_ref().is_some_and(|t| t.label() == "GCCcore-13.3.0")
        }),
        "upstream robot lacks Meson 1.8.2 for GCCcore-13.3.0: missing={:?}",
        without_drafts.missing
    );

    let merged = parse_easyconfig_trees(&[real.as_path(), drafts.as_path()]).expect("overlay");
    let check = check_recipe_deps(&recipe, &merged.candidates);
    eprintln!(
        "eOn robot check: found={} missing={:?} coverage={:.2}%",
        check.found.len(),
        check.missing.iter().map(|m| &m.name).collect::<Vec<_>>(),
        100.0 * merged.coverage()
    );
    assert!(
        check.ok(),
        "all runtime+build deps must resolve with drafts overlay: missing={:?}",
        check.missing
    );
    // Cross-toolchain quill + Meson floor pin found.
    assert!(check.found.iter().any(|f| f.contains("quill")));
    assert!(
        check.found.iter().any(|f| f.contains("Meson-1.8.2") && f.contains("GCCcore-13.3.0")),
        "need Meson 1.8.2 (GCCcore-13.3.0) from drafts: {:?}",
        check.found
    );
    // Guard: robot must not silently accept Meson 1.4.0 for a 1.8.2 pin.
    assert!(
        !check.found.iter().any(|f| f.contains("Meson-1.4.0")),
        "1.4.0 must not satisfy 1.8.2 pin: {:?}",
        check.found
    );
}
