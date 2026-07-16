//! Source-root discovery closes robot holes without per-package catalog entries.
//! Synthetic package identities only.

use eb_stack::package::{PackageOrigin, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_catalog::{resolve_package_catalog_layers, PackageCatalogLayer};
use eb_stack::package_closure::{plan_package_closure_with_sources, PackageClosureError};
use eb_stack::package_sources::{PackageSourceRoots, SourceRootKind};
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
        name: "discovery-test".into(),
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

fn robot_eb(root: &Path, name: &str, version: &str) {
    write(
        &root.join(format!("{name}-{version}-foss-2026.1.eb")),
        &format!(
            "name = '{name}'\nversion = '{version}'\ntoolchain = {{'name': 'foss', 'version': '2026.1'}}\n"
        ),
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
  sha256: {CHECKSUM}
requirements:
  host:
{reqs}"#
        ),
    );
    path
}

fn empty_catalog() -> eb_stack::PackageSourceCatalog {
    resolve_package_catalog_layers(&[]).expect("empty catalog")
}

fn request(source: PathBuf, robot: PathBuf) -> NewPackageRequest {
    NewPackageRequest {
        source,
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    }
}

#[test]
fn robot_win_without_catalog_or_sources() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    std::fs::create_dir_all(&robot).unwrap();
    robot_eb(&robot, "Bravo", "1.0");
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    // No holes → empty sources still works via closure with empty index.
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::CondaForge, root.join("missing-index"));
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("robot supplies Bravo");
    assert!(closure.companions.is_empty());
    assert_eq!(
        closure.root.locks[0]
            .dependencies
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case("bravo"))
            .expect("bravo")
            .version,
        "1.0"
    );
}

#[test]
fn easybuild_cross_generation_bump_without_catalog() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let eb_sources = root.join("eb-sources");
    std::fs::create_dir_all(&robot).unwrap();
    std::fs::create_dir_all(&eb_sources).unwrap();
    robot_eb(&robot, "Charlie", "1.0");
    write(
        &eb_sources.join("bravo-1.5-foss-2023b.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         homepage = 'https://example.invalid/bravo'\n\
         description = 'synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023b'}\n\
         sources = ['bravo-1.5.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
         dependencies = [\n\
             ('Charlie', '1.0'),\n\
         ]\n\
         moduleclass = 'lib'\n",
    );
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::EasyBuild, eb_sources);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("bump discovery");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.origin, PackageOrigin::EasyBuild);
    assert_eq!(closure.companions[0].plan.package.version, "1.5");
    assert_eq!(closure.companions[0].plan.build.toolchain.version, "2026.1");
}

#[test]
fn resolvo_selects_one_compatible_easybuild_source_version() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let eb_sources = root.join("eb-sources");
    std::fs::create_dir_all(&robot).unwrap();
    for version in ["1.0", "1.5"] {
        write(
            &eb_sources.join(format!("bravo-{version}-foss-2023b.eb")),
            &format!(
                "name = 'bravo'\n\
                 version = '{version}'\n\
                 homepage = 'https://example.invalid/bravo'\n\
                 description = 'synthetic'\n\
                 toolchain = {{'name': 'foss', 'version': '2023b'}}\n\
                 sources = ['bravo-{version}.tar.gz']\n\
                 checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
                 moduleclass = 'lib'\n"
            ),
        );
    }
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::EasyBuild, eb_sources);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("Resolvo source selection");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.package.name, "bravo");
    assert_eq!(closure.companions[0].plan.package.version, "1.5");
}

#[test]
fn easybuild_gcccore_source_targets_hierarchy_member_not_foss() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let eb_sources = root.join("eb-sources");
    std::fs::create_dir_all(&robot).unwrap();
    std::fs::create_dir_all(&eb_sources).unwrap();
    write(
        &eb_sources.join("corelib-1.0-GCCcore-13.3.0.eb"),
        "name = 'CoreLib'\n\
         version = '1.0'\n\
         homepage = 'https://example.invalid/corelib'\n\
         description = 'synthetic core'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
         sources = ['CoreLib-1.0.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
         moduleclass = 'lib'\n",
    );
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["corelib >=1.0"]);
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::EasyBuild, eb_sources);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("gcccore map");
    assert_eq!(closure.companions.len(), 1);
    let companion = &closure.companions[0];
    assert_eq!(companion.plan.build.toolchain.name, "GCCcore");
    assert_eq!(companion.plan.build.toolchain.version, "15.2.0");
    assert_ne!(companion.plan.build.toolchain.name, "foss");
}

