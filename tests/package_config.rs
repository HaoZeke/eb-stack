use eb_stack::package_config::{apply_profile_layers, ProfileConfigLayer};
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
fn layered_toml_profiles_materialize_easybuild_variants() {
    let base = ProfileConfigLayer::from_toml_str(
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
    let site = ProfileConfigLayer::from_toml_str(
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
    apply_profile_layers(&mut plan, &[base, site]).expect("apply profile layers");
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
fn profile_config_rejects_unknown_schema() {
    let error =
        ProfileConfigLayer::from_toml_str("schema_version = 99").expect_err("unsupported schema");
    assert!(error.to_string().contains("schema version 99"), "{error}");
}
