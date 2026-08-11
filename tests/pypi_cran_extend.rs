//! PyPI and CRAN ingest, extension provides, and language-bundle emission.

use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::{
    detect_foreign_format, inspect_new_package, parse_foreign_path, plan_new_package,
    ForeignFormat, NewPackageRequest, Toolchain,
};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn stack_policy() -> StackPolicy {
    StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "test".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    }
}

#[test]
fn detect_pypi_and_cran_paths() {
    assert_eq!(
        detect_foreign_format(Path::new("requirements.txt")),
        Some(ForeignFormat::Pypi)
    );
    assert_eq!(
        detect_foreign_format(Path::new("beautifulsoup4.pypi.json")),
        Some(ForeignFormat::Pypi)
    );
    assert_eq!(
        detect_foreign_format(Path::new("DESCRIPTION")),
        Some(ForeignFormat::Cran)
    );
    assert_eq!(
        detect_foreign_format(Path::new("jsonlite.cran.txt")),
        Some(ForeignFormat::Cran)
    );
}

#[test]
fn inspect_pypi_warehouse_fixture() {
    let path = root().join("fixtures/foreign_ingest/pypi_bs4/pypi.json");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::Pypi)).expect("parse");
    assert_eq!(recipe.name, "beautifulsoup4");
    assert_eq!(recipe.dependencies[0].name, "Python");
    assert_eq!(recipe.dependencies[1].name, "soupsieve");
    let (plan, _) =
        inspect_new_package(&path, Some(ForeignFormat::Pypi), &toolchain(), &[]).expect("inspect");
    assert_eq!(plan.origin, eb_stack::package::PackageOrigin::Pypi);
    assert_eq!(plan.build.easyblock.as_deref(), Some("PythonBundle"));
}

#[test]
fn plan_pypi_uses_python_bundle_and_soupsieve_provide() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_bs4/pypi.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/pypi_bs4/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan pypi");
    let recipe = bundle
        .easyconfigs
        .iter()
        .find(|config| config.filename.contains("beautifulsoup4"))
        .expect("emitted recipe");
    assert!(
        recipe.text.contains("easyblock = 'PythonBundle'"),
        "{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("exts_list"),
        "expected exts_list in {}",
        recipe.text
    );
    assert!(
        recipe.text.contains("('Python'") || recipe.text.contains("('Python',"),
        "PythonBundle must depend on Python:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("Python-bundle-PyPI"),
        "soupsieve must resolve via Python-bundle-PyPI:\n{}",
        recipe.text
    );
    assert!(
        !recipe.text.contains("('soupsieve'"),
        "soupsieve must not be re-emitted as an extension:\n{}",
        recipe.text
    );
    let lock = &bundle.locks[0];
    assert!(
        lock.dependencies.iter().any(|dep| dep.name == "Python"),
        "Python must be locked even when its easyconfig names binutils: {:?}",
        lock.dependencies
    );
    assert!(
        lock.dependencies
            .iter()
            .any(|dep| dep.name == "Python-bundle-PyPI"),
        "{:?}",
        lock.dependencies
    );
}

#[test]
fn inspect_cran_description_fixture() {
    let path = root().join("fixtures/foreign_ingest/cran_jsonlite/DESCRIPTION");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::Cran)).expect("parse");
    assert_eq!(recipe.name, "jsonlite");
    assert!(recipe.dependencies.iter().any(|dep| dep.name == "R"));
    let (plan, _) =
        inspect_new_package(&path, Some(ForeignFormat::Cran), &toolchain(), &[]).expect("inspect");
    assert_eq!(plan.origin, eb_stack::package::PackageOrigin::Cran);
    assert_eq!(plan.build.easyblock.as_deref(), Some("RPackage"));
}

#[test]
fn plan_cran_emits_r_bundle() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/cran_jsonlite/cran.json"),
        format: Some(ForeignFormat::Cran),
        toolchain: toolchain(),
        source_checksums: vec![
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        ],
        package_layers: Vec::new(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cran_jsonlite/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan cran");
    let recipe = &bundle.easyconfigs[0];
    assert!(
        recipe.text.contains("easyblock = 'RPackage'"),
        "{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("sanity_check_paths"),
        "{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("('R', '4.4.2')") || recipe.text.contains("'R'"),
        "R must be a dependency:\n{}",
        recipe.text
    );
    assert!(
        bundle.locks[0]
            .dependencies
            .iter()
            .any(|dep| dep.name == "R"),
        "R must be locked even when its easyconfig names binutils: {:?}",
        bundle.locks[0].dependencies
    );
}
