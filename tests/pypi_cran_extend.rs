//! PyPI and CRAN ingest, extension provides, and language-bundle emission.

use eb_stack::package::{PackageOrigin, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_catalog::resolve_package_catalog_layers;
use eb_stack::package_closure::plan_package_closure_with_sources;
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::package_sources::{PackageSourceRoots, SourceRootKind};
use eb_stack::{
    complete_package_bundle, detect_foreign_format, inspect_new_package, materialize_pypi,
    overlay_package_identity, parse_easyconfig_trees, parse_foreign_path, plan_new_package,
    prepare_new_package_plan, ForeignFormat, MapClient, NewPackageRequest, Toolchain,
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
    assert_eq!(
        detect_foreign_format(Path::new("lfs.rockspec")),
        Some(ForeignFormat::Luarocks)
    );
    assert_eq!(
        detect_foreign_format(Path::new("META6.json")),
        Some(ForeignFormat::Raku)
    );
}

#[test]
fn name_mode_fetch_writes_a_dump_replay_parses() {
    let mut client = MapClient::default();
    client.pages.insert(
        "https://pypi.org/pypi/demo/json".into(),
        br#"{
          "info": {
            "name": "demo",
            "version": "1.0.0",
            "requires_dist": ["soupsieve>=1.6"]
          },
          "urls": []
        }"#
        .to_vec(),
    );
    let tmp = tempfile::tempdir().expect("temp");
    let ingest = materialize_pypi("demo", &client, "https://pypi.org", tmp.path()).expect("fetch");
    let recipe = parse_foreign_path(&ingest.dump, Some(ForeignFormat::Pypi)).expect("replay");
    assert_eq!(recipe.name, "demo");
    assert!(recipe
        .dependencies
        .iter()
        .any(|dep| dep.name == "soupsieve"));
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
fn inspect_source_tree_pep518_marker_and_extra() {
    use eb_stack::package::{ConditionExpr, DependencyRole};

    let tmp = tempfile::tempdir().expect("temp");
    let dump = tmp.path().join("demo-1.0.0.json");
    std::fs::write(
        &dump,
        r#"{
          "info": {
            "name": "demo",
            "version": "1.0.0",
            "requires_dist": []
          },
          "urls": []
        }"#,
    )
    .expect("dump");
    let tree = tmp.path().join("demo-1.0.0");
    std::fs::create_dir_all(&tree).expect("dirs");
    std::fs::write(
        tree.join("pyproject.toml"),
        "[build-system]\nrequires = [\"tomli>=2.0.1; python_version < 3.11\", \"pytest; extra == test\"]\n",
    )
    .expect("pyproject");
    let (plan, _) =
        inspect_new_package(&dump, Some(ForeignFormat::Pypi), &toolchain(), &[]).expect("inspect");
    let tomli = plan
        .dependencies
        .iter()
        .find(|dep| dep.name == "tomli")
        .expect("tomli");
    assert!(
        tomli
            .roles
            .iter()
            .any(|role| *role == DependencyRole::Build),
        "{tomli:?}"
    );
    assert!(
        !matches!(tomli.condition, ConditionExpr::Always),
        "tomli must not be Always: {tomli:?}"
    );
    assert!(
        tomli.solver_excluded,
        "opaque PEP 518 marker must be solver-excluded: {tomli:?}"
    );
    assert!(
        plan.dependencies.iter().all(|dep| dep.name != "pytest"),
        "pytest must not be a build dep: {:?}",
        plan.dependencies
    );
}

