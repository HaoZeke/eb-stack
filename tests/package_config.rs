use eb_stack::package::{materialize_profile, ProfileEnvironment};
use eb_stack::package_config::{apply_package_layers, PackageConfigLayer};
use eb_stack::{package_plan_from_foreign, parse_foreign_path, ForeignFormat, Toolchain};
use std::path::PathBuf;

fn qmcpack_plan() -> eb_stack::package::PackagePlan {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/foreign_ingest/spack_qmcpack/package.py");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::Spack)).expect("parse QMCPACK");
    package_plan_from_foreign(
        &recipe,
        &Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        },
    )
}

#[test]
fn package_version_override_preserves_foreign_condition_identity() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let recipe = parse_foreign_path(
        &root.join("fixtures/foreign_ingest/spack_lammps/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse Spack fixture");
    let mut plan = package_plan_from_foreign(
        &recipe,
        &Toolchain {
            name: "foss".into(),
            version: "2025b".into(),
        },
    );
    let config = PackageConfigLayer::from_path(&root.join("examples/packages/lammps.toml"))
        .expect("package config");

    apply_package_layers(&mut plan, &[config]).expect("apply package identity");
    let materialized = materialize_profile(&plan, "default", &ProfileEnvironment::default())
        .expect("materialize configured profile");

    assert_eq!(plan.package.version, "22Jul2025_update4");
    assert!(materialized
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "kokkos"));
}

#[test]
fn layered_toml_profiles_materialize_easybuild_variants() {
    let base = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[profiles]]
name = "default"
default = true
config_options = [
  "-DQMC_MPI=ON",
  "-DQMC_OMP=ON",
  "-DQMC_COMPLEX=OFF",
  "-DQMC_MIXED_PRECISION=OFF",
]
verification_commands = [
  { program = "bash", args = ["-lc", "module load {module} && qmca --help"] },
]

[profiles.features]
mpi = true
phdf5 = false
complex = false
mixed = false

[profiles.toolchain_options]
usempi = true
openmp = true

[[profiles]]
name = "complex"
inherits = "default"
versionsuffix = ["-complex"]
config_options = [
  "-DQMC_MPI=ON",
  "-DQMC_OMP=ON",
  "-DQMC_COMPLEX=ON",
  "-DQMC_MIXED_PRECISION=OFF",
]

[profiles.features]
complex = true
"#,
    )
    .expect("base profile config");
    let site = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[profiles]]
name = "default"

[profiles.parameters]
build_type = "Release"

[[profiles]]
name = "complex"

[profiles.parameters]
build_type = "Release"
"#,
    )
    .expect("site profile config");

    let mut plan = qmcpack_plan();
    apply_package_layers(&mut plan, &[base, site]).expect("apply package layers");
    assert_eq!(plan.profiles.len(), 2);
    assert_eq!(plan.outputs.len(), 2);
    assert_eq!(plan.outputs[0].profile, "default");
    assert_eq!(plan.outputs[1].profile, "complex");

    let default = &plan.profiles[0];
    assert!(default.default);
    assert!(default.versionsuffix.is_empty());
    assert_eq!(default.toolchain_options.get("usempi"), Some(&true));
    assert_eq!(default.verification_commands.len(), 1);
    assert_eq!(default.verification_commands[0].program, "bash");
    assert_eq!(
        default.parameters.get("build_type").map(String::as_str),
        Some("Release")
    );

    let complex = &plan.profiles[1];
    assert!(!complex.default);
    assert_eq!(complex.versionsuffix, vec!["-complex"]);
    assert_eq!(complex.features.get("complex"), Some(&true));
    assert_eq!(complex.verification_commands.len(), 1);
    assert!(complex
        .config_options
        .iter()
        .any(|option| option == "-DQMC_COMPLEX=ON"));
}

#[test]
fn package_config_overrides_foreign_metadata_and_build_policy() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[package]
name = "LAMMPS"
version = "22Jul2025_update4"
homepage = "https://www.lammps.org/"
description = "LAMMPS molecular dynamics simulator"
license = "GPL-2.0-or-later"

[build]
easyblock = "LAMMPS"
build_systems = ["CMake"]
config_options = ["-DCMAKE_CXX_STANDARD=17"]
moduleclass = "chem"
patches = []

[dependencies]
exclude_from_solve = ["cmake"]

[dependencies.aliases]
hdf5 = "HDF5"

[dependencies.virtuals]
mpi = "MPI"

