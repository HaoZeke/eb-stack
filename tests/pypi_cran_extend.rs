//! PyPI and CRAN ingest, extension provides, and language-bundle emission.

use eb_stack::package::{PackageOrigin, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_catalog::resolve_package_catalog_layers;
use eb_stack::package_closure::plan_package_closure_with_sources;
use eb_stack::package_sources::{PackageSourceRoots, SourceRootKind};
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
    assert_eq!(
        detect_foreign_format(Path::new("Cargo.toml")),
        Some(ForeignFormat::Cargo)
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
fn plan_cran_emits_a_single_r_package() {
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
    // Pins the current shape rather than the intended one: CRAN emits a single
    // RPackage, not the Bundle with exts_defaultclass = 'RPackage' the R side
    // of the bundle story needs. Flip this assertion when that lands.
    assert!(
        !recipe.text.contains("exts_list"),
        "CRAN does not emit an exts_list yet:\n{}",
        recipe.text
    );
}

fn numpy_robot() -> PathBuf {
    root().join("fixtures/foreign_ingest/pypi_numpy/robot")
}

#[test]
fn plan_numpy_is_already_provided_by_scipy_bundle() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/numpy.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![numpy_robot()],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan numpy");
    assert!(
        bundle.easyconfigs.is_empty(),
        "numpy must not emit a pip PythonBundle: {:?}",
        bundle
            .easyconfigs
            .iter()
            .map(|config| &config.filename)
            .collect::<Vec<_>>()
    );
    assert!(bundle.locks.is_empty());
    assert!(
        bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "already-provided"
                && residual.summary.contains("SciPy-bundle")),
        "{:?}",
        bundle.plan.residuals
    );
}

#[test]
fn plan_leftover_depends_on_scipy_bundle_for_numpy() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/leftover.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![numpy_robot()],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan leftover");
    let recipe = bundle
        .easyconfigs
        .iter()
        .find(|config| config.filename.contains("leftover"))
        .expect("emitted leftover");
    assert!(
        recipe.text.contains("SciPy-bundle"),
        "numpy must resolve via SciPy-bundle:\n{}",
        recipe.text
    );
    assert!(
        !recipe.text.contains("('numpy'"),
        "numpy must not be re-emitted as an extension:\n{}",
        recipe.text
    );
    assert!(
        bundle.locks[0]
            .dependencies
            .iter()
            .any(|dep| dep.name == "SciPy-bundle"),
        "{:?}",
        bundle.locks[0].dependencies
    );
}

#[test]
fn plan_torch_without_pytorch_module_refuses_pip_overlay() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/torch.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![numpy_robot()],
        stack_policy: stack_policy(),
    };
    let error = plan_new_package(&request).expect_err("torch must refuse pip overlay");
    let message = error.to_string();
    assert!(
        message.contains("torch") && message.contains("PythonBundle"),
        "{message}"
    );
}

#[test]
fn plan_cargo_readcon_uses_rust_and_maturin() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/cargo_readcon/crates.json"),
        format: Some(ForeignFormat::Cargo),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cargo_readcon/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan cargo readcon");
    assert_eq!(bundle.plan.origin, eb_stack::package::PackageOrigin::Cargo);
    let recipe = bundle
        .easyconfigs
        .iter()
        .find(|config| config.filename.to_ascii_lowercase().contains("readcon"))
        .expect("emitted readcon");
    assert!(
        recipe.text.contains("easyblock = 'PythonPackage'")
            || recipe.text.contains("('Rust'")
            || recipe.text.contains("'Rust'"),
        "{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("('Rust'") || recipe.text.contains("'Rust'"),
        "Rust must be a build dependency:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("binutils"),
        "binutils must be a build dependency so cargo can find ld:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("maturin"),
        "maturin must be a build dependency:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("unset RUSTC_WRAPPER")
            && recipe
                .text
                .contains("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER")
            && recipe.text.contains("compat/linux"),
        "cargo leftovers must drop host rustc wrappers and pin the gcc linker:\n{}",
        recipe.text
    );
    assert!(
        bundle.locks[0]
            .dependencies
            .iter()
            .any(|dep| dep.name == "Rust"),
        "{:?}",
        bundle.locks[0].dependencies
    );
}

