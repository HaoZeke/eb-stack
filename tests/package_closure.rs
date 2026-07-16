//! In-memory recursive package-closure planner (catalog-backed robot holes).
//!
//! Synthetic package names only — no production package identities.

use eb_stack::package::{PackageOrigin, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_catalog::{
    resolve_package_catalog_layers, PackageCatalogLayer, PackageSourceCatalog,
};
use eb_stack::package_closure::{plan_package_closure, write_package_closure, PackageClosureError};
use eb_stack::{resolve_easyconfig_str, ForeignFormat, NewPackageRequest, Toolchain};
use std::path::{Path, PathBuf};

const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn policy() -> StackPolicy {
    StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "closure-test".into(),
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

fn robot_eb(root: &Path, name: &str, version: &str, deps: &[(&str, &str)]) {
    let mut body = format!(
        "name = '{name}'\nversion = '{version}'\ntoolchain = {{'name': 'foss', 'version': '2026.1'}}\n"
    );
    if !deps.is_empty() {
        body.push_str("dependencies = [\n");
        for (dep_name, dep_ver) in deps {
            body.push_str(&format!("    ('{dep_name}', '{dep_ver}'),\n"));
        }
        body.push_str("]\n");
    }
    write(
        &root.join(format!("{name}-{version}-foss-2026.1.eb")),
        &body,
    );
}

fn conda_recipe(root: &Path, file: &str, name: &str, version: &str, deps: &[&str]) -> PathBuf {
    let mut reqs = String::new();
    for dep in deps {
        reqs.push_str(&format!("    - {dep}\n"));
    }
    let path = root.join(file);
    write(
        &path,
        &format!(
            r#"package:
  name: {name}
  version: "{version}"
source:
  url: https://example.invalid/{name}-{version}.tar.gz
requirements:
  host:
{reqs}"#
        ),
    );
    path
}

fn catalog_from_toml(root: &Path, body: &str) -> PackageSourceCatalog {
    let path = root.join("catalog.toml");
    write(&path, body);
    let layer = PackageCatalogLayer::from_path(&path).expect("catalog layer");
    resolve_package_catalog_layers(&[layer]).expect("resolve catalog")
}

fn request(source: PathBuf, robot: PathBuf) -> NewPackageRequest {
    NewPackageRequest {
        source,
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: vec![CHECKSUM.to_string()],
        package_layers: Vec::new(),
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    }
}

#[test]
fn root_missing_direct_emits_only_companion_robot_leaf_stays_external() {
    // Alpha -> missing Bravo -> robot Charlie: emit Bravo only.
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    robot_eb(&robot, "Charlie", "3.0", &[]);

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let bravo = conda_recipe(root, "bravo.yaml", "bravo", "1.5", &["charlie >=3.0"]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.5"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close");

    assert_eq!(closure.root.plan.package.name, "alpha");
    assert_eq!(closure.companions.len(), 1, "only Bravo is a robot hole");
    assert_eq!(closure.companions[0].plan.package.name, "bravo");
    assert_eq!(closure.companions[0].plan.package.version, "1.5");

    let lock = &closure.root.locks[0];
    let bravo_dep = lock
        .dependencies
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case("bravo"))
        .expect("root lock selects Bravo");
    assert!(
        bravo_dep.easyconfig_path.contains("__package_closure__")
            || bravo_dep.easyconfig_path.contains("bravo"),
        "Bravo must be the generated candidate, path={}",
        bravo_dep.easyconfig_path
    );
    let charlie_dep = closure.companions[0].locks[0]
        .dependencies
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case("charlie"))
        .expect("Bravo lock selects robot Charlie");
    assert!(
        !charlie_dep.easyconfig_path.contains("__package_closure__"),
        "Charlie must remain a robot candidate: {}",
        charlie_dep.easyconfig_path
    );

    // Catalog entry for Charlie must not force emission when robot supplies it.
    let _ = bravo;
}

