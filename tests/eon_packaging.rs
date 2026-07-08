//! eOn EasyBuild packaging: drive the real shipped parser/resolve path on the
//! recipes under fixtures/eon_packaging (mirrors eOn/easybuild drafts).

use eb_stack::{
    check_recipe_deps, packaging_gate, parse_easyconfig_file, parse_easyconfig_trees,
    resolve_easyconfig_file,
};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_packaging")
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
    let c = parse_easyconfig_file(&p).expect("to candidate");
    assert_eq!(c.dependencies.iter().find(|d| d.name == "Python").unwrap().version_req, "==3.12.3");
    assert!(
        c.builddependencies.iter().any(|d| d.name == "Rust"),
        "Rust builddep for readcon-core"
    );
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
fn eon_recipe_deps_found_in_drafts_plus_real_robot() {
    let drafts = root().join("easyconfigs");
    let recipe = resolve_easyconfig_file(&drafts.join("e/eOn/eOn-2.16.0-gfbf-2024a.eb")).unwrap();
    // Drafts alone: only quill is present as a candidate; Python etc. missing.
    let draft_only = parse_easyconfig_trees(&[&drafts]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_only.candidates);
    assert!(!incomplete.ok(), "drafts alone cannot supply Python/Eigen/…");
    assert!(
        incomplete.missing.iter().any(|m| m.name == "Python"),
        "expected Python missing: {:?}",
        incomplete.missing
    );
    // quill cross-toolchain dep must still match when only drafts are loaded.
    assert!(
        incomplete.found.iter().any(|f| f.contains("quill")),
        "quill on GCCcore must match from drafts: {:?}",
        incomplete.found
    );

    let home = std::env::var("HOME").unwrap_or_default();
    let real = PathBuf::from(&home).join(".venvs/easybuild/easybuild/easyconfigs");
    if !real.is_dir() {
        eprintln!("skip full robot check: {real:?} missing");
        return;
    }
    let merged = parse_easyconfig_trees(&[real.as_path(), drafts.as_path()]).expect("overlay");
    let check = check_recipe_deps(&recipe, &merged.candidates);
    eprintln!(
        "eOn robot check: found={} missing={:?} coverage={:.2}%",
        check.found.len(),
        check.missing.iter().map(|m| &m.name).collect::<Vec<_>>(),
        100.0 * merged.coverage()
    );
    // Runtime deps should resolve; builddeps like Rust may be GCCcore-only.
    assert!(
        check.missing.iter().all(|m| m.role == "build" || m.name == "Rust" || m.name == "pkgconf"
            || m.name == "Meson" || m.name == "Ninja" || m.name == "CMake"),
        "unexpected runtime missing deps: {:?}",
        check.missing
    );
    // Cross-toolchain quill found.
    assert!(check.found.iter().any(|f| f.contains("quill")));
}
