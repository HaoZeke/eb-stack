//! Integration tests for next-generation easyconfig emit (library + CLI).

use eb_stack::{
    emit_next_generation, emit_next_generation_from_path, easyconfig_filename, EmitParams, Toolchain,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/gromacs_2025_to_next/easyconfigs")
        .join(rel)
}

fn foss(ver: &str) -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: ver.into(),
    }
}

#[test]
fn library_bumps_fixture_gromacs_toolchain_only() {
    let src_path = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let source = std::fs::read_to_string(&src_path).expect("read fixture");
    let params = EmitParams {
        toolchain: foss("2025b"),
        version: None,
        dep_versions: HashMap::new(),
    };
    let r = emit_next_generation(&source, &params).expect("emit");
    assert_eq!(r.filename, "GROMACS-2024.1-foss-2025b.eb");
    assert!(r
        .text
        .contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
    // Application version unchanged.
    assert!(r.text.contains("version = '2024.1'"));
    // Non-target fields preserved from fixture.
    assert!(r
        .text
        .contains("toolchainopts = {'openmp': True, 'usempi': True}"));
    assert!(r.text.contains("('OpenBLAS', '0.3.23')"));
    assert!(r.text.contains("('OpenMPI', '4.1.5')"));
    assert!(r.text.contains("('FFTW', '3.3.10')"));
}

#[test]
fn library_bumps_version_toolchain_and_deps() {
    let src_path = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let mut deps = HashMap::new();
    deps.insert("OpenBLAS".into(), "0.3.27".into());
    deps.insert("OpenMPI".into(), "5.0.3".into());
    let params = EmitParams {
        toolchain: foss("2025b"),
        version: Some("2025.0".into()),
        dep_versions: deps,
    };
    let r = emit_next_generation_from_path(&src_path, &params).expect("emit from path");
    assert_eq!(
        r.filename,
        easyconfig_filename("GROMACS", "2025.0", &foss("2025b"))
    );
    assert_eq!(r.filename, "GROMACS-2025.0-foss-2025b.eb");
    assert!(r.text.contains("version = '2025.0'"));
    assert!(r
        .text
        .contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
    assert!(r.text.contains("('OpenBLAS', '0.3.27')"));
    assert!(r.text.contains("('OpenMPI', '5.0.3')"));
    assert!(r.text.contains("('FFTW', '3.3.10')"));
    assert!(r.text.contains("name = 'GROMACS'"));
}

#[test]
fn cli_bump_writes_conventional_file() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path();

    let status = Command::new(bin)
        .args([
            "bump",
            "--source",
            src.to_str().unwrap(),
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2025b",
            "--version",
            "2025.0",
            "--dep",
            "OpenBLAS=0.3.27",
            "--dep",
            "OpenMPI=5.0.3",
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .status()
        .expect("spawn eb-stack bump");
    assert!(status.success(), "eb-stack bump failed: {status}");

    let written = out_dir.join("GROMACS-2025.0-foss-2025b.eb");
    assert!(written.is_file(), "missing {}", written.display());
    let text = std::fs::read_to_string(&written).expect("read written");
    assert!(text.contains("version = '2025.0'"));
    assert!(text.contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
    assert!(text.contains("('OpenBLAS', '0.3.27')"));
    assert!(text.contains("('OpenMPI', '5.0.3')"));
    assert!(text.contains("toolchainopts = {'openmp': True, 'usempi': True}"));
}

#[test]
fn cli_bump_toolchain_only_twice_idempotent_content() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let tmp = tempfile::tempdir().expect("tempdir");

    for i in 0..2 {
        let out = tmp.path().join(format!("run{i}.eb"));
        let status = Command::new(bin)
            .args([
                "bump",
                "--source",
                src.to_str().unwrap(),
                "--toolchain-name",
                "foss",
                "--toolchain-version",
                "2025b",
                "--out",
                out.to_str().unwrap(),
            ])
            .status()
            .expect("spawn");
        assert!(status.success(), "run {i} failed");
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
        assert!(text.contains("version = '2024.1'"));
        if i == 1 {
            let first = std::fs::read_to_string(tmp.path().join("run0.eb")).unwrap();
            assert_eq!(first, text, "two CLI runs must produce identical content");
        }
    }
}
