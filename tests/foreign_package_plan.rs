use eb_stack::package::{package_plan_to_cyclonedx, PackageRuleKind, ProfileLock};
use eb_stack::{
    emit_profile_easyconfigs, package_plan_from_foreign, parse_foreign_path, parse_foreign_str,
    ForeignFormat, Toolchain,
};
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

    assert_eq!(plan.package.name, "qmcpack");
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

    assert_eq!(plan.package.name, "eon");
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
fn foreign_dependency_names_are_preserved_until_resolution_policy() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/conda_eon/recipe.yaml"),
        Some(ForeignFormat::CondaForge),
    )
    .expect("parse eOn");
    let plan = package_plan_from_foreign(&recipe, &toolchain());

    for name in ["pip", "setuptools", "numpy", "libtorch"] {
        let dependency = plan
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert!(dependency.eb_name.is_none());
        assert!(dependency.virtual_capability.is_none());
    }

    let libtorch = plan
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "libtorch")
        .collect::<Vec<_>>();
    assert!(!libtorch.is_empty());
    assert!(
        libtorch
            .iter()
            .all(|dependency| dependency.constraint.is_none()),
        "conda build strings are not package versions: {libtorch:?}"
    );

    for name in ["libblas", "libcblas", "liblapack", "liblapacke"] {
        let dependency = plan
            .dependencies
            .iter()
            .find(|dependency| dependency.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(dependency.virtual_capability.as_deref(), Some(name));
    }
    assert!(
        !plan
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "sccache"),
        "compiler wrappers and build accelerators are not package edges"
    );
}

#[test]
fn spack_build_plus_link_is_a_runtime_edge() {
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let recipe = parse_foreign_str(
        ForeignFormat::Spack,
        &format!(
            r#"
class Pkg(Package):
    version("1.0", sha256="{digest}")
    depends_on("hdf5", type=("build", "link"))
"#
        ),
    )
    .expect("parse");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    let hdf5 = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "hdf5")
        .expect("hdf5");
    assert!(
        hdf5.roles
            .iter()
            .any(|role| matches!(role, eb_stack::package::DependencyRole::Run)),
        "{hdf5:?}"
    );
}

#[test]
fn raku_trailing_plus_is_a_lower_bound() {
    let recipe = parse_foreign_str(
        ForeignFormat::Raku,
        r#"{
          "name": "Demo",
          "version": "0.1.0",
          "depends": ["JSON::Fast:ver<0.10+>"]
        }"#,
    )
    .expect("parse");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    let dep = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "JSON::Fast")
        .expect("JSON::Fast");
    assert_eq!(dep.constraint.as_deref(), Some(">=0.10"));
}

#[test]
fn autotools_is_not_reported_as_a_default_easyblock() {
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let recipe = parse_foreign_str(
        ForeignFormat::Spack,
        &format!(
            r#"
class Pkg(AutotoolsPackage):
    version("1.0", sha256="{digest}")
"#
        ),
    )
    .expect("parse");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    assert_eq!(plan.build.easyblock.as_deref(), Some("ConfigureMake"));
    assert!(
        plan.residuals
            .iter()
            .all(|residual| residual.id != "easyblock:default"),
        "{:?}",
        plan.residuals
    );
}

#[test]
fn cran_sanity_dirs_are_under_the_r_library() {
    let recipe = parse_foreign_str(ForeignFormat::Cran, "Package: jsonlite\nVersion: 1.8.8\n")
        .expect("parse");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    let paths = plan.build.easyconfig_parameters.get("sanity_check_paths");
    let Some(eb_stack::package::EasyconfigValue::Table(table)) = paths else {
        panic!("sanity_check_paths: {paths:?}");
    };
    let Some(eb_stack::package::EasyconfigValue::List(dirs)) = table.get("dirs") else {
        panic!("dirs: {table:?}");
    };
    assert!(
        dirs.iter().any(|value| matches!(
            value,
            eb_stack::package::EasyconfigValue::String(path) if path == "lib/R/library/jsonlite"
        )),
        "{dirs:?}"
    );
}

#[test]
fn luarocks_hint_selects_luarocks_and_skips_default_residual() {
    let recipe = parse_foreign_str(
        ForeignFormat::Luarocks,
        r#"
package = "lfs"
version = "1.8.0-1"
source = { url = "https://example.invalid/lfs-1.8.0.tar.gz" }
"#,
    )
    .expect("parse");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    assert_eq!(plan.build.easyblock.as_deref(), Some("LuaRocks"));
    assert!(
        plan.residuals
            .iter()
            .all(|residual| residual.id != "easyblock:default"),
        "{:?}",
        plan.residuals
    );
}

#[test]
fn luarocks_git_scm_url_emits_git_config() {
    let recipe = parse_foreign_str(
        ForeignFormat::Luarocks,
        r#"
package = "lfs"
version = "1.8.0-1"
source = {
  url = "git+https://github.com/lunarmodules/luafilesystem.git",
  tag = "v1.8.0"
}
"#,
    )
    .expect("parse");
    assert_eq!(
        recipe.sources[0].git.as_deref(),
        Some("https://github.com/lunarmodules/luafilesystem.git")
    );
    assert!(recipe.sources[0].url.is_none());
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    let lock = ProfileLock {
        schema_version: eb_stack::package::PROFILE_LOCK_SCHEMA_VERSION,
        package: plan.package.name.clone(),
        version: plan.package.version.clone(),
        profile: "default".into(),
        toolchain: toolchain(),
        versionsuffix: String::new(),
        dependencies: Vec::new(),
        pin_outcomes: Vec::new(),
        exclusions: Vec::new(),
        solver: "resolvo".into(),
    };
    let emitted = emit_profile_easyconfigs(&plan, &[lock]).expect("emit");
    assert!(
        emitted[0].text.contains("git_config"),
        "checkout must emit git_config:\n{}",
        emitted[0].text
    );
    assert!(
        !emitted[0].text.contains("git+https://"),
        "must not wget the SCM URL:\n{}",
        emitted[0].text
    );
}

#[test]
fn luarocks_pessimistic_pin_is_a_series_range() {
    let recipe = parse_foreign_str(
        ForeignFormat::Luarocks,
        r#"
package = "demo"
version = "1.0-1"
source = { url = "https://example.invalid/demo.tgz" }
dependencies = {
  "lfs ~> 1.8",
  "bit32 ~= 1.0"
}
"#,
    )
    .expect("parse");
    let plan = package_plan_from_foreign(&recipe, &toolchain());
    let lfs = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "lfs")
        .expect("lfs");
    assert_eq!(lfs.constraint.as_deref(), Some(">=1.8,<1.9"));
    let bit32 = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "bit32")
        .expect("bit32");
    assert_eq!(bit32.constraint.as_deref(), Some("!=1.0"));
}