[[profiles]]
name = "default"
default = true
versionsuffix = ["-kokkos"]
"#,
    )
    .expect("package config");

    let mut plan = qmcpack_plan();
    plan.build.patches = vec!["foreign-feedstock.patch".into()];
    apply_package_layers(&mut plan, &[config]).expect("apply package config");

    assert_eq!(plan.package.name, "LAMMPS");
    assert_eq!(plan.package.version, "22Jul2025_update4");
    assert_eq!(
        plan.package.homepage.as_deref(),
        Some("https://www.lammps.org/")
    );
    assert_eq!(
        plan.package.description.as_deref(),
        Some("LAMMPS molecular dynamics simulator")
    );
    assert_eq!(plan.package.license.as_deref(), Some("GPL-2.0-or-later"));
    assert_eq!(plan.build.easyblock.as_deref(), Some("LAMMPS"));
    assert_eq!(plan.build.build_systems, ["CMake"]);
    assert_eq!(plan.build.config_options, ["-DCMAKE_CXX_STANDARD=17"]);
    assert_eq!(plan.build.moduleclass.as_deref(), Some("chem"));
    assert!(plan.build.patches.is_empty());
    assert_eq!(plan.profiles[0].versionsuffix, ["-kokkos"]);
    assert!(plan
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "hdf5")
        .all(|dependency| dependency.eb_name.as_deref() == Some("HDF5")));
    assert!(plan
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "cmake")
        .all(|dependency| dependency.solver_excluded));
    assert!(plan
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "mpi")
        .all(|dependency| dependency.virtual_capability.as_deref() == Some("MPI")));
}

#[test]
fn package_config_rejects_unknown_schema() {
    let error =
        PackageConfigLayer::from_toml_str("schema_version = 99").expect_err("unsupported schema");
    assert!(error.to_string().contains("schema version 99"), "{error}");
}

#[test]
fn auto_easyblock_defers_to_easybuild_software_selection() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1
[build]
easyblock = "auto"
"#,
    )
    .expect("automatic easyblock config");
    let mut plan = qmcpack_plan();
    assert!(plan.build.easyblock.is_some());

    apply_package_layers(&mut plan, &[config]).expect("apply automatic easyblock");
    assert!(plan.build.easyblock.is_none());
}

#[test]
fn public_package_config_examples_parse() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let common = PackageConfigLayer::from_path(&root.join("examples/packages/common.toml"))
        .expect("common foreign aliases");
    let eon = PackageConfigLayer::from_path(&root.join("examples/packages/eon.toml"))
        .expect("eOn package config");
    let qmcpack = PackageConfigLayer::from_path(&root.join("examples/packages/qmcpack.toml"))
        .expect("QMCPACK package config");
    let lammps = PackageConfigLayer::from_path(&root.join("examples/packages/lammps.toml"))
        .expect("LAMMPS package config");
    assert_eq!(
        common
            .dependencies
            .as_ref()
            .and_then(|dependencies| dependencies.aliases.get("py-numpy"))
            .map(String::as_str),
        Some("SciPy-bundle")
    );
    for (foreign, provider) in [
        ("py-pip", "Python"),
        ("py-wheel", "Python"),
        ("py-setuptools", "Python"),
        ("py-build", "build"),
        ("libnetcdf", "netCDF"),
        ("libpnetcdf", "PnetCDF"),
        ("libcurl", "cURL"),
        ("voro", "Voro++"),
    ] {
        assert_eq!(
            common
                .dependencies
                .as_ref()
                .and_then(|dependencies| dependencies.aliases.get(foreign))
                .map(String::as_str),
            Some(provider),
            "missing shared foreign-package alias for {foreign}"
        );
    }
    assert_eq!(eon.profiles.len(), 1);
    assert_eq!(
        eon.profiles[0]
            .verification_commands
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(qmcpack.profiles.len(), 2);
    assert_eq!(qmcpack.profiles[0].name, "default");
    assert_eq!(
        qmcpack.profiles[1].versionsuffix.as_deref(),
        Some(&["-complex".to_string()][..])
    );
    assert_eq!(
        lammps
            .package
            .as_ref()
            .and_then(|package| package.version.as_deref()),
        Some("22Jul2025_update4")
    );
    assert_eq!(
        lammps
            .profiles
            .first()
            .and_then(|profile| profile.versionsuffix.as_deref()),
        Some(&["-kokkos".to_string()][..])
    );
    assert_eq!(
        lammps
            .profiles
            .first()
            .and_then(|profile| profile.config_options.as_deref()),
        Some(&[][..])
    );
    assert_eq!(
        lammps.profiles[0].parameters.get("mpi").map(String::as_str),
        Some("openmpi")
    );
    assert!(lammps
        .dependencies
        .as_ref()
        .is_some_and(|dependencies| dependencies.exclude_from_solve == ["kokkos"]));
}
