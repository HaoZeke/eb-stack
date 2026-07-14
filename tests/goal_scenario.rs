//! Goal-scenario acceptance: parse conda-forge (eOn) and Spack (QMCPACK) into
//! an SBOM + build manifest, then emit an EasyBuild recipe, on the landable
//! foss-2026.1 generation. Locks the exact product goal as a machine check.
//!
//! Drives the shipped `plan_and_emit` library path (foreign -> IntermediatePlan
//! IR -> planned CycloneDX SBOM -> hierarchy + resolvo joint co-select -> new
//! `.eb`). The *resolves* rung against a full easyconfigs robot is proven on a
//! real EasyBuild host; here the in-repo overlay carries the companion recipes
//! only, so we assert the mechanical artefacts + reparse (the honest fixture
//! guarantee) rather than a full resolvo co-select.

use eb_stack::{plan_and_emit, resolve_easyconfig_file, ForeignFormat, IngestOpts, Toolchain};
use serde_json::Value;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn foss_2026() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn robot_overlay() -> PathBuf {
    repo().join("fixtures/eon_foss_2026_1/easyconfigs")
}

fn read_json(p: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(p).expect("read json artefact");
    serde_json::from_str(&text).expect("parse json artefact")
}

/// A planned SBOM must be a CycloneDX document carrying the package as a
/// component (name match, case-insensitive).
fn assert_cyclonedx_has_component(sbom: &Value, pkg: &str) {
    assert_eq!(
        sbom.get("bomFormat").and_then(Value::as_str),
        Some("CycloneDX"),
        "planned SBOM is not CycloneDX: {sbom}"
    );
    let comps = sbom
        .get("components")
        .and_then(Value::as_array)
        .expect("SBOM has components array");
    let found = comps.iter().any(|c| {
        c.get("name")
            .and_then(Value::as_str)
            .map(|n| n.eq_ignore_ascii_case(pkg))
            .unwrap_or(false)
    });
    assert!(found, "planned SBOM has no component for {pkg}: {sbom}");
}

fn run_plan(source_rel: &str, format: ForeignFormat, exp_name: &str) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manifest_out = tmp.path().join("plan.manifest.json");
    let sbom_out = tmp.path().join("plan.sbom.json");
    let out_dir = tmp.path().join("out");
    let toolchain = foss_2026();
    let opts = IngestOpts {
        easyconfigs: vec![robot_overlay()],
        keep_old_deps: true,
        hierarchy_fixture: None,
    };

    let (plan, eb_path) = plan_and_emit(
        &repo().join(source_rel),
        Some(format),
        &toolchain,
        &opts,
        Some(&manifest_out),
        Some(&sbom_out),
        None,
        Some(&out_dir),
        None,
    )
    .expect("plan_and_emit on foss-2026.1");

    // Manifest IR written and coherent.
    assert!(manifest_out.is_file(), "manifest not written");
    let manifest = read_json(&manifest_out);
    assert!(
        manifest.get("package").is_some(),
        "manifest missing package IR: {manifest}"
    );
    assert_eq!(plan.package.name.to_lowercase(), exp_name.to_lowercase());

    // Planned CycloneDX SBOM written and lists the package.
    assert!(sbom_out.is_file(), "SBOM not written");
    let sbom = read_json(&sbom_out);
    assert_cyclonedx_has_component(&sbom, exp_name);

    // Emitted easyconfig reparses to the target identity + toolchain.
    let eb = eb_path.expect("emitted .eb path");
    assert!(eb.is_file(), "emitted .eb missing at {}", eb.display());
    let resolved = resolve_easyconfig_file(&eb).expect("reparse emitted .eb");
    assert_eq!(resolved.toolchain.name, "foss");
    assert_eq!(resolved.toolchain.version, "2026.1");
    assert_eq!(
        resolved.name.to_lowercase(),
        exp_name.to_lowercase(),
        "emitted recipe name mismatch"
    );
}

#[test]
fn conda_eon_to_sbom_manifest_recipe_foss_2026_1() {
    run_plan(
        "fixtures/foreign_ingest/conda_eon/recipe.yaml",
        ForeignFormat::CondaForge,
        "eon",
    );
}

#[test]
fn spack_qmcpack_to_sbom_manifest_recipe_foss_2026_1() {
    run_plan(
        "fixtures/foreign_ingest/spack_qmcpack/package.py",
        ForeignFormat::Spack,
        "qmcpack",
    );
}