#[test]
fn plan_eon_akmc_overlays_missing_pypi_deps() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/eon-akmc.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![numpy_robot()],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan eon-akmc");
    let recipe = bundle
        .easyconfigs
        .iter()
        .find(|config| config.filename.contains("eon-akmc"))
        .expect("emitted leftover");
    assert!(
        recipe.text.contains("SciPy-bundle"),
        "numpy must resolve via SciPy-bundle:\n{}",
        recipe.text
    );
    assert!(
        !recipe.text.contains("('numpy'"),
        "numpy must not be re-emitted:\n{}",
        recipe.text
    );
    for ext in ["readcon", "vesin", "eon-schema", "xxhash", "eon-akmc"] {
        assert!(
            recipe.text.contains(&format!("('{ext}'")),
            "expected {ext} in exts_list:\n{}",
            recipe.text
        );
    }
    assert!(
        recipe.text.contains("exts_default_options")
            && recipe.text.contains(
                "PYTHONPATH=%(installdir)s/lib/python%(pyshortver)s/site-packages${PYTHONPATH:+:$PYTHONPATH}"
            )
            && !recipe.text.contains(r"\$PYTHONPATH"),
        "overlay extras must prepend the prefix to PYTHONPATH so pip sees prior exts and EESSI modules:\n{}",
        recipe.text
    );
    assert!(
        bundle.locks[0]
            .dependencies
            .iter()
            .any(|dep| dep.name == "SciPy-bundle"),
        "{:?}",
        bundle.locks[0].dependencies
    );
}

#[test]
fn plan_torch_uses_existing_pytorch_module() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/torch.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/pypi_numpy/robot-pytorch")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan torch");
    assert!(bundle.easyconfigs.is_empty(), "{:?}", bundle.easyconfigs);
    assert!(
        bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "already-provided"
                && residual.summary.contains("PyTorch")),
        "{:?}",
        bundle.plan.residuals
    );
}

#[test]
fn plan_eon_akmc_resolvo_takes_robot_yaml_quill_cbindgen() {
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(
        SourceRootKind::Cargo,
        root().join("fixtures/foreign_ingest/cargo_readcon"),
    );
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/eon-akmc-resolvo.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![
            numpy_robot(),
            root().join("fixtures/foreign_ingest/cargo_readcon/robot"),
        ],
        stack_policy: stack_policy(),
    };
    let catalog = resolve_package_catalog_layers(&[]).expect("empty catalog");
    let closure = plan_package_closure_with_sources(&request, &catalog, &sources)
        .expect("resolvo must close cargo + robot provides");
    let recipe = closure
        .root
        .easyconfigs
        .iter()
        .find(|config| config.filename.contains("eon-akmc"))
        .expect("eon-akmc root");
    assert!(
        recipe.text.contains("PyYAML"),
        "PyYAML must resolve as a robot module:\n{}",
        recipe.text
    );
    let extras_block = recipe
        .text
        .split("exts_list")
        .nth(1)
        .and_then(|text| text.split("builddependencies").next())
        .unwrap_or("");
    assert!(
        !extras_block.contains("('PyYAML'"),
        "PyYAML must not sit in exts_list:\n{}",
        recipe.text
    );
    assert!(
        !extras_block.contains("('hatchling'")
            && !extras_block.contains("('packaging'")
            && !extras_block.contains("('meson-python'"),
        "robot-provided build backends stay modules, not pip extras:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("quill")
            && recipe.text.contains("cbindgen")
            && recipe.text.contains("Eigen"),
        "meson leftovers take wrap natives from ingest + robot:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("wrap_mode=default"),
        "mesonpy backend must emit wrap_mode=default, not a hand-edited recipe:\n{}",
        recipe.text
    );
    assert!(
        recipe
            .text
            .contains("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"),
        "meson wraps need the EESSI gcc cargo linker:\n{}",
        recipe.text
    );
    assert!(
        closure
            .root
            .locks
            .iter()
            .flat_map(|lock| lock.dependencies.iter())
            .any(|dep| dep.name == "PyYAML"),
        "Resolvo must lock PyYAML: {:?}",
        closure.root.locks
    );
    assert!(
        closure
            .root
            .locks
            .iter()
            .flat_map(|lock| lock.dependencies.iter())
            .any(|dep| dep.name == "quill")
            && closure
                .root
                .locks
                .iter()
                .flat_map(|lock| lock.dependencies.iter())
                .any(|dep| dep.name == "cbindgen")
            && closure
                .root
                .locks
                .iter()
                .flat_map(|lock| lock.dependencies.iter())
                .any(|dep| dep.name == "Eigen"),
        "Resolvo must lock wrap natives from ingest: {:?}",
        closure.root.locks
    );
}

