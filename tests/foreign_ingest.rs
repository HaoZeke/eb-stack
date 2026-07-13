//! Foreign recipe ingest: conda-forge + Spack fixtures → EasyBuild scaffold.
//!
//! Drives the shipped library and CLI paths (not a reimplementation).

use eb_stack::{
    detect_foreign_format, ingest_foreign_to_easyconfig, parse_foreign_path, resolve_easyconfig_file,
    resolve_easyconfig_str, ForeignFormat, Toolchain,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/foreign_ingest")
}

fn foss() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
    }
}

fn foss_2026() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

#[test]
fn conda_zlib_parse_and_emit_reparses() {
    let path = root().join("conda_zlib/meta.yaml");
    assert_eq!(
        detect_foreign_format(&path),
        Some(ForeignFormat::CondaForge)
    );
    let recipe = parse_foreign_path(&path, None).expect("parse conda fixture");
    assert_eq!(recipe.name, "zlib");
    assert_eq!(recipe.version, "1.3.1");
    assert!(
        recipe
            .source_url
            .as_ref()
            .is_some_and(|u| u.contains("zlib-1.3.1.tar.gz")),
        "source from fixture: {:?}",
        recipe.source_url
    );
    let names: Vec<_> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"make"), "{names:?}");
    assert!(names.contains(&"libgcc-ng"), "{names:?}");

    let out = ingest_foreign_to_easyconfig(&path, None, &foss()).expect("ingest");
    assert!(out.text.contains("name = 'zlib'"));
    assert!(out.text.contains("version = '1.3.1'"));
    let r = resolve_easyconfig_str(&out.text).expect("re-parse emitted eb");
    assert_eq!(r.name, "zlib");
    assert_eq!(r.version, "1.3.1");
    assert_eq!(r.toolchain.name, "foss");
}

#[test]
fn conda_eon_rattler_recipe_expands_context_and_multi_source() {
    let path = root().join("conda_eon/recipe.yaml");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::CondaForge))
        .expect("parse eon recipe.yaml");
    assert_eq!(recipe.name, "eon");
    assert_eq!(recipe.version, "2.16.0", "context version expanded");
    assert!(
        recipe.sources.len() >= 3,
        "multi-source eon: got {}",
        recipe.sources.len()
    );
    assert!(
        recipe
            .source_url
            .as_ref()
            .is_some_and(|u| u.contains("eon-v2.16.0") || u.contains("2.16.0")),
        "primary source: {:?}",
        recipe.source_url
    );
    assert!(
        recipe.sha256.as_ref().is_some_and(|s| s.len() == 64),
        "sha256: {:?}",
        recipe.sha256
    );
    let names: Vec<_> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    assert!(names.iter().any(|n| n.contains("metatomic") || *n == "xtb" || *n == "quill"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.contains("compiler(")),
        "compiler macros must be skipped: {names:?}"
    );

    let out = ingest_foreign_to_easyconfig(&path, None, &foss_2026()).expect("ingest eon");
    assert!(
        out.text.contains("name = 'eOn'") || out.recipe.name == "eOn",
        "EB-style eOn casing: name={:?} text has eon? {}",
        out.recipe.name,
        out.text.contains("name = 'eon'")
    );
    assert!(out.text.contains("version = '2.16.0'"));
    // Meson in build reqs → MesonNinja easyblock
    assert!(
        out.text.contains("MesonNinja") || out.text.contains("CMakeNinja") || out.text.contains("ConfigureMake"),
        "easyblock present: {}",
        out.text.lines().find(|l| l.contains("easyblock")).unwrap_or("?")
    );
    let r = resolve_easyconfig_str(&out.text).expect("re-parse eon scaffold");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.16.0");
    assert_eq!(r.toolchain.version, "2026.1");
    // Toolchain virtuals / conda packaging noise must not appear as modules.
    assert!(
        !out.text.contains("('pip',") && !out.text.contains("('setuptools',"),
        "conda packaging noise should not be EB deps: {}",
        out.text
    );
}

#[test]
fn spack_zlib_parse_and_emit_reparses() {
    let path = root().join("spack_zlib/package.py");
    assert_eq!(detect_foreign_format(&path), Some(ForeignFormat::Spack));
    let recipe = parse_foreign_path(&path, None).expect("parse spack fixture");
    assert_eq!(recipe.name, "zlib");
    assert_eq!(recipe.version, "1.3.1");
    let names: Vec<_> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"gmake"), "{names:?}");

    let out = ingest_foreign_to_easyconfig(&path, Some(ForeignFormat::Spack), &foss())
        .expect("ingest spack");
    let r = resolve_easyconfig_str(&out.text).expect("re-parse");
    assert_eq!(r.name, "zlib");
    assert_eq!(r.version, "1.3.1");
}

#[test]
fn spack_eon_real_package_py() {
    let path = root().join("spack_eon/package.py");
    let recipe = parse_foreign_path(&path, None).expect("parse spack eon");
    assert_eq!(recipe.name, "eon");
    assert_eq!(recipe.version, "2.16.0");
    assert!(
        recipe.sha256.as_ref().is_some_and(|s| s.starts_with("3d4da89a")),
        "{:?}",
        recipe.sha256
    );
    assert!(
        recipe
            .build_system_hints
            .iter()
            .any(|h| h.contains("Meson")),
        "hints: {:?}",
        recipe.build_system_hints
    );
    let names: Vec<_> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"eigen"), "{names:?}");
    assert!(names.contains(&"quill"), "{names:?}");
    assert!(names.contains(&"highway") || names.contains(&"libinih"), "{names:?}");

    let out = ingest_foreign_to_easyconfig(&path, None, &foss_2026()).expect("ingest");
    assert!(
        out.text.contains("easyblock = 'MesonNinja'"),
        "expected MesonNinja from MesonPackage: {}",
        out.text.lines().find(|l| l.starts_with("easyblock")).unwrap_or("")
    );
    let r = resolve_easyconfig_str(&out.text).unwrap();
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.16.0");
}