#[test]
fn plan_pypi_emits_one_package_and_takes_soupsieve_from_the_robot() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_bs4/pypi.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        package_index: Default::default(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/pypi_bs4/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan pypi");
    let recipe = bundle
        .easyconfigs
        .iter()
        .find(|config| config.filename.contains("beautifulsoup4"))
        .expect("emitted recipe");
    // One PyPI package installed on its own is a PythonPackage upstream: of
    // 400 sampled PythonBundle recipes in the tree, 3 carry a single
    // extension. A bundle is for a set installed together.
    assert!(
        recipe.text.contains("easyblock = 'PythonPackage'"),
        "{}",
        recipe.text
    );
    assert!(
        !recipe.text.contains("exts_list"),
        "one package needs no exts_list:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("checksums = ["),
        "the package's own source must be carried:\n{}",
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
        package_index: Default::default(),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cran_jsonlite/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan cran");
    let recipe = &bundle.easyconfigs[0];
    assert!(
        recipe.text.contains("easyblock = 'RPackage'")
            && !recipe.text.contains("exts_list")
            && !recipe.text.contains("exts_defaultclass"),
        "{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("sanity_check_paths")
            && recipe.text.contains("lib/R/library/jsonlite"),
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

fn numpy_robot() -> PathBuf {
    root().join("fixtures/foreign_ingest/pypi_numpy/robot")
}

fn assert_eessi_cargo_isolation(text: &str) {
    assert!(
        text.contains("unset RUSTC_WRAPPER")
            && text.contains("uname -m")
            && text.contains("compat/linux/${_arch}/usr/bin")
            && text.contains("link-arg=-B")
            && text.contains("LINKER=${CC:-gcc}")
            && !text.contains("X86_64"),
        "cargo on EESSI must share one host-derived isolation prelude:\n{text}"
    );
}

#[test]
fn plan_numpy_is_already_provided_by_scipy_bundle() {
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/pypi_numpy/numpy.json"),
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        package_index: Default::default(),
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
        package_index: Default::default(),
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
        package_index: Default::default(),
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
        package_index: Default::default(),
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
    assert_eessi_cargo_isolation(&recipe.text);
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
        package_index: Default::default(),
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
        package_index: Default::default(),
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
        package_index: Default::default(),
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
    assert_eessi_cargo_isolation(&recipe.text);
    assert!(
        recipe.text.contains("PYTHONPATH"),
        "mesonpy leftovers still prepend the overlay prefix:\n{}",
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
        package_index: Default::default(),
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
        package_index: Default::default(),
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
    assert!(
        bundle.plan.residuals.iter().any(|residual| {
            residual.summary.contains("R bundle extension")
                && !residual.summary.contains("PythonBundle")
        }),
        "CRAN leftovers must not be described as PythonBundle: {:?}",
        bundle.plan.residuals
    );
}

/// A dependency written as a bare name is normal in CRAN, and an exts_list
/// entry still needs one concrete version. The repository index supplies it,
/// with the checksum the index publishes.
#[test]
fn plan_cran_takes_unpinned_versions_from_the_package_index() {
    let index = eb_stack::parse_package_index(
        "Package: processx\nVersion: 3.8.4\nMD5sum: 1111111111111111111111111111abcd\n\n\
         Package: rmarkdown\nVersion: 2.27\nMD5sum: 2222222222222222222222222222abcd\n",
    );
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/cran_unpinned/cran.json"),
        format: Some(ForeignFormat::Cran),
        toolchain: toolchain(),
        source_checksums: vec![
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
        ],
        package_layers: Vec::new(),
        package_index: index,
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cran_bundle/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan with an index");
    let recipe = &bundle.easyconfigs[0];
    assert!(
        recipe.text.contains("('processx', '3.8.4'"),
        "the index supplies the version:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("md5:1111111111111111111111111111abcd"),
        "and the checksum it publishes:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("cran.r-project.org/src/contrib"),
        "an R bundle needs somewhere to download from:\n{}",
        recipe.text
    );
    assert!(
        recipe
            .text
            .contains("cran.r-project.org/src/contrib/Archive/%(name)s"),
        "archived CRAN releases are not at contrib/:\n{}",
        recipe.text
    );
}

/// `prepare_new_package_plan` is the documented split before
/// `complete_package_bundle`. The index has to be on the plan it returns,
/// because those two callers do not copy it again.
#[test]
fn prepare_new_package_plan_keeps_the_request_package_index() {
    let index = eb_stack::parse_package_index(
        "Package: processx\nVersion: 3.8.4\nMD5sum: 1111111111111111111111111111abcd\n\n\
         Package: rmarkdown\nVersion: 2.27\nMD5sum: 2222222222222222222222222222abcd\n",
    );
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/cran_unpinned/cran.json"),
        format: Some(ForeignFormat::Cran),
        toolchain: toolchain(),
        source_checksums: vec![
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
        ],
        package_layers: Vec::new(),
        package_index: index,
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cran_bundle/robot")],
        stack_policy: stack_policy(),
    };
    let (plan, sbom) = prepare_new_package_plan(&request).expect("prepare");
    assert_eq!(
        plan.package_index, request.package_index,
        "prepare must copy the request index onto the plan"
    );
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots).expect("robot");
    let bundle = complete_package_bundle(plan, sbom, &tree.candidates, &request.stack_policy)
        .expect("complete from prepare without a second index copy");
    let recipe = &bundle.easyconfigs[0];
    assert!(
        recipe.text.contains("('processx', '3.8.4'"),
        "the index on the prepared plan supplies the leftover version:\n{}",
        recipe.text
    );
}

/// A language-bundle recipe is a different template from ConfigureMake.
/// Patches and configopts still have to land on it, or a CRAN/PyPI leftover
/// that carries either silently ships without them.
#[test]
fn language_bundle_emit_keeps_patches_and_configopts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let patch = temp.path().join("jsonlite-fix.patch");
    std::fs::write(&patch, "bundle patch\n").expect("write patch");
    let layer = PackageConfigLayer::from_toml_str(&format!(
        r#"
schema_version = 1
[[build.patches]]
filename = "jsonlite-fix.patch"
sha256 = "1b3cb9a974469894efcbb136177166fc6921db16b00683369582e0fe9a7165e9"
source = "{patch}"
[[profiles]]
name = "default"
default = true
config_options = ["--with-bundle-flag"]
"#,
        patch = patch.display(),
    ))
    .expect("layer");
    let request = NewPackageRequest {
        source: root().join("fixtures/foreign_ingest/cran_unpinned/cran.json"),
        format: Some(ForeignFormat::Cran),
        toolchain: toolchain(),
        source_checksums: vec![
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".into(),
        ],
        package_layers: vec![layer],
        package_index: eb_stack::parse_package_index(
            "Package: processx\nVersion: 3.8.4\nMD5sum: 1111111111111111111111111111abcd\n\n\
             Package: rmarkdown\nVersion: 2.27\nMD5sum: 2222222222222222222222222222abcd\n",
        ),
        easyconfig_roots: vec![root().join("fixtures/foreign_ingest/cran_bundle/robot")],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("plan language bundle");
    let recipe = &bundle.easyconfigs[0];
    assert!(
        recipe.text.contains("patches") && recipe.text.contains("jsonlite-fix.patch"),
        "bundle must keep the plan patch:\n{}",
        recipe.text
    );
    assert!(
        recipe.text.contains("configopts") && recipe.text.contains("--with-bundle-flag"),
        "bundle must keep plan configopts:\n{}",
        recipe.text
    );
}

fn write_pypi_robot(dir: &Path, bundle_exts: &str, extra_easyconfigs: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).expect("robot directory");
    std::fs::write(
        dir.join("Python-3.13.1-foss-2026.1.eb"),
        r#"name = 'Python'
version = '3.13.1'
homepage = 'https://python.org'
description = "Test Python provider"
toolchain = {'name': 'foss', 'version': '2026.1'}
moduleclass = 'lang'
"#,
    )
    .expect("write Python");
    std::fs::write(
        dir.join("Python-bundle-PyPI-2025.04-foss-2026.1.eb"),
        format!(
            r#"name = 'Python-bundle-PyPI'
version = '2025.04'
homepage = 'https://example.invalid/python-bundle-pypi'
description = "Test bundle"
toolchain = {{'name': 'foss', 'version': '2026.1'}}
dependencies = [
    ('Python', '3.13.1'),
]
exts_list = [
{bundle_exts}
]
moduleclass = 'lang'
"#
        ),
    )
    .expect("write bundle");
    for (filename, body) in extra_easyconfigs {
        std::fs::write(dir.join(filename), body).expect("write extra easyconfig");
    }
}

fn pypi_dump_with_requires(dir: &Path, requires: &[&str]) -> PathBuf {
    let path = dir.join("pypi.json");
    let requires_json = requires
        .iter()
        .map(|spec| format!("      \"{spec}\""))
        .collect::<Vec<_>>()
        .join(",\n");
    std::fs::write(
        &path,
        format!(
            r#"{{
  "info": {{
    "name": "demo",
    "version": "1.0.0",
    "home_page": "https://example.invalid/demo",
    "summary": "availability fixture",
    "license": "MIT",
    "requires_dist": [
{requires_json}
    ]
  }},
  "urls": [
    {{
      "packagetype": "sdist",
      "url": "https://example.invalid/demo-1.0.0.tar.gz",
      "filename": "demo-1.0.0.tar.gz",
      "digests": {{
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      }}
    }}
  ]
}}
"#
        ),
    )
    .expect("write pypi dump");
    path
}