#[test]
fn easybuild_variants_require_an_unambiguous_source_identity() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let eb_sources = root.join("eb-sources");
    std::fs::create_dir_all(&robot).unwrap();
    write(
        &eb_sources.join("bravo-1.5-foss-2023b.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         toolchain = {'name': 'foss', 'version': '2023b'}\n",
    );
    write(
        &eb_sources.join("bravo-1.5-MPI-foss-2023b.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         versionsuffix = '-MPI'\n\
         toolchain = {'name': 'foss', 'version': '2023b'}\n",
    );
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::EasyBuild, eb_sources);
    let err = plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
        .expect_err("variant selection must be explicit");
    match err {
        PackageClosureError::AmbiguousSource {
            count, candidates, ..
        } => {
            assert_eq!(count, 2);
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected AmbiguousSource, got {other}"),
    }
}

#[test]
fn source_toolchain_families_outside_target_hierarchy_are_ignored() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let eb_sources = root.join("eb-sources");
    std::fs::create_dir_all(&robot).unwrap();
    write(
        &eb_sources.join("bravo-1.5-GCC-14.3.0.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         toolchain = {'name': 'GCC', 'version': '14.3.0'}\n",
    );
    write(
        &eb_sources.join("bravo-1.5-intel-compilers-2025.2.0.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         toolchain = {'name': 'intel-compilers', 'version': '2025.2.0'}\n",
    );
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::EasyBuild, eb_sources);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("foss target admits GCC source family");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.build.toolchain.name, "GCC");
    assert_eq!(closure.companions[0].plan.build.toolchain.version, "15.2.0");
}

#[test]
fn conda_fallback_without_catalog() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let conda = root.join("conda");
    std::fs::create_dir_all(&robot).unwrap();
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravolib >=1.0"]);
    write(
        &conda.join("bravolib").join("meta.yaml"),
        &format!(
            r#"package:
  name: bravolib
  version: "2.0"
source:
  url: https://example.invalid/bravolib-2.0.tar.gz
  sha256: {CHECKSUM}
"#
        ),
    );
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::CondaForge, conda);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("conda fallback");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.package.name, "bravolib");
    assert_eq!(closure.companions[0].plan.package.version, "2.0");
    assert_eq!(closure.companions[0].plan.origin, PackageOrigin::CondaForge);
}

#[test]
fn discovered_foreign_recipe_inherits_relative_root_package_config() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let conda = root.join("conda");
    std::fs::create_dir_all(&robot).unwrap();
    robot_eb(&robot, "Zeta", "3.0");
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    conda_recipe(
        &conda,
        "bravo/meta.yaml",
        "foreign-bravo",
        "1.5",
        &["foreign-zeta >=3.0"],
    );
    write(
        &root.join("common.toml"),
        r#"schema_version = 1
[dependencies.aliases]
"foreign-bravo" = "Bravo"
"foreign-zeta" = "Zeta"
"#,
    );
    write(
        &root.join("sources.toml"),
        r#"schema_version = 1
[[source_roots]]
kind = "conda-forge"
path = "conda"
package_config = ["common.toml"]
"#,
    );
    let sources = PackageSourceRoots::from_path(&root.join("sources.toml")).expect("sources");
    assert_eq!(
        sources.source_roots[0].package_config,
        vec![root.join("common.toml")]
    );

    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("shared alias closes companion");
    let bravo = closure
        .companions
        .iter()
        .find(|companion| companion.plan.package.name == "Bravo")
        .expect("bravo companion");
    assert_eq!(bravo.plan.package.name, "Bravo");
    let dependency = bravo
        .plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "foreign-zeta")
        .expect("foreign dependency");
    assert_eq!(dependency.eb_name.as_deref(), Some("Zeta"));
}

#[test]
fn spack_fallback_without_catalog() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let spack = root.join("spack");
    std::fs::create_dir_all(&robot).unwrap();
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["spackleaf >=1.0"]);
    write(
        &spack.join("packages").join("spackleaf").join("package.py"),
        r#"
from spack.package import *
class Spackleaf(Package):
    version("1.1", sha256="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
"#,
    );
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::Spack, spack);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("spack fallback");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.origin, PackageOrigin::Spack);
    assert_eq!(closure.companions[0].plan.package.version, "1.1");
}

#[test]
fn ambiguous_conda_and_spack_is_typed_error_with_candidates() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let conda = root.join("conda");
    let spack = root.join("spack");
    std::fs::create_dir_all(&robot).unwrap();
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["dupe >=1.0"]);
    write(
        &conda.join("dupe").join("meta.yaml"),
        &format!(
            r#"package:
  name: dupe
  version: "1.0"
source:
  url: https://example.invalid/dupe-1.0.tar.gz
  sha256: {CHECKSUM}
"#
        ),
    );
    write(
        &spack.join("packages").join("dupe").join("package.py"),
        r#"
from spack.package import *
class Dupe(Package):
    version("1.0", sha256="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
"#,
    );
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::CondaForge, conda);
    sources.push(SourceRootKind::Spack, spack);
    let err = plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
        .expect_err("ambiguous");
    match err {
        PackageClosureError::AmbiguousSource {
            count, candidates, ..
        } => {
            assert_eq!(count, 2);
            assert_eq!(candidates.len(), 2);
        }
        other => panic!("expected AmbiguousSource, got {other}"),
    }
}

