//! In-memory recursive package-closure planner (catalog-backed robot holes).
//!
//! Synthetic package names only — no production package identities.

use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_catalog::{
    resolve_package_catalog_layers, PackageCatalogLayer, PackageSourceCatalog,
};
use eb_stack::package_closure::{plan_package_closure, PackageClosureError};
use eb_stack::{ForeignFormat, NewPackageRequest, Toolchain};
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
    let charlie_dep = closure.companions[0]
        .locks[0]
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
    assert!(echo_pos < bravo_pos && echo_pos < delta_pos, "echo before dependents");
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
            assert!(required.contains('5') || required.contains(">="), "{required}");
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
    let profiles: Vec<_> = closure.root.locks.iter().map(|l| l.profile.as_str()).collect();
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
