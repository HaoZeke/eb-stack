use eb_stack::package::{
    materialize_profile, package_plan_to_cyclonedx, ConditionExpr, ConditionPredicate,
    DependencyIntent, DependencyRole, EasyconfigValue, PatchArtifact, ProfileEnvironment,
    StackPolicy, STACK_POLICY_SCHEMA_VERSION,
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
        url: None,
        source: None,
        condition: ConditionExpr::Always,
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
fn package_config_patch_merge_preserves_foreign_recipe_patches() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[build]
patches_mode = "merge"

[[build.patches]]
filename = "foreign-recipe.patch"
sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"

[[build.patches]]
filename = "easybuild-policy.patch"
sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#,
    )
    .expect("package config with merged patches");
    let mut plan = qmcpack_plan();
    plan.build.patches = vec![PatchArtifact {
        filename: "foreign-recipe.patch".into(),
        sha256: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        url: Some("https://example.invalid/foreign-recipe.patch".into()),
        source: Some("foreign-recipe.patch".into()),
        condition: ConditionExpr::Predicate(ConditionPredicate::Feature {
            name: "cuda".into(),
            enabled: true,
        }),
        resolved_source: None,
    }];

    apply_package_layers(&mut plan, &[config]).expect("merge package patches");

    assert_eq!(
        plan.build
            .patches
            .iter()
            .map(|patch| patch.filename.as_str())
            .collect::<Vec<_>>(),
        ["foreign-recipe.patch", "easybuild-policy.patch"]
    );
    assert_eq!(
        plan.build.patches[0].sha256.as_deref(),
        Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
    );
    assert_eq!(
        plan.build.patches[0].url.as_deref(),
        Some("https://example.invalid/foreign-recipe.patch")
    );
    assert_eq!(
        plan.build.patches[0].condition,
        ConditionExpr::Predicate(ConditionPredicate::Feature {
            name: "cuda".into(),
            enabled: true,
        })
    );
}

#[test]
fn package_profile_supplies_target_context_for_foreign_selectors() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[profiles]]
name = "default"
platform = "linux"
architecture = "x86_64"
"#,
    )
    .expect("package config with target context");
    let mut plan = qmcpack_plan();
    let mut linux = plan.dependencies[0].clone();
    linux.id = "linux-only".into();
    linux.name = "linux-only".into();
    linux.condition = ConditionExpr::Predicate(ConditionPredicate::Platform {
        name: "linux".into(),
    });
    let mut windows = linux.clone();
    windows.id = "windows-only".into();
    windows.name = "windows-only".into();
    windows.condition =
        ConditionExpr::Predicate(ConditionPredicate::Platform { name: "win".into() });
    plan.dependencies = vec![linux, windows];

    apply_package_layers(&mut plan, &[config]).expect("apply target context");
    let materialized = materialize_profile(&plan, "default", &ProfileEnvironment::default())
        .expect("materialize configured target");

    assert_eq!(
        materialized
            .dependencies
            .iter()
            .map(|dependency| dependency.name.as_str())
            .collect::<Vec<_>>(),
        ["linux-only"]
    );
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
fn provider_alias_rejects_unknown_table_keys() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.aliases]
py-numpy = { provider = "SciPy-bundle", constraints = "drop" }
"#,
    )
    .expect_err("unknown alias key");
    assert!(
        error.to_string().contains("constraints") || error.to_string().contains("unknown"),
        "{error}"
    );
}

#[test]
fn empty_virtual_capability_is_rejected() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.virtuals]
hdf5 = ""
"#,
    )
    .expect_err("empty virtual");
    assert!(
        error.to_string().contains("hdf5") && error.to_string().contains("empty"),
        "{error}"
    );
}

#[test]
fn empty_virtual_name_is_rejected() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.virtuals]
"" = "mpi"
"#,
    )
    .expect_err("empty virtual name");
    assert!(
        error.to_string().contains("virtual name") && error.to_string().contains("empty"),
        "{error}"
    );
}

#[test]
fn empty_alias_name_is_rejected() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.aliases]
"" = "HDF5"
"#,
    )
    .expect_err("empty alias name");
    assert!(
        error.to_string().contains("alias name") && error.to_string().contains("empty"),
        "{error}"
    );
}

#[test]
fn empty_alias_provider_is_rejected() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.aliases]
hdf5 = ""
"#,
    )
    .expect_err("empty alias");
    assert!(
        error.to_string().contains("hdf5") && error.to_string().contains("empty"),
        "{error}"
    );
    let table = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.aliases]