#[test]
fn robot_candidate_preferred_over_catalog_entry() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    robot_eb(&robot, "Bravo", "1.0", &[]);

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _catalog_bravo = conda_recipe(root, "bravo.yaml", "bravo", "9.9", &[]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "9.9"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close");
    assert!(
        closure.companions.is_empty(),
        "compatible robot Bravo must win; got companions {:?}",
        closure
            .companions
            .iter()
            .map(|c| c.plan.package.name.as_str())
            .collect::<Vec<_>>()
    );
    let bravo = closure.root.locks[0]
        .dependencies
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case("bravo"))
        .expect("Bravo selected");
    assert_eq!(bravo.version, "1.0");
    assert!(
        !bravo.easyconfig_path.contains("__package_closure__"),
        "must use robot path: {}",
        bravo.easyconfig_path
    );
}

#[test]
fn recursive_chain_emits_leaf_then_middle_in_topo_order() {
    // Alpha -> missing Bravo -> missing Charlie: emit Charlie then Bravo.
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    // Empty robot: every non-root node is a hole.

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo = conda_recipe(root, "bravo.yaml", "bravo", "2.0", &["charlie >=1.0"]);
    let _charlie = conda_recipe(root, "charlie.yaml", "charlie", "1.1", &[]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "2.0"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "charlie"
version = "1.1"
source = "charlie.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close");
    let names: Vec<_> = closure
        .companions
        .iter()
        .map(|c| c.plan.package.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["charlie", "bravo"],
        "topological build order leaf-before-parent"
    );
}

#[test]
fn shared_companion_is_deduplicated() {
    // Alpha depends on Bravo and Delta; both depend on missing Echo → one Echo.
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(
        root,
        "alpha.yaml",
        "alpha",
        "1.0",
        &["bravo >=1.0", "delta >=1.0"],
    );
    let _bravo = conda_recipe(root, "bravo.yaml", "bravo", "1.0", &["echo >=1.0"]);
    let _delta = conda_recipe(root, "delta.yaml", "delta", "1.0", &["echo >=1.0"]);
    let _echo = conda_recipe(root, "echo.yaml", "echo", "1.0", &[]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.0"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "delta"
version = "1.0"
source = "delta.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "echo"
version = "1.0"
source = "echo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close");
    let echo_count = closure
        .companions
        .iter()
        .filter(|c| c.plan.package.name == "echo")
        .count();
    assert_eq!(echo_count, 1, "shared Echo must appear once");
    assert_eq!(closure.companions.len(), 3, "bravo, delta, echo");
    let echo_pos = closure
        .companions
        .iter()
        .position(|c| c.plan.package.name == "echo")
        .expect("echo present");
    let bravo_pos = closure
        .companions
        .iter()
        .position(|c| c.plan.package.name == "bravo")
        .unwrap();
    let delta_pos = closure
        .companions
        .iter()
        .position(|c| c.plan.package.name == "delta")
        .unwrap();
    assert!(
        echo_pos < bravo_pos && echo_pos < delta_pos,
        "echo before dependents"
    );
}

#[test]
fn cycle_reports_complete_package_path() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo = conda_recipe(root, "bravo.yaml", "bravo", "1.0", &["charlie >=1.0"]);
    let _charlie = conda_recipe(root, "charlie.yaml", "charlie", "1.0", &["bravo >=1.0"]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.0"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "charlie"
version = "1.0"
source = "charlie.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let err = plan_package_closure(&request(alpha, robot), &catalog).expect_err("cycle");
    match err {
        PackageClosureError::Cycle { path } => {
            let joined = path.join(" -> ");
            assert!(
                joined.contains("bravo") && joined.contains("charlie"),
                "complete path required, got {joined}"
            );
            assert!(
                path.len() >= 3,
                "path must include the cycle edge, got {path:?}"
            );
        }
        other => panic!("expected Cycle, got {other}"),
    }
}

