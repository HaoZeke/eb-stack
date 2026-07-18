//! eOn foss-2026.1 overlay tree under fixtures/eon_foss_2026_1.
//!
//! Historical companion recipes from the fat-product surface (metatensor,
//! metatensor-torch, metatomic-torch, PyTorch residual) plus the current
//! core eOn recipe used by the tutorial overlay. The canonical core + rgpot
//! PR snapshot lives in fixtures/eon_core_rgpot (tests/eon_core_rgpot.rs).

use eb_stack::{check_recipe_deps, parse_easyconfig_trees, resolve_easyconfig_file};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/eon_foss_2026_1")
}

fn drafts() -> PathBuf {
    root().join("easyconfigs")
}

#[test]
fn resolve_eon_overlay_recipe() {
    let p = drafts().join("e/eOn/eOn-2.17.1-foss-2026.1.eb");
    let r = resolve_easyconfig_file(&p).expect("resolve eOn overlay recipe");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.17.1");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2026.1");
    assert_eq!(r.easyblock.as_deref(), Some("MesonNinja"));
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
fn rust_wrappers_are_unset_before_cargo_runs() {
    let text =
        std::fs::read_to_string(drafts().join("m/metatensor/metatensor-0.2.2-GCCcore-15.2.0.eb"))
            .unwrap();
    assert!(
        text.contains("unset RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER"),
        "metatensor recipe must remove Cargo wrappers from the process environment"
    );
    assert!(
        !text.contains("export RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER="),
        "metatensor recipe must not expose empty wrapper executable paths"
    );
}

#[test]
fn eon_overlay_check_recipe_stays_loud_about_missing_deps() {
    // The overlay tree carries historical companions, not the core deps:
    // readcon-core and rgpot live in fixtures/eon_core_rgpot. A dep check
    // against this tree alone must fail loudly instead of pretending the
    // recipe resolves.
    let recipe =
        resolve_easyconfig_file(&drafts().join("e/eOn/eOn-2.17.1-foss-2026.1.eb")).unwrap();
    let drafts_root = drafts();
    let draft_tree = parse_easyconfig_trees(&[drafts_root.as_path()]).unwrap();
    let incomplete = check_recipe_deps(&recipe, &draft_tree.candidates);
    assert!(
        !incomplete.ok(),
        "overlay tree alone cannot supply the core closure"
    );
    assert!(
        incomplete.found.iter().any(|f| f.contains("quill")),
        "overlay must still supply quill: found={:?}",
        incomplete.found
    );
    for core_dep in ["readcon-core", "rgpot"] {
        assert!(
            incomplete.missing.iter().any(|m| m.name.contains(core_dep)),
            "{core_dep} lives in eon_core_rgpot, not the overlay: missing={:?}",
            incomplete.missing
        );
    }
}
