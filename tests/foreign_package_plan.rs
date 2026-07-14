use eb_stack::package::{package_plan_to_cyclonedx, PackageRuleKind};
use eb_stack::{package_plan_from_foreign, parse_foreign_path, ForeignFormat, Toolchain};
use std::collections::HashSet;
use std::path::PathBuf;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

#[test]
fn qmcpack_foreign_recipe_becomes_canonical_package_plan() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse QMCPACK");
    let plan = package_plan_from_foreign(&recipe, &toolchain());

    assert_eq!(plan.package.name, "QMCPACK");
    assert_eq!(plan.build.easyblock.as_deref(), Some("CMakeNinja"));
    assert_eq!(plan.outputs.len(), 1);
    assert_eq!(plan.outputs[0].profile, "default");
    assert_eq!(plan.outputs[0].stack, "foss-2026.1");

    let profile = plan
        .profiles
        .iter()
        .find(|profile| profile.default)
        .expect("default profile");
    assert_eq!(profile.features.get("mpi"), Some(&true));
    assert_eq!(profile.features.get("phdf5"), Some(&false));
    assert_eq!(profile.features.get("complex"), Some(&false));
    assert_eq!(profile.features.get("mixed"), Some(&false));
    assert_eq!(
        profile.parameters.get("build_type").map(String::as_str),
        Some("Release")
    );

    assert_eq!(
        plan.dependencies
            .iter()
            .filter(|dependency| dependency.name == "hdf5")
            .count(),
        2,
        "conditional HDF5 edges survive canonicalization"
    );
    assert_eq!(
        plan.rules
            .iter()
            .filter(|rule| rule.kind == PackageRuleKind::Conflict)
            .count(),
        19
    );
    assert_eq!(
        plan.rules
            .iter()
            .filter(|rule| rule.kind == PackageRuleKind::Requirement)
            .count(),
        2
    );
}

#[test]
fn eon_canonical_sbom_has_unique_component_references() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/conda_eon/recipe.yaml"),
        Some(ForeignFormat::CondaForge),
    )
    .expect("parse eOn");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    let sbom = package_plan_to_cyclonedx(&plan).expect("canonical SBOM");

    assert_eq!(plan.package.name, "eOn");
    assert_eq!(plan.profiles.len(), 1);
    assert!(plan.profiles[0].default);
    assert!(plan.profiles[0].versionsuffix.is_empty());
    assert_eq!(
        plan.dependencies
            .iter()
            .filter(|dependency| dependency.name == "libblas")
            .count(),
        2,
        "selector branches remain separate manifest edges"
    );

    let components = sbom["components"].as_array().expect("components");
    let references: Vec<&str> = components
        .iter()
        .filter_map(|component| component["bom-ref"].as_str())
        .collect();
    let unique: HashSet<&str> = references.iter().copied().collect();
    assert_eq!(
        references.len(),
        unique.len(),
        "CycloneDX bom-ref values must be unique: {references:?}"
    );
}

#[test]
fn conda_packaging_and_virtual_libraries_map_to_easybuild_conventions() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/conda_eon/recipe.yaml"),
        Some(ForeignFormat::CondaForge),
    )
    .expect("parse eOn");
    let plan = package_plan_from_foreign(&recipe, &toolchain());

    for name in ["pip", "setuptools"] {
        let dependency = plan
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(dependency.eb_name.as_deref(), Some("Python"));
        assert!(dependency.virtual_capability.is_none());
    }

    let numpy = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "numpy")
        .expect("numpy");
    assert_eq!(numpy.eb_name.as_deref(), Some("SciPy-bundle"));

    for (name, capability) in [
        ("libblas", "blas"),
        ("libcblas", "blas"),
        ("liblapack", "lapack"),
        ("liblapacke", "lapack"),
        ("cargo-bundle-licenses", "cargo-bundle-licenses"),
    ] {
        let dependency = plan
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(dependency.virtual_capability.as_deref(), Some(capability));
    }
    assert!(
        !plan
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "sccache"),
        "compiler wrappers and build accelerators are not package edges"
    );
}
