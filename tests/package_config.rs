use eb_stack::package::{
    materialize_profile, package_plan_to_cyclonedx, ConditionExpr, DependencyRole, EasyconfigValue,
    PatchArtifact, ProfileEnvironment, StackPolicy, STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::package_config::{apply_package_layers, DependencyAlias, PackageConfigLayer};
use eb_stack::{
    package_plan_from_foreign, parse_foreign_path, solve_package_profile, Candidate, ForeignFormat,
    Toolchain,
};
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
    plan.build.patches = vec![PatchArtifact {
        filename: "foreign-feedstock.patch".into(),
        sha256: None,
        source: None,
        resolved_source: None,
    }];
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
fn provider_alias_can_drop_a_component_version_constraint() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.aliases]
py-setuptools = { provider = "Python", constraint = "drop" }
"#,
    )
    .expect("structured provider alias");
    let mut plan = qmcpack_plan();
    {
        let dependency = plan.dependencies.first_mut().expect("fixture dependency");
        dependency.name = "py-setuptools".into();
        dependency.eb_name = None;
        dependency.constraint = Some("42:".into());
    }

    apply_package_layers(&mut plan, &[config]).expect("apply provider alias");

    let dependency = &plan.dependencies[0];
    assert_eq!(dependency.eb_name.as_deref(), Some("Python"));
    assert!(dependency.constraint.is_none());
}

#[test]
fn package_policy_models_typed_easyconfig_parameters_and_requirements() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[build.easyconfig_parameters]
general_packages = ["ASPHERE", "KSPACE", "MOLECULE"]
build_shared_libs = true
max_nbins = 16

[[build.patches]]
filename = "Orbit-2.0-portability.patch"
sha256 = "4f43b42fdcf84d0cf634d993dd944f252c8243dc612a919fe2825d56f937c8eb"

[[dependencies.requirements]]
name = "HDF5"
roles = ["run"]

[[dependencies.requirements]]
name = "CMake"
constraint = ">=3.30"
roles = ["build"]

[[profiles]]
name = "default"

[profiles.easyconfig_parameters]
with_tests = false
"#,
    )
    .expect("typed package policy");
    let mut plan = qmcpack_plan();

    apply_package_layers(&mut plan, &[config]).expect("apply typed package policy");

    assert_eq!(
        plan.build.easyconfig_parameters.get("general_packages"),
        Some(&EasyconfigValue::List(vec![
            EasyconfigValue::String("ASPHERE".into()),
            EasyconfigValue::String("KSPACE".into()),
            EasyconfigValue::String("MOLECULE".into()),
        ]))
    );
    assert_eq!(
        plan.build.easyconfig_parameters.get("build_shared_libs"),
        Some(&EasyconfigValue::Bool(true))
    );
    assert_eq!(
        plan.build.easyconfig_parameters.get("max_nbins"),
        Some(&EasyconfigValue::Integer(16))
    );
    assert_eq!(plan.build.patches.len(), 1);
    assert_eq!(
        plan.build.patches[0].filename,
        "Orbit-2.0-portability.patch"
    );
    assert_eq!(
        plan.build.patches[0].sha256.as_deref(),
        Some("4f43b42fdcf84d0cf634d993dd944f252c8243dc612a919fe2825d56f937c8eb")
    );
    assert_eq!(
        plan.profiles[0].easyconfig_parameters.get("with_tests"),
        Some(&EasyconfigValue::Bool(false))
    );

    let hdf5 = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.eb_name.as_deref() == Some("HDF5"))
        .expect("existing HDF5 intent is ensured");
    assert_eq!(hdf5.condition, ConditionExpr::Always);
    assert!(hdf5.roles.contains(&DependencyRole::Run));

    let cmake = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.eb_name.as_deref() == Some("CMake"))
        .expect("EasyBuild-only requirement enters the canonical plan");
    assert_eq!(cmake.constraint.as_deref(), Some(">=3.30"));
    assert_eq!(cmake.roles, [DependencyRole::Build]);
    assert_eq!(cmake.condition, ConditionExpr::Always);
}

#[test]
fn package_policy_rejects_python_fragments_as_easyconfig_parameter_names() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[build.easyconfig_parameters]
"general_packages; import os" = ["MOLECULE"]
"#,
    )
    .expect_err("parameter keys are EasyBuild identifiers, not Python");

    assert!(error.to_string().contains("general_packages; import os"));
}

