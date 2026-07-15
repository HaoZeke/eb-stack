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
fn conda_lammps_expands_deterministic_date_templates() {
    let path = root().join("conda_lammps/meta.yaml");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::CondaForge))
        .expect("parse conda-forge LAMMPS recipe");

    assert_eq!(recipe.name, "lammps");
    assert_eq!(recipe.version, "2025.07.22");
    assert_eq!(recipe.sources.len(), 3);
    assert_eq!(
        recipe.sources[0].sha256.as_deref(),
        Some("411088d9c03339e025f6a975e0a5741bb9e3f351cc39eda220ab22ac318fe2fb")
    );
    assert!(recipe.sources[0]
        .url
        .as_deref()
        .is_some_and(|url| url.ends_with("stable_22Jul2025_update4.tar.gz")));
    assert_eq!(
        recipe.patches,
        [
            "macos_install.patch",
            "vcsgc_mtp_n2p2.patch",
            "fix-cython.patch",
            "matgl.patch",
        ]
    );
    for dependency in ["cmake", "fftw", "hdf5", "kim-api", "plumed"] {
        assert!(
            recipe
                .dependencies
                .iter()
                .any(|candidate| candidate.name == dependency),
            "missing {dependency}: {:?}",
            recipe.dependencies
        );
    }
    assert!(recipe.dependencies.iter().all(|dependency| dependency
        .pin
        .as_deref()
        .is_none_or(|pin| !pin.contains("{{"))));
}

#[test]
fn spack_lammps_honors_preference_and_materializes_sources() {
    let path = root().join("spack_lammps/package.py");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::Spack)).expect("parse Spack LAMMPS");

    assert_eq!(recipe.name, "lammps");
    assert_eq!(recipe.version, "20250722.4");
    assert_eq!(
        recipe.sources[0].sha256.as_deref(),
        Some("411088d9c03339e025f6a975e0a5741bb9e3f351cc39eda220ab22ac318fe2fb")
    );
    assert_eq!(
        recipe.sources[0].url.as_deref(),
        Some("https://github.com/lammps/lammps/archive/stable_22Jul2025_update4.tar.gz")
    );

    let potential = recipe
        .sources
        .iter()
        .find(|source| {
            source.url.as_deref() == Some("https://download.lammps.org/potentials/C_10_10.mesocnt")
        })
        .expect("MESONT potential resource");
    assert_eq!(
        potential.sha256.as_deref(),
        Some("923f600a081d948eb8b4510f84aa96167b5a6c3e1aba16845d2364ae137dc346")
    );
    assert_eq!(
        potential.target_directory.as_deref(),
        Some("potentials/C_10_10.mesocnt")
    );
}

#[test]
fn spack_lammps_splits_dependency_specs_from_variant_constraints() {
    let recipe = parse_foreign_path(
        &root().join("spack_lammps/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse Spack LAMMPS");

    assert!(recipe.dependencies.iter().all(|dependency| {
        !dependency.name.contains(char::is_whitespace) && !dependency.name.contains('+')
    }));
    let kokkos = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.original_spec.as_deref() == Some("kokkos+shared@3.1:"))
        .expect("Kokkos shared dependency");
    assert_eq!(kokkos.name, "kokkos");
    assert_eq!(kokkos.pin.as_deref(), Some("3.1:"));

    let scafacos = recipe
        .dependencies
        .iter()
        .find(|dependency| {
            dependency
                .original_spec
                .as_deref()
                .is_some_and(|spec| spec.starts_with("scafacos cflags=-fPIC"))
        })
        .expect("ScaFaCoS dependency");
    assert_eq!(scafacos.name, "scafacos");
}

#[test]
fn spack_lammps_splits_compound_when_predicates() {
    let recipe = parse_foreign_path(
        &root().join("spack_lammps/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse Spack LAMMPS");
    let dependency = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.original_spec.as_deref() == Some("kokkos@4.6.02:"))
        .expect("versioned Kokkos dependency");
    let condition = serde_json::to_string(&dependency.condition).expect("condition JSON");

    assert!(condition.contains("package-version"), "{condition}");
    assert!(condition.contains("\"name\":\"kokkos\""), "{condition}");
    assert!(condition.contains("\"name\":\"kspace\""), "{condition}");
    assert!(!condition.contains("kokkos+kspace"), "{condition}");
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