hdf5 = { provider = "" }
"#,
    )
    .expect_err("empty table provider");
    assert!(
        table.to_string().contains("hdf5") && table.to_string().contains("empty"),
        "{table}"
    );
}

#[test]
fn requirement_keeps_an_existing_provider_alias() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[dependencies.requirements]]
name = "hdf5"
roles = ["run"]
"#,
    )
    .expect("requirement");
    let mut plan = qmcpack_plan();
    plan.dependencies.push(DependencyIntent {
        id: "seed-hdf5".into(),
        name: "hdf5".into(),
        eb_name: Some("HDF5".into()),
        constraint: None,
        toolchain: None,
        versionsuffix: None,
        roles: vec![DependencyRole::Run],
        condition: ConditionExpr::Always,
        virtual_capability: None,
        solver_excluded: false,
        provenance: Vec::new(),
    });
    apply_package_layers(&mut plan, &[config]).expect("apply");
    let hdf5 = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.id == "seed-hdf5")
        .expect("seeded hdf5");
    assert_eq!(
        hdf5.eb_name.as_deref(),
        Some("HDF5"),
        "requirement must not undo the alias"
    );
}

#[test]
fn requirement_updates_every_same_identity_always_edge() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[dependencies.aliases]
py-numpy = { provider = "SciPy-bundle", constraint = "drop" }
py-scipy = { provider = "SciPy-bundle", constraint = "drop" }

[[dependencies.requirements]]
name = "SciPy-bundle"
constraint = "==1.2.3"
roles = ["run"]
"#,
    )
    .expect("aliases plus pin");
    let mut plan = qmcpack_plan();
    plan.dependencies = vec![
        DependencyIntent {
            id: "numpy".into(),
            name: "py-numpy".into(),
            eb_name: None,
            constraint: Some("1.26".into()),
            toolchain: None,
            versionsuffix: None,
            roles: vec![DependencyRole::Run],
            condition: ConditionExpr::Always,
            virtual_capability: None,
            solver_excluded: false,
            provenance: Vec::new(),
        },
        DependencyIntent {
            id: "scipy".into(),
            name: "py-scipy".into(),
            eb_name: None,
            constraint: Some("1.11".into()),
            toolchain: None,
            versionsuffix: None,
            roles: vec![DependencyRole::Run],
            condition: ConditionExpr::Always,
            virtual_capability: None,
            solver_excluded: false,
            provenance: Vec::new(),
        },
    ];
    apply_package_layers(&mut plan, &[config]).expect("apply");
    let pinned: Vec<_> = plan
        .dependencies
        .iter()
        .filter(|dependency| {
            dependency.eb_name.as_deref() == Some("SciPy-bundle")
                && dependency.constraint.as_deref() == Some("==1.2.3")
        })
        .collect();
    assert_eq!(
        pinned.len(),
        2,
        "both aliased Always rows must carry the pin: {:?}",
        plan.dependencies
    );
}

#[test]
fn inherits_applies_after_the_parent_patch_in_the_same_layer() {
    let config = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1

[[profiles]]
name = "complex"
inherits = "default"
versionsuffix = ["-complex"]

[[profiles]]
name = "default"
default = true
config_options = ["-DQMC_COMPLEX=OFF"]
"#,
    )
    .expect("child listed first");
    let mut plan = qmcpack_plan();
    apply_package_layers(&mut plan, &[config]).expect("apply");
    let complex = plan
        .profiles
        .iter()
        .find(|profile| profile.name == "complex")
        .expect("complex");
    assert_eq!(
        complex.config_options,
        vec!["-DQMC_COMPLEX=OFF".to_string()],
        "child must inherit the same-layer parent update"
    );
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

    let hdf5_always = plan
        .dependencies
        .iter()
        .find(|dependency| {
            dependency.eb_name.as_deref() == Some("HDF5")
                && dependency.condition == ConditionExpr::Always
        })
        .expect("Always HDF5 is a new intent, not a hijacked conditional");
    assert!(hdf5_always.roles.contains(&DependencyRole::Run));
    let hdf5_gated = plan
        .dependencies
        .iter()
        .filter(|dependency| {
            let identity = dependency
                .eb_name
                .as_deref()
                .unwrap_or(dependency.name.as_str());
            identity.eq_ignore_ascii_case("hdf5") && dependency.condition != ConditionExpr::Always
        })
        .count();
    assert_eq!(
        hdf5_gated, 2,
        "feature-gated HDF5 siblings stay after an Always requirement"
    );

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
fn source_checksums_zip_onto_every_plan_source() {
    let digest_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let digest_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let config = PackageConfigLayer::from_toml_str(&format!(
        "schema_version = 1\nsource_checksums = [\"{digest_a}\", \"{digest_b}\"]\n"
    ))
    .expect("layer");
    let mut plan = qmcpack_plan();
    plan.sources = vec![
        eb_stack::package::SourceArtifact {
            filename: Some("a.tar.gz".into()),
            ..eb_stack::package::SourceArtifact::default()
        },
        eb_stack::package::SourceArtifact {
            filename: Some("b.tar.gz".into()),
            ..eb_stack::package::SourceArtifact::default()
        },
    ];
    apply_package_layers(&mut plan, &[config]).expect("zip checksums");
    assert_eq!(plan.sources[0].sha256.as_deref(), Some(digest_a));
    assert_eq!(plan.sources[1].sha256.as_deref(), Some(digest_b));
}

