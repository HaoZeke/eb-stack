//! The QMCPACK and eOn easyblock layers: the plan resolves to the derived
//! class, ships the module, and the emitted recipes carry none of what the
//! easyblock derives.

use eb_stack::package::{LockedDependency, ProfileLock, PROFILE_LOCK_SCHEMA_VERSION};
use eb_stack::package_config::{apply_package_layers, PackageConfigLayer};
use eb_stack::{
    emit_profile_easyconfigs, package_plan_from_foreign, parse_foreign_path, ForeignFormat,
    Toolchain,
};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn locked(name: &str, version: &str, build: bool) -> LockedDependency {
    LockedDependency {
        name: name.into(),
        version: version.into(),
        versionsuffix: None,
        toolchain: toolchain(),
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        build,
    }
}

fn lock(package: &str, version: &str, profile: &str, versionsuffix: &str) -> ProfileLock {
    ProfileLock {
        schema_version: PROFILE_LOCK_SCHEMA_VERSION,
        package: package.into(),
        version: version.into(),
        profile: profile.into(),
        toolchain: toolchain(),
        versionsuffix: versionsuffix.into(),
        dependencies: vec![
            locked("CMake", "4.2.1", true),
            locked("Python", "3.14.2", false),
        ],
        pin_outcomes: Vec::new(),
        exclusions: Vec::new(),
        solver: "resolvo".into(),
    }
}

fn layered_plan(
    fixture: &str,
    format: ForeignFormat,
    layers: &[&str],
) -> eb_stack::package::PackagePlan {
    let recipe =
        parse_foreign_path(&root().join(fixture), Some(format)).expect("parse foreign fixture");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    let layers: Vec<PackageConfigLayer> = layers
        .iter()
        .map(|layer| {
            PackageConfigLayer::from_path(&root().join("examples/packages").join(layer))
                .expect("package layer")
        })
        .collect();
    apply_package_layers(&mut plan, &layers).expect("apply package layers");
    plan
}

#[test]
fn qmcpack_easyblock_layer_moves_the_variant_into_the_class() {
    let plan = layered_plan(
        "fixtures/foreign_ingest/spack_qmcpack/package.py",
        ForeignFormat::Spack,
        &["qmcpack.toml", "qmcpack-easyblock.toml"],
    );
    assert_eq!(plan.build.easyblock.as_deref(), Some("EB_QMCPACK"));
    let module = plan
        .build
        .easyblocks
        .iter()
        .find(|module| module.filename == "qmcpack.py")
        .expect("qmcpack.py shipped with the layer");
    let bytes = std::fs::read(module.resolved_source.as_ref().expect("resolved source"))
        .expect("read shipped module");
    assert!(
        String::from_utf8_lossy(&bytes).contains("class EB_QMCPACK(CMakeNinja)"),
        "the shipped module defines EB_QMCPACK on CMakeNinja"
    );
    for profile in &plan.profiles {
        assert!(
            profile.config_options.is_empty(),
            "{}: config options belong to the easyblock: {:?}",
            profile.name,
            profile.config_options
        );
    }

    let emitted = emit_profile_easyconfigs(
        &plan,
        &[
            lock("QMCPACK", &plan.package.version, "default", ""),
            lock("QMCPACK", &plan.package.version, "complex", "-complex"),
        ],
    )
    .expect("emit the layered recipes");
    assert_eq!(emitted.len(), 2);
    for recipe in &emitted {
        let text = &recipe.text;
        assert!(
            !text.contains("easyblock ="),
            "{}: derived class is not spelled out",
            recipe.filename
        );
        for derived in [
            "-DQMC_COMPLEX",
            "-DQMC_MPI",
            "-DBUILD_AFQMC",
            "sanity_check_paths",
            "sanity_check_commands",
            "test_cmd",
            "testopts",
            "pretestopts",
            "modextrapaths",
            "start_dir",
        ] {
            assert!(
                !text.contains(derived),
                "{}: {derived} is the easyblock's: {text}",
                recipe.filename
            );
        }
        assert!(
            text.contains("runtest = True"),
            "{}: tests run through the easyblock",
            recipe.filename
        );
    }
    assert!(emitted[1].text.contains("versionsuffix = '-complex'"));
}

#[test]
fn eon_easyblock_layer_drops_the_meson_feature_flags() {
    let plan = layered_plan(
        "fixtures/foreign_ingest/conda_eon/recipe.yaml",
        ForeignFormat::CondaForge,
        &["eon.toml", "eon-easyblock.toml"],
    );
    assert_eq!(plan.package.name, "eOn");
    assert_eq!(plan.build.easyblock.as_deref(), Some("EB_eOn"));
    assert!(plan
        .build
        .easyblocks
        .iter()
        .any(|module| module.filename == "eon.py"));
    let default = plan
        .profiles
        .iter()
        .find(|profile| profile.default)
        .expect("default profile");
    assert!(
        !default
            .config_options
            .iter()
            .any(|option| option.starts_with("-Dwith_")),
        "feature flags belong to the easyblock: {:?}",
        default.config_options
    );
    assert!(default
        .config_options
        .iter()
        .any(|option| option == "--wrap-mode=nodownload"));
}