/// A PEP 517 backend shipped only as a bundle extension on the target
/// generation is still the module the recipe must name.
#[test]
fn hatchling_ext_on_target_generation_is_named_as_backend() {
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    write_pypi_robot(
        &robot,
        "    ('soupsieve', '2.6'),\n    ('hatchling', '1.27.0'),",
        &[],
    );
    let source = pypi_dump_with_requires(temp.path(), &["soupsieve>=1.6"]);
    let request = NewPackageRequest {
        source,
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        package_index: Default::default(),
        easyconfig_roots: vec![robot],
        stack_policy: stack_policy(),
    };
    let (mut plan, sbom) = prepare_new_package_plan(&request).expect("prepare");
    plan.build
        .build_systems
        .push("backend:hatchling.build".into());
    let tree = parse_easyconfig_trees(&[request.easyconfig_roots[0].as_path()]).expect("robot");
    let bundle = complete_package_bundle(plan, sbom, &tree.candidates, &request.stack_policy)
        .expect("complete with hatchling ext");
    assert!(
        bundle.plan.dependencies.iter().any(|dependency| {
            overlay_package_identity(
                dependency
                    .eb_name
                    .as_deref()
                    .unwrap_or(dependency.name.as_str()),
            ) == overlay_package_identity("hatchling")
        }),
        "hatchling must be named when the bundle already ships it:\n{:#?}",
        bundle.plan.dependencies
    );
}

