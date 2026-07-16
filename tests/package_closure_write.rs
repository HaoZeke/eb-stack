//! Aggregate package-closure bundle writer.
//!
//! Synthetic package names only — no production package identities.

use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_catalog::{resolve_package_catalog_layers, PackageCatalogLayer};
use eb_stack::{
    merge_closure_sboms, package_layout_segment, plan_new_package, plan_package_closure,
    write_package_bundle, write_package_closure, ForeignFormat, NewPackageRequest,
    PackageClosureError, PackageWorkflowError, Toolchain,
};
use serde_json::json;
use std::path::Path;

const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CHECKSUM_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn policy() -> StackPolicy {
    StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "closure-write-test".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    }
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(path, body).expect("write");
}

fn plan_alpha_bravo_closure(temp: &Path) -> eb_stack::PackageClosure {
    write(
        &temp.join("alpha.yaml"),
        r#"
package:
  name: alpha
  version: "1.0"
source:
  url: https://example.invalid/alpha-1.0.tar.gz
requirements:
  host:
    - bravo >=1.0
"#,
    );
    write(
        &temp.join("bravo.yaml"),
        r#"
package:
  name: bravo
  version: "1.5"
source:
  url: https://example.invalid/bravo-1.5.tar.gz
"#,
    );
    let catalog_path = temp.join("catalog.toml");
    write(
        &catalog_path,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.5"
source = "bravo.yaml"
format = "conda-forge"
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
source_checksums = ["{CHECKSUM_B}"]
"#
        ),
    );
    let robot = temp.join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    let layer = PackageCatalogLayer::from_path(&catalog_path).expect("layer");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("catalog");
    let request = NewPackageRequest {
        source: temp.join("alpha.yaml"),
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: vec![CHECKSUM.to_string()],
        package_layers: Vec::new(),
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    };
    plan_package_closure(&request, &catalog).expect("plan closure")
}

#[test]
fn write_package_closure_emits_aggregate_layout_and_order() {
    let temp = tempfile::tempdir().expect("temp");
    let closure = plan_alpha_bravo_closure(temp.path());
    let out = temp.path().join("bundle");
    let written = write_package_closure(&closure, &out).expect("write closure");

    assert!(written.root.manifest.ends_with("package.plan.json"));
    assert!(written.root.sbom.ends_with("package.sbom.cdx.json"));
    assert!(out.join("package.plan.json").is_file());
    assert!(out.join("package.sbom.cdx.json").is_file());
    assert!(out.join("locks/default.lock.json").is_file());
    assert!(out
        .join("packages/bravo-1.5-foss-2026.1/package.plan.json")
        .is_file());
    assert!(out
        .join("packages/bravo-1.5-foss-2026.1/package.sbom.cdx.json")
        .is_file());
    assert!(out
        .join("packages/bravo-1.5-foss-2026.1/locks/default.lock.json")
        .is_file());
    assert!(out
        .join("easyconfigs/a/alpha/alpha-1.0-foss-2026.1.eb")
        .is_file());
    assert!(out
        .join("easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb")
        .is_file());
    assert!(written.build_order.is_file());
    assert!(written.closure_plan.is_file());
    assert!(written.closure_sbom.is_file());

    let build_order: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&written.build_order).expect("order"))
            .expect("json");
    assert_eq!(build_order["schema_version"], 1);
    assert_eq!(
        build_order["recipes"],
        json!([
            "easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb",
            "easyconfigs/a/alpha/alpha-1.0-foss-2026.1.eb"
        ])
    );

    let segment = package_layout_segment(&closure.companions[0]).expect("segment");
    assert_eq!(segment, "bravo-1.5-foss-2026.1");

    let aggregate: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&written.closure_sbom).expect("sbom"))
            .expect("json");
    assert_eq!(aggregate["bomFormat"], "CycloneDX");
    let names = aggregate["components"]
        .as_array()
        .expect("components")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"bravo"));
}

