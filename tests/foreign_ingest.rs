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

#[test]
fn conda_fixture_parse_and_emit_reparses() {
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
    assert!(out.text.contains("zlib-1.3.1.tar.gz"));
    assert!(
        out.text.contains("make") || out.text.contains("libgcc-ng"),
        "deps must appear in scaffold text"
    );
    let r = resolve_easyconfig_str(&out.text).expect("re-parse emitted eb");
    assert_eq!(r.name, "zlib");
    assert_eq!(r.version, "1.3.1");
    assert_eq!(r.toolchain.name, "foss");
    assert_eq!(r.toolchain.version, "2024a");
    assert!(!r.checksums.is_empty());
}

#[test]
fn spack_fixture_parse_and_emit_reparses() {
    let path = root().join("spack_zlib/package.py");
    assert_eq!(detect_foreign_format(&path), Some(ForeignFormat::Spack));
    let recipe = parse_foreign_path(&path, None).expect("parse spack fixture");
    assert_eq!(recipe.name, "zlib");
    assert_eq!(recipe.version, "1.3.1");
    assert!(recipe.source_url.as_ref().is_some_and(|u| u.contains("zlib")));
    let names: Vec<_> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"gmake"), "{names:?}");

    let out = ingest_foreign_to_easyconfig(&path, Some(ForeignFormat::Spack), &foss())
        .expect("ingest spack");
    assert!(out.text.contains("name = 'zlib'"));
    assert!(out.text.contains("version = '1.3.1'"));
    assert!(out.text.contains("spack") || out.text.contains("gmake"));
    let r = resolve_easyconfig_str(&out.text).expect("re-parse");
    assert_eq!(r.name, "zlib");
    assert_eq!(r.version, "1.3.1");
}

#[test]
fn cli_ingest_conda_writes_parseable_eb() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = root().join("conda_zlib/meta.yaml");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("out.eb");
    let status = Command::new(bin)
        .args([
            "ingest",
            "--source",
            src.to_str().unwrap(),
            "--format",
            "conda-forge",
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2024a",
            "--out",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn ingest");
    assert!(status.success(), "ingest failed: {status}");
    assert!(out.is_file());
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("name = 'zlib'"));
    assert!(text.contains("version = '1.3.1'"));
    assert!(text.contains("zlib-1.3.1.tar.gz"));
    let r = resolve_easyconfig_file(&out).expect("CLI output re-parse");
    assert_eq!(r.name, "zlib");
    assert_eq!(r.version, "1.3.1");
}

#[test]
fn cli_ingest_spack_writes_parseable_eb() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = root().join("spack_zlib/package.py");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("Zlib-from-spack.eb");
    let status = Command::new(bin)
        .args([
            "ingest",
            "--source",
            src.to_str().unwrap(),
            "--format",
            "spack",
            "--out",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(status.success(), "spack ingest failed: {status}");
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("name = 'zlib'"));
    assert!(text.contains("version = '1.3.1'"));
    let r = resolve_easyconfig_file(Path::new(&out)).expect("re-parse spack out");
    assert_eq!(r.name, "zlib");
}