#[test]
fn package_policy_requirement_reaches_the_sbom_and_resolvo_lock() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[dependencies.requirements]]
name = "NumericsRuntime"
constraint = ">=2"
roles = ["run"]
"#,
    )
    .expect("policy requirement");
    let mut plan = qmcpack_plan();
    plan.dependencies.clear();
    apply_package_layers(&mut plan, &[config]).expect("apply policy requirement");

    let sbom = package_plan_to_cyclonedx(&plan).expect("canonical SBOM");
    assert!(sbom["components"]
        .as_array()
        .expect("CycloneDX components")
        .iter()
        .any(|component| component["name"] == "NumericsRuntime"));

    let candidate = Candidate {
        name: "NumericsRuntime".into(),
        version: "2.4.0".into(),
        toolchain: plan.build.toolchain.clone(),
        versionsuffix: None,
        easyconfig_path: "NumericsRuntime-2.4.0-foss-2026.1.eb".into(),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    };
    let stack_policy = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "site-stack".into(),
        toolchain: plan.build.toolchain.clone(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    };
    let lock = solve_package_profile(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &[candidate],
        &stack_policy,
    )
    .expect("Resolvo lock");

    assert_eq!(lock.solver, "resolvo");
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0].name, "NumericsRuntime");
    assert_eq!(lock.dependencies[0].version, "2.4.0");
}

#[test]
fn public_qmcpack_policy_encodes_build_and_verification_contract() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = PackageConfigLayer::from_path(&root.join("examples/packages/qmcpack.toml"))
        .expect("QMCPACK package config");
    let package = config.package.as_ref().expect("package metadata");
    let build = config.build.as_ref().expect("build policy");

    assert_eq!(package.homepage.as_deref(), Some("https://qmcpack.org"));
    assert_eq!(package.license.as_deref(), Some("NCSA"));
    assert_eq!(build.easyblock.as_deref(), Some("CMakeNinja"));
    assert_eq!(
        build.build_systems.as_deref(),
        Some(&["CMake".into(), "Ninja".into()][..])
    );
    assert_eq!(build.moduleclass.as_deref(), Some("chem"));
    assert_eq!(
        build.easyconfig_parameters.get("start_dir"),
        Some(&EasyconfigValue::String("%(namelower)s-%(version)s".into()))
    );
    assert_eq!(
        build.easyconfig_parameters.get("test_cmd"),
        Some(&EasyconfigValue::String("ctest".into()))
    );
    assert_eq!(
        build.easyconfig_parameters.get("testopts"),
        Some(&EasyconfigValue::String(
            "-j %(parallel)s --output-on-failure -E 'performance|long'".into()
        ))
    );

    let Some(EasyconfigValue::Table(paths)) = build.easyconfig_parameters.get("sanity_check_paths")
    else {
        panic!("sanity_check_paths must be typed data");
    };
    assert_eq!(
        paths.get("files"),
        Some(&EasyconfigValue::List(vec![
            EasyconfigValue::String("bin/qmcpack".into()),
            EasyconfigValue::String("bin/convert4qmc".into()),
            EasyconfigValue::String("bin/ppconvert".into()),
            EasyconfigValue::String("bin/qmcpack.settings".into()),
        ]))
    );
    assert_eq!(
        paths.get("dirs"),
        Some(&EasyconfigValue::List(vec![EasyconfigValue::String(
            "lib/nexus".into()
        )]))
    );
    assert_eq!(
        build.easyconfig_parameters.get("sanity_check_commands"),
        Some(&EasyconfigValue::List(vec![EasyconfigValue::String(
            "qmcpack --version 2>&1 | grep -q 'QMCPACK version'".into()
        )]))
    );
    let Some(EasyconfigValue::Table(paths)) = build.easyconfig_parameters.get("modextrapaths")
    else {
        panic!("modextrapaths must be typed data");
    };
    assert_eq!(
        paths.get("PYTHONPATH"),
        Some(&EasyconfigValue::String("lib".into()))
    );
}