#[test]
fn spack_qmcpack_multi_base_skips_develop() {
    let path = root().join("spack_qmcpack/package.py");
    let recipe = parse_foreign_path(&path, None).expect("parse spack qmcpack");
    assert_eq!(recipe.name, "qmcpack");
    assert_eq!(
        recipe.version, "4.3.0",
        "first non-develop version, not develop"
    );
    assert!(
        recipe
            .build_system_hints
            .iter()
            .any(|h| h.contains("CMake")),
        "{:?}",
        recipe.build_system_hints
    );
    let names: Vec<_> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"boost"), "{names:?}");
    assert!(names.contains(&"hdf5"), "{names:?}");
    assert!(names.contains(&"libxml2"), "{names:?}");
    assert!(names.contains(&"python"), "{names:?}");

    let out = ingest_foreign_to_easyconfig(&path, None, &foss_2026()).expect("ingest");
    assert!(
        out.text.contains("easyblock = 'CMakeNinja'"),
        "{}",
        out.text.lines().find(|l| l.starts_with("easyblock")).unwrap_or("")
    );
    // tag-only version → placeholder checksum warning path still re-parses
    let r = resolve_easyconfig_str(&out.text).unwrap();
    assert_eq!(r.name, "QMCPACK", "EB-style title casing from Spack Qmcpack");
    assert_eq!(r.version, "4.3.0");
    // Toolchain virtuals (blas/lapack/mpi) must not be emitted as residual modules.
    assert!(
        !out.text.contains("('blas',")
            && !out.text.contains("('lapack',")
            && !out.text.contains("('mpi',"),
        "virtuals must not be EB deps: {}",
        out.text
    );
}

#[test]
fn cli_ingest_conda_eon_and_spack_qmcpack() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");

    let eon_out = tmp.path().join("eon.eb");
    let st = Command::new(bin)
        .args([
            "ingest",
            "--source",
            root().join("conda_eon/recipe.yaml").to_str().unwrap(),
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2026.1",
            "--out",
            eon_out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(st.success(), "conda eon ingest failed");
    let r = resolve_easyconfig_file(&eon_out).expect("re-parse eon");
    assert_eq!(r.name, "eOn");
    assert_eq!(r.version, "2.16.0");

    let qmc_out = tmp.path().join("qmc.eb");
    let st = Command::new(bin)
        .args([
            "ingest",
            "--source",
            root().join("spack_qmcpack/package.py").to_str().unwrap(),
            "--format",
            "spack",
            "--out",
            qmc_out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(st.success(), "spack qmcpack ingest failed");
    let r = resolve_easyconfig_file(Path::new(&qmc_out)).expect("re-parse qmc");
    assert_eq!(r.name, "QMCPACK");
    assert_eq!(r.version, "4.3.0");
}

#[test]
fn ingest_with_robot_resolves_dep_versions_from_hierarchy() {
    let path = root().join("spack_eon/package.py");
    let robot = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/hierarchy_resolve/easyconfigs");
    let opts = eb_stack::IngestOpts {
        easyconfigs: vec![robot],
        keep_old_deps: true,
        hierarchy_fixture: None,
    };
    let out = eb_stack::ingest_foreign_to_easyconfig_with_opts(
        &path,
        Some(ForeignFormat::Spack),
        &foss(),
        &opts,
    )
    .expect("ingest+robot");
    // Robot has Python-3.12.3 and CMake for foss-2024a hierarchy.
    assert!(
        out.text.contains("('Python', '3.12.3')"),
        "expected robot-resolved Python 3.12.3 from universe: {}\n{}",
        out.text,
        out.warnings.join("\n")
    );
    assert!(
        out.warnings.iter().any(|w| w.contains("robot resolve")),
        "expected robot resolve warnings: {:?}",
        out.warnings
    );
    // Joint resolvo path must fire when hierarchy candidates exist for deps.
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("resolvo") && w.contains("joint")),
        "expected resolvo joint co-select warning: {:?}",
        out.warnings
    );
    // Spack eon meson_args static -D flags should appear when present
    let r = resolve_easyconfig_str(&out.text).expect("re-parse after robot resolve");
    assert_eq!(r.version, "2.16.0");
    // Resolved Python version must match a real hierarchy candidate, not a foreign floor.
    let py = r
        .dependencies
        .iter()
        .chain(r.builddependencies.iter())
        .find(|d| d.name == "Python")
        .expect("Python dep present after robot resolve");
    assert_eq!(py.version, "3.12.3");
}

#[test]
fn spack_eon_extracts_static_meson_configopts() {
    let path = root().join("spack_eon/package.py");
    let recipe = parse_foreign_path(&path, None).expect("parse");
    let opts = recipe.configopts.as_deref().unwrap_or("");
    // Static -D flags from meson_args (not f-string dynamics)
    assert!(
        opts.contains("-Dwith_xtb=false") || opts.contains("-Dwith_metatomic=false"),
        "expected static meson -D flags, got {opts:?}"
    );
    let out = ingest_foreign_to_easyconfig(&path, None, &foss()).expect("emit");
    assert!(
        out.text.contains("configopts") && out.text.contains("-Dwith_"),
        "{}",
        out.text
    );
}