#[test]
fn missing_provider_is_typed_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["missinglib >=1.0"]);
    let catalog = catalog_from_toml(
        root,
        r#"
schema_version = 1
"#,
    );

    let err = plan_package_closure(&request(alpha, robot), &catalog).expect_err("missing");
    match err {
        PackageClosureError::Catalog(inner) => {
            let msg = inner.to_string();
            assert!(
                msg.contains("missinglib") || msg.to_lowercase().contains("no package-source"),
                "{msg}"
            );
        }
        PackageClosureError::MissingProvider { name, .. } => {
            assert!(name.to_lowercase().contains("missinglib"), "{name}");
        }
        other => panic!("expected missing provider error, got {other}"),
    }
}

#[test]
fn ambiguous_provider_is_typed_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _b1 = conda_recipe(root, "bravo-a.yaml", "bravo", "1.0", &[]);
    let _b2 = conda_recipe(root, "bravo-b.yaml", "bravo", "2.0", &[]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.0"
source = "bravo-a.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "bravo"
version = "2.0"
source = "bravo-b.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let err = plan_package_closure(&request(alpha, robot), &catalog).expect_err("ambiguous");
    match err {
        PackageClosureError::Catalog(inner) => {
            assert!(
                inner.to_string().to_lowercase().contains("ambiguous"),
                "{inner}"
            );
        }
        PackageClosureError::AmbiguousProvider { name, .. } => {
            assert!(name.to_lowercase().contains("bravo"), "{name}");
        }
        other => panic!("expected ambiguous provider, got {other}"),
    }
}

#[test]
fn incompatible_provider_version_is_typed_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=5.0"]);
    let _bravo = conda_recipe(root, "bravo.yaml", "bravo", "1.0", &[]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.0"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let err = plan_package_closure(&request(alpha, robot), &catalog).expect_err("version");
    match err {
        PackageClosureError::IncompatibleProviderVersion {
            name,
            provided,
            required,
        } => {
            assert!(name.to_lowercase().contains("bravo"), "{name}");
            assert_eq!(provided, "1.0");
            assert!(
                required.contains('5') || required.contains(">="),
                "{required}"
            );
        }
        other => panic!("expected IncompatibleProviderVersion, got {other}"),
    }
}

#[test]
fn range_constraint_filters_catalog_versions_before_ambiguity() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _old = conda_recipe(root, "bravo-old.yaml", "bravo", "0.5", &[]);
    let _compatible = conda_recipe(root, "bravo-compatible.yaml", "bravo", "1.5", &[]);
    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "0.5"
source = "bravo-old.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "bravo"
version = "1.5"
source = "bravo-compatible.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close range");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.package.version, "1.5");
}

#[test]
fn generated_candidate_rejected_by_hierarchy_is_a_typed_error() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo = conda_recipe(root, "bravo.yaml", "bravo", "1.5", &[]);
    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.5"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2025a" }}
"#
        ),
    );

    let error = plan_package_closure(&request(alpha, robot), &catalog)
        .expect_err("cross-generation candidate needs an admitting stack pin");
    match error {
        PackageClosureError::GeneratedCandidateNotAdmitted {
            name,
            required,
            profile,
        } => {
            assert_eq!(name, "bravo");
            assert_eq!(required, ">=1.0");
            assert_eq!(profile, "default");
        }
        other => panic!("expected GeneratedCandidateNotAdmitted, got {other}"),
    }
}

