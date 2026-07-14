//! Syntax-adapter regression for conda-forge and Spack inputs.

use eb_stack::{
    detect_foreign_format, inspect_new_package, package_plan_from_foreign, parse_foreign_path,
    ForeignFormat, Toolchain,
};
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/foreign_ingest")
}

fn toolchain(version: &str) -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: version.into(),
    }
}

#[test]
fn conda_zlib_parses_into_a_canonical_plan() {
    let path = root().join("conda_zlib/meta.yaml");
    assert_eq!(
        detect_foreign_format(&path),
        Some(ForeignFormat::CondaForge)
    );
    let recipe = parse_foreign_path(&path, None).expect("parse conda fixture");
    assert_eq!(recipe.name, "zlib");
    assert_eq!(recipe.version, "1.3.1");
    assert!(recipe
        .source_url
        .as_ref()
        .is_some_and(|url| url.contains("zlib-1.3.1.tar.gz")));
    let plan = package_plan_from_foreign(&recipe, &toolchain("2024a"));
    assert_eq!(plan.package.name, "zlib");
    assert_eq!(plan.sources[0].sha256.as_deref(), recipe.sha256.as_deref());
    assert!(plan
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "make"));
    assert!(plan
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "libgcc-ng"));
}

#[test]
fn conda_eon_expands_context_multi_source_and_selectors() {
    let path = root().join("conda_eon/recipe.yaml");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::CondaForge)).expect("eOn");
    assert_eq!(recipe.version, "2.16.0");
    assert!(recipe.sources.len() >= 3);
    assert!(recipe
        .sha256
        .as_ref()
        .is_some_and(|checksum| checksum.len() == 64));
    assert!(!recipe
        .dependencies
        .iter()
        .any(|dependency| dependency.name.contains("compiler(")));
    let plan = package_plan_from_foreign(&recipe, &toolchain("2026.1"));
    assert_eq!(plan.package.name, "eOn");
    assert_eq!(plan.origin, eb_stack::package::PackageOrigin::CondaForge);
    assert_eq!(plan.build.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(plan.sources.len(), recipe.sources.len());
    assert!(plan
        .dependencies
        .iter()
        .any(|dependency| dependency.name.contains("metatomic") || dependency.name == "xtb"));
}

#[test]
fn spack_zlib_and_eon_preserve_build_system_information() {
    let zlib_path = root().join("spack_zlib/package.py");
    assert_eq!(
        detect_foreign_format(&zlib_path),
        Some(ForeignFormat::Spack)
    );
    let zlib = parse_foreign_path(&zlib_path, None).expect("Spack zlib");
    assert_eq!(zlib.version, "1.3.1");
    assert!(zlib
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "gmake"));

    let eon = parse_foreign_path(&root().join("spack_eon/package.py"), None).expect("Spack eOn");
    assert_eq!(eon.version, "2.16.0");
    assert!(eon
        .build_system_hints
        .iter()
        .any(|hint| hint.contains("Meson")));
    assert!(eon
        .configopts
        .as_deref()
        .is_some_and(|options| options.contains("-Dwith_")));
    let plan = package_plan_from_foreign(&eon, &toolchain("2026.1"));
    assert_eq!(plan.build.easyblock.as_deref(), Some("MesonNinja"));
}

#[test]
fn spack_qmcpack_preserves_variants_rules_and_conditions() {
    let recipe =
        parse_foreign_path(&root().join("spack_qmcpack/package.py"), None).expect("QMCPACK");
    assert_eq!(recipe.name, "qmcpack");
    assert_eq!(recipe.version, "4.3.0");
    assert!(recipe
        .build_system_hints
        .iter()
        .any(|hint| hint.contains("CMake")));
    assert!(recipe
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "hdf5"));
    assert!(recipe
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "python"));
    assert!(
        recipe.patches.is_empty(),
        "unresolved Spack patch directives are residuals, not EasyBuild patch filenames"
    );
    assert!(recipe
        .notes
        .iter()
        .any(|note| note.contains("3 patch() directive")));
    let plan = package_plan_from_foreign(&recipe, &toolchain("2026.1"));
    assert_eq!(plan.package.name, "QMCPACK");
    assert_eq!(plan.rules.len(), recipe.rules.len());
    assert!(plan.rules.len() >= 10);
    assert!(plan.dependencies.iter().any(|dependency| {
        dependency.name == "mpi" && dependency.virtual_capability.as_deref() == Some("mpi")
    }));
}

#[test]
fn inspect_cli_writes_manifest_sbom_and_embedded_residuals() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    for (source, format, expected) in [
        ("conda_eon/recipe.yaml", "conda-forge", "eOn"),
        ("spack_qmcpack/package.py", "spack", "QMCPACK"),
    ] {
        let output = temp.path().join(expected);
        let result = Command::new(binary)
            .args([
                "package",
                "inspect",
                "--source",
                root().join(source).to_str().unwrap(),
                "--format",
                format,
                "--toolchain-version",
                "2026.1",
                "--out-dir",
                output.to_str().unwrap(),
            ])
            .output()
            .expect("package inspect");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(output.join("package.plan.json")).expect("manifest"),
        )
        .expect("manifest JSON");
        assert_eq!(manifest["package"]["name"], expected);
        assert!(manifest["residuals"].is_array());
        assert!(output.join("package.sbom.cdx.json").is_file());
    }
}

#[test]
fn inspect_library_matches_cli_artifact_identity() {
    let source = root().join("spack_qmcpack/package.py");
    let (plan, sbom) = inspect_new_package(
        &source,
        Some(ForeignFormat::Spack),
        &toolchain("2026.1"),
        &[],
    )
    .expect("inspect library");
    assert_eq!(plan.package.name, "QMCPACK");
    assert_eq!(sbom["metadata"]["component"]["name"], "QMCPACK");
    assert_eq!(sbom["bomFormat"], "CycloneDX");
}