#[test]
fn recursion_across_mixed_providers() {
    // Alpha -> missing Bravo (conda) -> missing Charlie (easybuild bump) -> robot Delta
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let conda = root.join("conda");
    let eb = root.join("eb");
    std::fs::create_dir_all(&robot).unwrap();
    robot_eb(&robot, "Delta", "1.0");
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    write(
        &conda.join("bravo").join("meta.yaml"),
        &format!(
            r#"package:
  name: bravo
  version: "1.0"
source:
  url: https://example.invalid/bravo-1.0.tar.gz
  sha256: {CHECKSUM}
requirements:
  host:
    - charlie >=1.0
"#
        ),
    );
    write(
        &eb.join("charlie-1.0-foss-2023b.eb"),
        "name = 'charlie'\n\
         version = '1.0'\n\
         homepage = 'https://example.invalid/charlie'\n\
         description = 'synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023b'}\n\
         sources = ['charlie-1.0.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
         dependencies = [\n\
             ('Delta', '1.0'),\n\
         ]\n\
         moduleclass = 'lib'\n",
    );
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::CondaForge, conda);
    sources.push(SourceRootKind::EasyBuild, eb);
    let closure =
        plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
            .expect("mixed");
    let names: Vec<_> = closure
        .companions
        .iter()
        .map(|c| c.plan.package.name.as_str())
        .collect();
    assert!(
        names.contains(&"charlie") && names.contains(&"bravo"),
        "expected bravo and charlie companions, got {names:?}"
    );
    let charlie = closure
        .companions
        .iter()
        .find(|c| c.plan.package.name == "charlie")
        .expect("charlie");
    assert_eq!(charlie.plan.origin, PackageOrigin::EasyBuild);
    let bravo = closure
        .companions
        .iter()
        .find(|c| c.plan.package.name == "bravo")
        .expect("bravo");
    assert_eq!(bravo.plan.origin, PackageOrigin::CondaForge);
}

#[test]
fn catalog_override_wins_over_source_discovery() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let conda = root.join("conda");
    std::fs::create_dir_all(&robot).unwrap();
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    write(
        &conda.join("bravo").join("meta.yaml"),
        &format!(
            r#"package:
  name: bravo
  version: "9.9"
source:
  url: https://example.invalid/bravo-9.9.tar.gz
  sha256: {CHECKSUM}
"#
        ),
    );
    let _catalog_recipe = conda_recipe(root, "bravo-catalog.yaml", "bravo", "1.5", &[]);
    write(
        &root.join("catalog.toml"),
        r#"
schema_version = 1
[[packages]]
name = "bravo"
version = "1.5"
source = "bravo-catalog.yaml"
format = "conda-forge"
toolchain = { name = "foss", version = "2026.1" }
"#,
    );
    let layer = PackageCatalogLayer::from_path(&root.join("catalog.toml")).expect("catalog");
    let catalog = resolve_package_catalog_layers(&[layer]).expect("resolve");
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::CondaForge, conda);
    let closure = plan_package_closure_with_sources(&request(alpha, robot), &catalog, &sources)
        .expect("catalog override");
    assert_eq!(closure.companions.len(), 1);
    assert_eq!(closure.companions[0].plan.package.version, "1.5");
}

#[test]
fn cycle_still_reported_with_source_discovery() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let robot = root.join("robot");
    let conda = root.join("conda");
    std::fs::create_dir_all(&robot).unwrap();
    let alpha = conda_recipe(root, "alpha.yaml", "alpha", "1.0", &["bravo >=1.0"]);
    write(
        &conda.join("bravo").join("meta.yaml"),
        &format!(
            r#"package:
  name: bravo
  version: "1.0"
source:
  url: https://example.invalid/bravo-1.0.tar.gz
  sha256: {CHECKSUM}
requirements:
  host:
    - charlie >=1.0
"#
        ),
    );
    write(
        &conda.join("charlie").join("meta.yaml"),
        &format!(
            r#"package:
  name: charlie
  version: "1.0"
source:
  url: https://example.invalid/charlie-1.0.tar.gz
  sha256: {CHECKSUM}
requirements:
  host:
    - bravo >=1.0
"#
        ),
    );
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(SourceRootKind::CondaForge, conda);
    let err = plan_package_closure_with_sources(&request(alpha, robot), &empty_catalog(), &sources)
        .expect_err("cycle");
    match err {
        PackageClosureError::Cycle { path } => {
            let joined = path.join(" -> ");
            assert!(
                joined.contains("bravo") && joined.contains("charlie"),
                "{joined}"
            );
        }
        other => panic!("expected Cycle, got {other}"),
    }
}