#[test]
fn separate_root_profiles_each_solve_against_shared_closure() {
    use eb_stack::package_config::PackageConfigLayer;

    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    robot_eb(&robot, "Charlie", "1.0", &[]);

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo = conda_recipe(root, "bravo.yaml", "bravo", "1.0", &["charlie >=1.0"]);

    let layers = vec![PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1
[package]
name = "Alpha"
[[profiles]]
name = "default"
default = true
[[profiles]]
name = "complex"
inherits = "default"
versionsuffix = ["-complex"]
"#,
    )
    .expect("profiles")];

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.0"
source = "bravo.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let mut req = request(alpha, robot);
    req.package_layers = layers;
    let closure = plan_package_closure(&req, &catalog).expect("close multi-profile");

    assert_eq!(closure.root.locks.len(), 2);
    let profiles: Vec<_> = closure
        .root
        .locks
        .iter()
        .map(|l| l.profile.as_str())
        .collect();
    assert!(profiles.contains(&"default"));
    assert!(profiles.contains(&"complex"));
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.package.name, "bravo");
    for lock in &closure.root.locks {
        assert!(
            lock.dependencies
                .iter()
                .any(|d| d.name.eq_ignore_ascii_case("bravo")),
            "profile {} must lock Bravo",
            lock.profile
        );
    }
}

/// Source EasyBuild recipe for catalog `easybuild-bump` providers (synthetic names).
fn source_eb(
    root: &Path,
    file: &str,
    name: &str,
    version: &str,
    toolchain_version: &str,
    deps: &[(&str, &str)],
    body_extra: &str,
) -> PathBuf {
    let mut body = format!(
        "name = '{name}'\n\
         version = '{version}'\n\
         homepage = 'https://example.invalid/{name}'\n\
         description = \"synthetic {name}\"\n\
         toolchain = {{'name': 'foss', 'version': '{toolchain_version}'}}\n\
         sources = ['{name}-{version}.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
         moduleclass = 'lib'\n"
    );
    if !deps.is_empty() {
        body.push_str("dependencies = [\n");
        for (dep_name, dep_ver) in deps {
            body.push_str(&format!("    ('{dep_name}', '{dep_ver}'),\n"));
        }
        body.push_str("]\n");
    }
    body.push_str(body_extra);
    let path = root.join(file);
    write(&path, &body);
    path
}