#[test]
fn public_eon_policy_encodes_the_repaired_build_contract() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = PackageConfigLayer::from_path(&root.join("examples/packages/eon.toml"))
        .expect("eOn package config");
    let package = config.package.as_ref().expect("package metadata");
    let build = config.build.as_ref().expect("build policy");

    assert_eq!(package.homepage.as_deref(), Some("https://eondocs.org/"));
    assert_eq!(package.license.as_deref(), Some("BSD-3-Clause"));
    assert_eq!(build.easyblock.as_deref(), Some("MesonNinja"));
    assert_eq!(
        build.build_systems.as_deref(),
        Some(&["Meson".into(), "Ninja".into()][..])
    );
    assert_eq!(build.source_root.as_deref(), Some("%(name)s-%(version)s"));
    assert_eq!(build.moduleclass.as_deref(), Some("chem"));
    let patches = build.patches.as_ref().expect("checked patch policy");
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].filename,
        "eOn-2.16.0_safemath-eigen5-core-guard.patch"
    );
    assert_eq!(
        patches[0].sha256.as_deref(),
        Some("34fd1abc414cccbfc2d454880f6df3136af2aa68c0bd65dc45c8894480a98e11")
    );
    assert!(patches[0]
        .resolved_source
        .as_ref()
        .is_some_and(|path| path.is_file()));

    let Some(EasyconfigValue::Concat(preconfig)) = build.easyconfig_parameters.get("preconfigopts")
    else {
        panic!("preconfigopts must be typed string fragments");
    };
    let preconfig = preconfig.concat.join("");
    for required in [
        "cargo cinstall --locked --release",
        "RUSTC_WRAPPER= CARGO_BUILD_RUSTC_WRAPPER=",
        "readcon-stage",
        "EBROOTMETATENSORMINTORCH",
        "EBROOTMETATOMICMINTORCH",
        "lib/python3.12/site-packages",
    ] {
        assert!(
            preconfig.contains(required),
            "preconfigopts missing {required}"
        );
    }
    assert!(!preconfig.contains("unset RUSTC_WRAPPER"));
    assert!(matches!(
        build.easyconfig_parameters.get("postinstallcmds"),
        Some(EasyconfigValue::List(commands)) if commands.len() >= 4
    ));
    assert!(matches!(
        build.easyconfig_parameters.get("sanity_check_paths"),
        Some(EasyconfigValue::Table(paths))
            if matches!(paths.get("files"), Some(EasyconfigValue::List(files)) if files.len() >= 4)
    ));
    assert!(matches!(
        build.easyconfig_parameters.get("sanity_check_commands"),
        Some(EasyconfigValue::List(commands)) if commands.len() >= 6
    ));

    let requirements = &config
        .dependencies
        .as_ref()
        .expect("EasyBuild product requirements")
        .requirements;
    for name in ["Rust", "cargo-c", "patchelf"] {
        assert!(requirements.iter().any(|requirement| {
            requirement.name == name && requirement.roles == [DependencyRole::Build]
        }));
    }
    let eigen = requirements
        .iter()
        .find(|requirement| requirement.name == "Eigen")
        .expect("Eigen 5 compatibility requirement");
    assert_eq!(eigen.constraint.as_deref(), Some("==5.0.0"));

    let common = PackageConfigLayer::from_path(&root.join("examples/packages/common.toml"))
        .expect("common package aliases");
    for (foreign, provider) in [
        ("libmetatensor", "metatensor"),
        ("libmetatensor-torch", "metatensor-torch"),
        ("libmetatomic-torch", "metatomic-torch"),
    ] {
        assert_eq!(
            common
                .dependencies
                .as_ref()
                .and_then(|dependencies| dependencies.aliases.get(foreign))
                .map(DependencyAlias::provider),
            Some(provider)
        );
    }

    let stack: StackPolicy = toml::from_str(
        &std::fs::read_to_string(root.join("examples/stacks/eon-foss-2026.1.toml"))
            .expect("eOn stack policy"),
    )
    .expect("parse eOn stack policy");
    let eigen_pin = stack
        .pins
        .iter()
        .find(|pin| pin.name == "Eigen")
        .expect("Eigen stack pin");
    assert_eq!(eigen_pin.version_requirement, "==5.0.0");
    assert_eq!(
        eigen_pin.toolchain,
        Some(Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        })
    );
    assert!(!stack.pins.iter().any(|pin| pin.name == "Meson"));
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
            .map(DependencyAlias::provider),
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
                .map(DependencyAlias::provider),
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
    assert_eq!(
        lammps
            .build
            .as_ref()
            .and_then(|build| build.patches.as_ref())
            .map(Vec::len),
        Some(3)
    );
    assert!(matches!(
        lammps
            .build
            .as_ref()
            .and_then(|build| build.easyconfig_parameters.get("general_packages")),
        Some(EasyconfigValue::List(packages)) if packages.len() > 80
    ));
    assert!(lammps.dependencies.as_ref().is_some_and(|dependencies| {
        dependencies
            .requirements
            .iter()
            .any(|requirement| requirement.name == "VTK")
            && dependencies.requirements.iter().any(|requirement| {
                requirement.name == "CMake" && requirement.roles == [DependencyRole::Build]
            })
    }));
}