/// CMake that exists only on an older foss generation is not a build tool
/// this generation can inject.
#[test]
fn cmake_only_on_older_generation_is_not_injected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    write_pypi_robot(
        &robot,
        "    ('soupsieve', '2.6'),",
        &[(
            "CMake-3.26.3-foss-2023a.eb",
            r#"name = 'CMake'
version = '3.26.3'
homepage = 'https://cmake.org'
description = "Older generation CMake"
toolchain = {'name': 'foss', 'version': '2023a'}
moduleclass = 'tools'
"#,
        )],
    );
    let source = pypi_dump_with_requires(temp.path(), &["soupsieve>=1.6", "orphanpkg==1.2.3"]);
    let request = NewPackageRequest {
        source,
        format: Some(ForeignFormat::Pypi),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        package_index: Default::default(),
        easyconfig_roots: vec![robot],
        stack_policy: stack_policy(),
    };
    let bundle = plan_new_package(&request).expect("foreign-generation CMake must not be injected");
    assert!(
        bundle
            .plan
            .overlay_extensions
            .iter()
            .any(|extension| extension.name == "orphanpkg"),
        "leftover must become an overlay so build-tool injection runs:\n{:#?}",
        bundle.plan.overlay_extensions
    );
    assert!(
        bundle.plan.dependencies.iter().all(|dependency| {
            overlay_package_identity(
                dependency
                    .eb_name
                    .as_deref()
                    .unwrap_or(dependency.name.as_str()),
            ) != overlay_package_identity("CMake")
        }),
        "CMake from foss-2023a must not be injected into foss-2026.1:\n{:#?}",
        bundle.plan.dependencies
    );
}
