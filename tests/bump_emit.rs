//! Integration tests for next-generation easyconfig emit (library + CLI).

use eb_stack::{
    easyconfig_filename, emit_next_generation, emit_next_generation_auto_from_path,
    emit_next_generation_from_path, EmitParams, Toolchain,
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
        source_checksum: None,
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
        source_checksum: None,
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
            "package",
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
                "package",
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

fn hierarchy_universe() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/hierarchy_resolve/easyconfigs")
}

fn gromacs_repro_source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb")
}

fn gromacs_repro_universe() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/repro_fixtures/universe_foss_2024a")
}

/// Library auto-resolve fills dep versions from a multi-subtoolchain universe
/// with an empty hand override map.
#[test]
fn library_auto_resolve_gromacs_deps_from_universe() {
    let empty = HashMap::new();
    let r = emit_next_generation_auto_from_path(
        &gromacs_repro_source(),
        &foss("2024a"),
        &gromacs_repro_universe(),
        None,
        None,
        &empty,
        None,
        None,
    )
    .expect("auto emit");
    assert_eq!(r.filename, "GROMACS-2024.4-foss-2024a.eb");
    assert!(r
        .text
        .contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
    assert!(r.text.contains("('CMake', '3.29.3')"));
    assert!(r.text.contains("('scikit-build-core', '0.11.1')"));
    assert!(r.text.contains("('Python', '3.12.3')"));
    assert!(r.text.contains("('SciPy-bundle', '2024.05')"));
    assert!(r.text.contains("('networkx', '3.4.2')"));
    assert!(r.text.contains("('mpi4py', '4.0.1')"));
    // pybind11 is maintainer-added; source does not list it.
    assert!(!r.text.contains("pybind11"));
}

/// CLI `bump --easyconfigs` with no `--dep` resolves versions from the universe.
#[test]
fn cli_bump_auto_resolve_from_easyconfigs() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = gromacs_repro_source();
    let universe = gromacs_repro_universe();
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("out.eb");

    let status = Command::new(bin)
        .args([
            "package",
            "bump",
            "--source",
            src.to_str().unwrap(),
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2024a",
            "--easyconfigs",
            universe.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn eb-stack bump --easyconfigs");
    assert!(status.success(), "auto bump failed: {status}");
    let text = std::fs::read_to_string(&out).expect("read");
    assert!(text.contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
    assert!(text.contains("('Python', '3.12.3')"));
    assert!(text.contains("('mpi4py', '4.0.1')"));
    assert!(text.contains("('CMake', '3.29.3')"));
}

/// Hierarchy unit universe: multi-level resolve picks generation members, not
/// exact foss-only filtering (also exercised in hierarchy module tests).
#[test]
fn library_auto_resolve_uses_hierarchy_not_exact_toolchain_only() {
    // Minimal source that lists only Python + SciPy-bundle.
    let src = "\
name = 'Demo'
version = '1.0'
toolchain = {'name': 'foss', 'version': '2023b'}
homepage = 'https://example.invalid'
dependencies = [
    ('Python', '3.11.5'),
    ('SciPy-bundle', '2023.11'),
]
";
    let tmp = tempfile::tempdir().unwrap();
    let src_path = tmp.path().join("Demo-1.0-foss-2023b.eb");
    std::fs::write(&src_path, src).unwrap();
    let empty = HashMap::new();
    let r = emit_next_generation_auto_from_path(
        &src_path,
        &foss("2024a"),
        &hierarchy_universe(),
        None,
        None,
        &empty,
        None,
        None,
    )
    .expect("auto");
    assert!(r.text.contains("('Python', '3.12.3')"));
    assert!(r.text.contains("('SciPy-bundle', '2024.05')"));
    // Decoys at GCCcore-14.2.0 / gfbf-2025a must not win.
    assert!(!r.text.contains("3.13.1"));
    assert!(!r.text.contains("2025.06"));
}