#[test]
fn source_checksums_count_must_match_plan_sources() {
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let config = PackageConfigLayer::from_toml_str(&format!(
        "schema_version = 1\nsource_checksums = [\"{digest}\"]\n"
    ))
    .expect("layer");
    let mut plan = qmcpack_plan();
    plan.sources = vec![
        eb_stack::package::SourceArtifact::default(),
        eb_stack::package::SourceArtifact::default(),
    ];
    let error = apply_package_layers(&mut plan, &[config]).expect_err("count mismatch");
    assert!(
        error.to_string().contains("source_checksums has 1"),
        "{error}"
    );
}

#[test]
fn source_checksums_do_not_invent_sources_on_an_empty_plan() {
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let config = PackageConfigLayer::from_toml_str(&format!(
        "schema_version = 1\nsource_checksums = [\"{digest}\"]\n"
    ))
    .expect("layer");
    let mut plan = qmcpack_plan();
    plan.sources.clear();
    let error = apply_package_layers(&mut plan, &[config]).expect_err("empty plan");
    assert!(
        error.to_string().contains("source_checksums has 1")
            && error.to_string().contains("plan has 0"),
        "{error}"
    );
    assert!(plan.sources.is_empty(), "{:?}", plan.sources);
}

#[test]
fn patch_layer_sha256_must_be_sixty_four_hex() {
    let error = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1
[[build.patches]]
filename = "portability.patch"
sha256 = "abcd"
"#,
    )
    .expect_err("short patch digest");
    assert!(
        error.to_string().contains("patch checksum") && error.to_string().contains("abcd"),
        "{error}"
    );
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
        moduleclass: None,
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
            "-L deterministic -j %(parallel)s --output-on-failure \
             -E 'unit_test_message-r12|unit_test_new_drivers_mpi-r16'"
                .into()
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
    let dependency_policy = config
        .dependencies
        .as_ref()
        .expect("QMCPACK dependency policy");
    for name in ["Ninja", "pkgconf"] {
        assert!(dependency_policy.requirements.iter().any(|requirement| {
            requirement.name == name && requirement.roles == vec![DependencyRole::Build]
        }));
    }
    assert!(dependency_policy.requirements.iter().any(|requirement| {
        requirement.name == "Boost" && requirement.roles == vec![DependencyRole::Run]
    }));
}