#[test]
fn package_layout_segment_rejects_unsafe_names() {
    let temp = tempfile::tempdir().expect("temp");
    let source = temp.path().join("pkg.yaml");
    write(
        &source,
        r#"
package:
  name: safe
  version: "1.0"
source:
  url: https://example.invalid/safe-1.0.tar.gz
"#,
    );
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    let mut bundle = plan_new_package(&NewPackageRequest {
        source,
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: vec![CHECKSUM.to_string()],
        package_layers: Vec::new(),
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    })
    .expect("plan");
    bundle.plan.package.name = "evil/../x".into();
    let err = package_layout_segment(&bundle).expect_err("unsafe");
    assert!(
        matches!(
            err,
            PackageClosureError::Workflow(PackageWorkflowError::UnsafePathSegment { .. })
        ) || err.to_string().contains("unsafe")
            || err.to_string().contains("path"),
        "{err}"
    );
}

#[test]
fn write_package_closure_rejects_recipe_destination_collisions() {
    let temp = tempfile::tempdir().expect("temp");
    let closure = plan_alpha_bravo_closure(temp.path());
    // Force both packages to emit the same overlay path by rewriting companion identity
    // to collide with root recipe directory + filename.
    let mut colliding = closure.clone();
    assert!(!colliding.companions.is_empty());
    let root_filename = colliding.root.easyconfigs[0].filename.clone();
    colliding.companions[0].plan.package.name = colliding.root.plan.package.name.clone();
    colliding.companions[0].easyconfigs[0].filename = root_filename;
    colliding.companions[0].easyconfigs[0].text = "name = 'collision'\n".into();

    let out = temp.path().join("collide");
    let err = write_package_closure(&colliding, &out).expect_err("collision");
    let message = err.to_string();
    assert!(
        message.contains("collision") || message.contains("overlay"),
        "{message}"
    );
}

#[test]
fn merge_closure_sboms_deduplicates_by_bom_ref_and_keeps_cyclonedx() {
    let left = json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": { "type": "application", "name": "alpha", "version": "1.0", "bom-ref": "pkg:generic/alpha@1.0" }
        },
        "components": [
            { "type": "application", "name": "alpha", "version": "1.0", "bom-ref": "pkg:generic/alpha@1.0" },
            { "type": "library", "name": "shared", "version": "9", "bom-ref": "pkg:generic/shared@9" }
        ],
        "dependencies": [
            { "ref": "pkg:generic/alpha@1.0", "dependsOn": ["pkg:generic/shared@9"] }
        ]
    });
    let right = json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": { "type": "application", "name": "bravo", "version": "1.5", "bom-ref": "pkg:generic/bravo@1.5" }
        },
        "components": [
            { "type": "application", "name": "bravo", "version": "1.5", "bom-ref": "pkg:generic/bravo@1.5" },
            { "type": "library", "name": "shared", "version": "9", "bom-ref": "pkg:generic/shared@9" }
        ],
        "dependencies": [
            { "ref": "pkg:generic/bravo@1.5", "dependsOn": ["pkg:generic/shared@9"] },
            { "ref": "pkg:generic/shared@9", "dependsOn": [] }
        ]
    });
    let merged = merge_closure_sboms([&left, &right]).expect("merge");
    assert_eq!(merged["bomFormat"], "CycloneDX");
    let components = merged["components"].as_array().expect("components");
    assert_eq!(components.len(), 3, "{components:?}");
    let names: Vec<_> = components
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"bravo"));
    assert!(names.contains(&"shared"));
    let shared_count = components
        .iter()
        .filter(|c| c["bom-ref"] == "pkg:generic/shared@9")
        .count();
    assert_eq!(shared_count, 1);
}

#[test]
fn single_package_writer_api_still_writes_legacy_layout() {
    let temp = tempfile::tempdir().expect("temp");
    let source = temp.path().join("solo.yaml");
    write(
        &source,
        r#"
package:
  name: solo
  version: "2.0"
source:
  url: https://example.invalid/solo-2.0.tar.gz
"#,
    );
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    let bundle = plan_new_package(&NewPackageRequest {
        source,
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: vec![CHECKSUM.to_string()],
        package_layers: Vec::new(),
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    })
    .expect("plan");
    let out = temp.path().join("out");
    let written = write_package_bundle(&bundle, &out).expect("write");
    assert!(written.manifest.is_file());
    assert!(out
        .join("easyconfigs/s/solo/solo-2.0-foss-2026.1.eb")
        .is_file());
    assert!(!out.join("build-order.json").exists());
    assert!(!out.join("closure.plan.json").exists());
}