#[test]
fn plan_eon_akmc_closes_readcon_from_cargo_source() {
    let mut sources = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    sources.push(
        SourceRootKind::Cargo,
        root().join("fixtures/foreign_ingest/cargo_readcon"),
    );
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/eon-akmc.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        easyconfig_roots: vec![
            numpy_robot(),
            root().join("fixtures/foreign_ingest/cargo_readcon/robot"),
        ],
        stack_policy: stack_policy(),
    };
    let catalog = resolve_package_catalog_layers(&[]).expect("empty catalog");
    let closure = plan_package_closure_with_sources(&request, &catalog, &sources)
        .expect("cargo leftover becomes a companion");
    assert_eq!(closure.companions.len(), 1, "{:?}", closure.companions);
    let companion = &closure.companions[0];
    assert_eq!(companion.plan.origin, PackageOrigin::Cargo);
    assert_eq!(companion.plan.package.name, "readcon");
    assert_eq!(companion.plan.package.version, "0.13.1");
    let readcon = companion
        .easyconfigs
        .iter()
        .find(|config| config.filename.to_ascii_lowercase().contains("readcon"))
        .expect("readcon companion recipe");
    assert!(
        readcon.text.contains("maturin") && readcon.text.contains("unset RUSTC_WRAPPER"),
        "{}",
        readcon.text
    );
    let recipe = closure
        .root
        .easyconfigs
        .iter()
        .find(|config| config.filename.contains("eon-akmc"))
        .expect("eon-akmc root");
    assert!(
        recipe.text.contains("('readcon'") || recipe.text.contains("'readcon'"),
        "root must depend on the cargo companion:\n{}",
        recipe.text
    );
    assert!(
        !recipe.text.contains("('readcon', '0.13.0')"),
        "readcon must not remain a pip overlay extra:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("('vesin'")
            && recipe.text.contains("('eon-schema'")
            && recipe.text.contains("('xxhash'"),
        "pure leftover deps stay overlay extras:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("SciPy-bundle"),
        "numpy still maps to SciPy-bundle:\n{}",
        recipe.text
    );
    assert!(
        !recipe.text.contains("quill")
            && !recipe.text.contains("cbindgen")
            && !recipe.text.contains("Eigen"),
        "non-meson leftovers must not take meson wrap natives:\n{}",
        recipe.text
    );
}

/// A CRAN package whose imports the robot does not carry has to become a
/// Bundle: the leftovers are already in the plan, and the single-RPackage
/// rendering has nowhere to put them, so they used to be dropped in silence.
#[test]
fn plan_cran_with_leftovers_emits_an_r_bundle() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/cran_bundle/cran.json"),
        format: Some(ForeignFormat::Cran),
        toolchain: toolchain(),
        source_checksums: vec![
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".into(),
        ],
        package_layers: Vec::new(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cran_bundle/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan cran bundle");
    let recipe = &bundle.easyconfigs[0];
    assert!(
        recipe.text.contains("easyblock = 'Bundle'"),
        "{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("exts_defaultclass = 'RPackage'"),
        "{}",
        recipe.text
    );
    for extension in ["processx", "rmarkdown"] {
        assert!(
            recipe.text.contains(extension),
            "expected {extension} in exts_list:\n{}",
            recipe.text
        );
    }
    assert!(recipe.text.contains("'R'"), "{}", recipe.text);
}