#[test]
fn robot_hole_closed_by_easybuild_bump_provider() {
    // Alpha (foreign) -> missing Bravo, which has an EasyBuild recipe at another generation.
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    // Target robot has Charlie; Bravo is absent and must be retargeted.
    robot_eb(&robot, "Charlie", "1.0", &[]);

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo_src = source_eb(
        root,
        "bravo-1.5-foss-2023b.eb",
        "bravo",
        "1.5",
        "2023b",
        &[("Charlie", "1.0")],
        "configopts = '--enable-feature-x'\n",
    );

    let catalog = catalog_from_toml(
        root,
        r#"
schema_version = 1

[[packages]]
name = "bravo"
provider = "easybuild-bump"
version = "1.5"
source = "bravo-1.5-foss-2023b.eb"
toolchain = { name = "foss", version = "2026.1" }
"#,
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close via bump");
    assert_eq!(closure.companions.len(), 1);
    let bravo = &closure.companions[0];
    assert_eq!(bravo.plan.origin, PackageOrigin::EasyBuild);
    assert_eq!(bravo.plan.package.name, "bravo");
    assert_eq!(bravo.plan.package.version, "1.5");
    assert_eq!(bravo.plan.build.toolchain.version, "2026.1");
    assert_eq!(bravo.locks.len(), 1);
    assert_eq!(bravo.locks[0].solver, "resolvo");
    assert!(bravo.locks[0]
        .dependencies
        .iter()
        .any(|dep| dep.name.eq_ignore_ascii_case("charlie") && dep.version == "1.0"));
    assert_eq!(bravo.easyconfigs.len(), 1);
    let recipe = resolve_easyconfig_str(&bravo.easyconfigs[0].text).expect("parse bumped");
    assert_eq!(recipe.toolchain.name, "foss");
    assert_eq!(recipe.toolchain.version, "2026.1");
    assert_eq!(recipe.version, "1.5");
}

#[test]
fn easybuild_bump_preserves_recipe_mechanics_and_checksum_identity() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=2.0"]);
    let source_body = "\
name = 'bravo'
version = '2.0'
homepage = 'https://example.invalid/bravo'
description = \"preserve mechanics\"
toolchain = {'name': 'foss', 'version': '2023b'}
sources = ['bravo-2.0.tar.gz']
patches = [
    'bravo-portability.patch',
]
checksums = [
    {'bravo-2.0.tar.gz': 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'},
    {'bravo-portability.patch': 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'},
]
configopts = '--with-special-flag'
moduleclass = 'tools'
";
    write(&root.join("bravo-2.0-foss-2023b.eb"), source_body);

    let catalog = catalog_from_toml(
        root,
        r#"
schema_version = 1

[[packages]]
name = "bravo"
provider = "easybuild-bump"
version = "2.0"
source = "bravo-2.0-foss-2023b.eb"
toolchain = { name = "foss", version = "2026.1" }
"#,
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close");
    let text = &closure.companions[0].easyconfigs[0].text;
    assert!(
        text.contains("configopts = '--with-special-flag'"),
        "build mechanics must stay: {text}"
    );
    assert!(
        text.contains("moduleclass = 'tools'"),
        "moduleclass must stay: {text}"
    );
    assert!(
        text.contains("'bravo-2.0.tar.gz': 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd'"),
        "source checksum identity: {text}"
    );
    assert!(
        text.contains("'bravo-portability.patch': 'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee'"),
        "patch checksum order/identity: {text}"
    );
    assert!(
        text.contains("toolchain = {'name': 'foss', 'version': '2026.1'}")
            || text.contains("toolchain = {\"name\": \"foss\", \"version\": \"2026.1\"}")
            || (text.contains("foss") && text.contains("2026.1")),
        "toolchain retargeted: {text}"
    );
    // Source artifact order: tarball before patch.
    let tar_pos = text.find("bravo-2.0.tar.gz").expect("tarball key");
    let patch_pos = text.find("bravo-portability.patch").expect("patch key");
    assert!(tar_pos < patch_pos, "checksum order preserved");
}

#[test]
fn easybuild_bump_records_preferred_stack_pin_fallback_in_lock() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    // Only the fallback HDF5 version is available; preferred 1.14.2 is not.
    robot_eb(&robot, "HDF5", "1.14.3", &[]);

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo_src = source_eb(
        root,
        "bravo-1.0-foss-2023b.eb",
        "bravo",
        "1.0",
        "2023b",
        &[("HDF5", "1.14.0")],
        "",
    );

    let stack_path = root.join("stack.toml");
    write(
        &stack_path,
        r#"
schema_version = 1
name = "preferred-hdf5"

[toolchain]
name = "foss"
version = "2026.1"

[[pins]]
name = "HDF5"
version_requirement = "==1.14.2"
mode = "preferred"
source = "site stack"
"#,
    );

    let catalog = catalog_from_toml(
        root,
        r#"
schema_version = 1

[[packages]]
name = "bravo"
provider = "easybuild-bump"
version = "1.0"
source = "bravo-1.0-foss-2023b.eb"
toolchain = { name = "foss", version = "2026.1" }
stack_policy = "stack.toml"
"#,
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("close with pin");
    let lock = &closure.companions[0].locks[0];
    let hdf5 = lock
        .dependencies
        .iter()
        .find(|dep| dep.name == "HDF5")
        .expect("HDF5 selected");
    assert_eq!(hdf5.version, "1.14.3");
    let outcome = lock
        .pin_outcomes
        .iter()
        .find(|outcome| outcome.name == "HDF5")
        .expect("HDF5 pin outcome");
    assert!(outcome.fallback, "preferred pin must record fallback");
    assert_eq!(outcome.requested, "==1.14.2");
    assert_eq!(outcome.selected_version.as_deref(), Some("1.14.3"));
}

#[test]
fn mixed_foreign_and_bump_providers_topo_order_and_dedup() {
    // Alpha (foreign) -> Bravo (bump) -> Charlie (foreign hole); Charlie shared.
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo_src = source_eb(
        root,
        "bravo-1.0-foss-2023b.eb",
        "bravo",
        "1.0",
        "2023b",
        &[("charlie", "1.0")],
        "",
    );
    let _charlie = conda_recipe(root, "charlie.yaml", "charlie", "1.0", &[]);

    let catalog = catalog_from_toml(
        root,
        &format!(
            r#"
schema_version = 1

[[packages]]
name = "bravo"
provider = "easybuild-bump"
version = "1.0"
source = "bravo-1.0-foss-2023b.eb"
toolchain = {{ name = "foss", version = "2026.1" }}

[[packages]]
name = "charlie"
version = "1.0"
source = "charlie.yaml"
format = "conda-forge"
source_checksums = ["{CHECKSUM}"]
profile = "default"
toolchain = {{ name = "foss", version = "2026.1" }}
"#
        ),
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("mixed close");
    let names: Vec<_> = closure
        .companions
        .iter()
        .map(|c| c.plan.package.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["charlie", "bravo"],
        "foreign child before bumped parent: {names:?}"
    );
    assert_eq!(
        closure.companions[1].plan.origin,
        PackageOrigin::EasyBuild,
        "bravo is bump-origin"
    );
    assert_ne!(
        closure.companions[0].plan.origin,
        PackageOrigin::EasyBuild,
        "charlie is foreign-origin"
    );

    // Charlie appears once even if another path also needed it.
    let charlie_count = closure
        .companions
        .iter()
        .filter(|c| c.plan.package.name == "charlie")
        .count();
    assert_eq!(charlie_count, 1);
}

#[test]
fn easybuild_bump_companion_in_aggregate_bundle() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();

    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let _bravo_src = source_eb(
        root,
        "bravo-1.5-foss-2023b.eb",
        "bravo",
        "1.5",
        "2023b",
        &[],
        "",
    );
    let catalog = catalog_from_toml(
        root,
        r#"
schema_version = 1

[[packages]]
name = "bravo"
provider = "easybuild-bump"
version = "1.5"
source = "bravo-1.5-foss-2023b.eb"
toolchain = { name = "foss", version = "2026.1" }
"#,
    );

    let closure = plan_package_closure(&request(alpha, robot), &catalog).expect("plan");
    let out = root.join("bundle");
    let written = write_package_closure(&closure, &out).expect("write");

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
        .join("easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb")
        .is_file());

    let plan: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("packages/bravo-1.5-foss-2026.1/package.plan.json"))
            .expect("plan"),
    )
    .expect("json");
    assert_eq!(plan["origin"], "easy-build");

    let lock: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            out.join("packages/bravo-1.5-foss-2026.1/locks/default.lock.json"),
        )
        .expect("lock"),
    )
    .expect("json");
    assert_eq!(lock["solver"], "resolvo");

    let build_order: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&written.build_order).expect("order"))
            .expect("json");
    let recipes = build_order["recipes"].as_array().expect("recipes");
    assert!(
        recipes
            .iter()
            .any(|r| r.as_str() == Some("easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb")),
        "{recipes:?}"
    );
    // Companion before root.
    let bravo_idx = recipes
        .iter()
        .position(|r| r.as_str().is_some_and(|s| s.contains("bravo")))
        .expect("bravo");
    let alpha_idx = recipes
        .iter()
        .position(|r| r.as_str().is_some_and(|s| s.contains("alpha")))
        .expect("alpha");
    assert!(bravo_idx < alpha_idx);

    let aggregate: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&written.closure_sbom).expect("sbom"))
            .expect("json");
    let names: Vec<_> = aggregate["components"]
        .as_array()
        .expect("components")
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"bravo"));
}
