//! Integration tests for next-generation easyconfig emit (library + CLI).

use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::{plan_package_bump, BumpPackageRequest, PackageBundle, Toolchain};
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

fn bump(
    source: PathBuf,
    target: Toolchain,
    easyconfigs: PathBuf,
    version: Option<&str>,
    overrides: HashMap<String, String>,
) -> PackageBundle {
    plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: target.clone(),
        version: version.map(str::to_string),
        source_checksum: None,
        easyconfig_roots: vec![easyconfigs],
        hierarchy_fixture: None,
        overrides,
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "test".into(),
            toolchain: target,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect("canonical bump")
}

fn bundle_recipe(out: &std::path::Path, package: &str, filename: &str) -> PathBuf {
    out.join("easyconfigs")
        .join(package[..1].to_ascii_lowercase())
        .join(package)
        .join(filename)
}

#[test]
fn library_bumps_fixture_gromacs_toolchain_only() {
    let src_path = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let overrides = HashMap::from([
        ("OpenBLAS".into(), "0.3.23".into()),
        ("OpenMPI".into(), "4.1.5".into()),
        ("FFTW".into(), "3.3.10".into()),
    ]);
    let bundle = bump(src_path, foss("2025b"), fixture(""), None, overrides);
    let recipe = &bundle.easyconfigs[0];
    assert_eq!(recipe.filename, "GROMACS-2024.1-foss-2025b.eb");
    assert!(recipe
        .text
        .contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
    assert!(recipe.text.contains("version = '2024.1'"));
    assert!(recipe
        .text
        .contains("toolchainopts = {'openmp': True, 'usempi': True}"));
    assert!(recipe.text.contains("('OpenBLAS', '0.3.23')"));
    assert!(recipe.text.contains("('OpenMPI', '4.1.5')"));
    assert!(recipe.text.contains("('FFTW', '3.3.10')"));
    assert_eq!(bundle.sbom["bomFormat"], "CycloneDX");
    assert_eq!(bundle.locks.len(), 1);
}

#[test]
fn library_bumps_version_toolchain_and_deps() {
    let src_path = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let mut deps = HashMap::new();
    deps.insert("OpenBLAS".into(), "0.3.27".into());
    deps.insert("OpenMPI".into(), "5.0.3".into());
    let bundle = bump(src_path, foss("2025b"), fixture(""), Some("2025.0"), deps);
    let recipe = &bundle.easyconfigs[0];
    assert_eq!(recipe.filename, "GROMACS-2025.0-foss-2025b.eb");
    assert!(recipe.text.contains("version = '2025.0'"));
    assert!(recipe
        .text
        .contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
    assert!(recipe.text.contains("('OpenBLAS', '0.3.27')"));
    assert!(recipe.text.contains("('OpenMPI', '5.0.3')"));
    assert!(recipe.text.contains("('FFTW', '3.3.10')"));
    assert!(recipe.text.contains("name = 'GROMACS'"));
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
            "--easyconfigs",
            fixture("").to_str().unwrap(),
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .status()
        .expect("spawn eb-stack bump");
    assert!(status.success(), "eb-stack bump failed: {status}");

    let written = bundle_recipe(out_dir, "GROMACS", "GROMACS-2025.0-foss-2025b.eb");
    assert!(written.is_file(), "missing {}", written.display());
    assert!(out_dir.join("package.plan.json").is_file());
    assert!(out_dir.join("package.sbom.cdx.json").is_file());
    assert!(out_dir.join("locks/default.lock.json").is_file());
    let text = std::fs::read_to_string(&written).expect("read written");
    assert!(text.contains("version = '2025.0'"));
    assert!(text.contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
    assert!(text.contains("('OpenBLAS', '0.3.27')"));
    assert!(text.contains("('OpenMPI', '5.0.3')"));
    assert!(text.contains("toolchainopts = {'openmp': True, 'usempi': True}"));
}

#[test]
fn cli_bump_resolvo_bundle_is_deterministic() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = fixture("foss-2025a/GROMACS-2024.1-foss-2025a.eb");
    let tmp = tempfile::tempdir().expect("tempdir");

    for i in 0..2 {
        let out = tmp.path().join(format!("run{i}"));
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
                "--easyconfigs",
                fixture("").to_str().unwrap(),
                "--out-dir",
                out.to_str().unwrap(),
            ])
            .status()
            .expect("spawn");
        assert!(status.success(), "run {i} failed");
        let recipe = bundle_recipe(&out, "GROMACS", "GROMACS-2024.1-foss-2025b.eb");
        let text = std::fs::read_to_string(&recipe).unwrap();
        assert!(text.contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
        assert!(text.contains("version = '2024.1'"));
        assert!(out.join("package.sbom.cdx.json").is_file());
        assert!(out.join("locks/default.lock.json").is_file());
        if i == 1 {
            let first = std::fs::read_to_string(bundle_recipe(
                &tmp.path().join("run0"),
                "GROMACS",
                "GROMACS-2024.1-foss-2025b.eb",
            ))
            .unwrap();
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
    let bundle = bump(
        gromacs_repro_source(),
        foss("2024a"),
        gromacs_repro_universe(),
        None,
        HashMap::new(),
    );
    let recipe = &bundle.easyconfigs[0];
    assert_eq!(recipe.filename, "GROMACS-2024.4-foss-2024a.eb");
    assert!(recipe
        .text
        .contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
    assert!(recipe.text.contains("('CMake', '3.29.3')"));
    assert!(recipe.text.contains("('scikit-build-core', '0.11.1')"));
    assert!(recipe.text.contains("('Python', '3.12.3')"));
    assert!(recipe.text.contains("('SciPy-bundle', '2024.05')"));
    assert!(recipe.text.contains("('networkx', '3.4.2')"));
    assert!(recipe.text.contains("('mpi4py', '4.0.1')"));
    assert!(!recipe.text.contains("pybind11"));
}

/// CLI `bump --easyconfigs` with no `--dep` resolves versions from the universe.
#[test]
fn cli_bump_auto_resolve_from_easyconfigs() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let src = gromacs_repro_source();
    let universe = gromacs_repro_universe();
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("bundle");

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
            "--out-dir",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn eb-stack bump --easyconfigs");
    assert!(status.success(), "auto bump failed: {status}");
    let recipe = bundle_recipe(&out, "GROMACS", "GROMACS-2024.4-foss-2024a.eb");
    let text = std::fs::read_to_string(recipe).expect("read");
    assert!(out.join("package.sbom.cdx.json").is_file());
    assert!(out.join("locks/default.lock.json").is_file());
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
    let bundle = bump(
        src_path,
        foss("2024a"),
        hierarchy_universe(),
        None,
        HashMap::new(),
    );
    let recipe = &bundle.easyconfigs[0].text;
    assert!(recipe.contains("('Python', '3.12.3')"));
    assert!(recipe.contains("('SciPy-bundle', '2024.05')"));
    assert!(!recipe.contains("3.13.1"));
    assert!(!recipe.contains("2025.06"));
}