#[test]
fn public_eon_policy_encodes_the_core_rgpot_contract() {
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
        Some("fccc4ceb74f2ec7b225142071fcc2dde07f89df637f1aa0e57404831e650a59c")
    );
    assert!(patches[0]
        .resolved_source
        .as_ref()
        .is_some_and(|path| path.is_file()));

    // Core + rgpot product: no staging scripts, no install rewrites. The
    // emitted recipe must stay a conventional MesonNinja easyconfig.
    assert!(!build.easyconfig_parameters.contains_key("preconfigopts"));
    assert!(!build.easyconfig_parameters.contains_key("postinstallcmds"));
    assert!(matches!(
        build.easyconfig_parameters.get("sanity_check_paths"),
        Some(EasyconfigValue::Table(paths))
            if matches!(paths.get("files"), Some(EasyconfigValue::List(files)) if files.len() >= 2)
    ));
    assert!(matches!(
        build.easyconfig_parameters.get("sanity_check_commands"),
        Some(EasyconfigValue::List(commands)) if commands.len() >= 3
    ));

    let dependencies = config
        .dependencies
        .as_ref()
        .expect("EasyBuild product requirements");
    for excluded in ["PyTorch", "metatensor-torch", "metatomic-torch", "xtb"] {
        assert!(
            dependencies
                .exclude_from_solve
                .iter()
                .any(|name| name == excluded),
            "fat-product dependency {excluded} must stay out of the core solve"
        );
    }
    assert_eq!(
        dependencies
            .aliases
            .get("libreadcon-core")
            .map(DependencyAlias::provider),
        Some("readcon-core")
    );
    let requirements = &dependencies.requirements;
    for name in ["Meson", "Ninja", "pkgconf", "CMake"] {
        assert!(requirements.iter().any(|requirement| {
            requirement.name == name && requirement.roles == [DependencyRole::Build]
        }));
    }
    assert!(!requirements
        .iter()
        .any(|requirement| ["Rust", "cargo-c", "patchelf"].contains(&requirement.name.as_str())));
    for name in [
        "Python",
        "SciPy-bundle",
        "PyYAML",
        "Highway",
        "inih",
        "quill",
    ] {
        assert!(
            requirements.iter().any(|requirement| {
                requirement.name == name && requirement.roles == [DependencyRole::Run]
            }),
            "core product requires {name} as a runtime dependency"
        );
    }
    for (name, constraint) in [
        ("Eigen", "==5.0.0"),
        ("readcon-core", "==0.13.1"),
        ("rgpot", "==2.5.3"),
    ] {
        let requirement = requirements
            .iter()
            .find(|requirement| requirement.name == name)
            .unwrap_or_else(|| panic!("missing pinned requirement {name}"));
        assert_eq!(requirement.constraint.as_deref(), Some(constraint));
    }

    let profile = &config.profiles[0];
    let options = profile.config_options.as_deref().unwrap_or_default();
    assert!(options.iter().any(|option| option == "-Dwith_rgpot=true"));
    for fat in [
        "-Dwith_metatomic=true",
        "-Dwith_xtb=true",
        "-Dwith_serve=true",
    ] {
        assert!(
            !options.iter().any(|option| option == fat),
            "core profile must not enable {fat}"
        );
    }

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
    // Single-generation policy: no cross-generation PyTorch/xtb pins survive.
    for cross in ["PyTorch", "xtb", "Meson"] {
        assert!(
            !stack.pins.iter().any(|pin| pin.name == cross),
            "stack policy must not pin {cross}"
        );
    }
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

/// Optional fields must stay optional.
///
/// The `#[serde(default)]` attributes are easy to drop while editing a struct
/// for unrelated reasons, and nothing else in the suite notices: every fixture
/// happens to spell the fields out. A layer that omits them is the real shape a
/// user writes, so it is worth one test of its own.
#[test]
fn a_layer_may_omit_every_optional_field() {
    let layer = PackageConfigLayer::from_toml_str("schema_version = 1\n")
        .expect("a layer that sets nothing is valid");
    assert!(layer.package.is_none());
    assert!(layer.build.is_none());
    assert!(layer.dependencies.is_none());
    assert!(layer.profiles.is_empty());

    let minimal = PackageConfigLayer::from_toml_str(
        "schema_version = 1\n\
         [[profiles]]\n\
         name = \"default\"\n\
         [package]\n\
         name = \"Thing\"\n\
         [build]\n\
         easyblock = \"CMakeMake\"\n\
         [dependencies]\n",
    );
    let minimal = match minimal {
        Ok(layer) => layer,
        Err(error) => panic!("minimal layer rejected: {error}"),
    };
    let profile = &minimal.profiles[0];
    assert!(profile.inherits.is_none());
    assert!(profile.features.is_empty());
    assert!(profile.toolchain_options.is_empty());
    let build = minimal.build.as_ref().expect("build section");
    assert!(build.patches.is_none());
    assert!(build.easyconfig_parameters.is_empty());
}

/// A dependency requirement without `roles` defaults to run-only.
#[test]
fn a_requirement_defaults_to_the_run_role() {
    let layer = PackageConfigLayer::from_toml_str(
        "schema_version = 1\n\
         [dependencies]\n\
         [[dependencies.requirements]]\n\
         name = \"HDF5\"\n",
    )
    .expect("requirement without roles is valid");
    let requirement = &layer.dependencies.as_ref().unwrap().requirements[0];
    assert_eq!(requirement.roles, vec![DependencyRole::Run]);
}
