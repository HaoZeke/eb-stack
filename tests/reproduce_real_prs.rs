//! Mechanical reproduction suite: prove `eb-stack`'s toolchain-bump emit
//! reproduces real EasyBuild maintainer PRs byte-for-byte (modulo any
//! genuinely-new dependency lines the maintainer hand-added, which no
//! mechanical bump can invent).
//!
//! Each fixture pair is a real `(source, target)` easyconfig lifted from a
//! local EasyBuild checkout: same application version, adjacent `foss`
//! toolchain generation (`2023b` -> `2024a`).
//!
//! Two drive modes:
//! 1. **Hand map** (historical): explicit per-dependency version map.
//! 2. **Auto-resolve** (hierarchy-aware): only source, target generation, and
//!    a bundled easyconfig universe — no hand-fed dep versions. Versions are
//!    resolved by accepting recipes whose toolchain is any member of the
//!    target generation's sub-toolchain hierarchy.

use eb_stack::{
    emit_next_generation_auto_from_path, emit_next_generation_from_path, EmitParams, Toolchain,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/repro_fixtures")
        .join(rel)
}

fn universe_foss_2024a() -> PathBuf {
    fixture("universe_foss_2024a")
}

fn foss(ver: &str) -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: ver.into(),
    }
}

fn deps(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Assert that bumping `source` with `params` reproduces `target` exactly,
/// except that `target` may contain each line in `allowed_additions` once,
/// in the position the maintainer inserted it. Removing those lines from
/// `target` (in order) must leave text identical to the emitted bump.
fn assert_reproduces(
    source: &Path,
    target: &Path,
    params: &EmitParams,
    allowed_additions: &[&str],
) {
    let result = emit_next_generation_from_path(source, params)
        .unwrap_or_else(|e| panic!("emit failed for {}: {e}", source.display()));
    assert_emitted_matches_target(&result.text, target, allowed_additions, source);
}

fn assert_reproduces_auto(
    source: &Path,
    target: &Path,
    toolchain: &Toolchain,
    universe: &Path,
    allowed_additions: &[&str],
) {
    let empty = HashMap::new();
    let result = emit_next_generation_auto_from_path(
        source,
        toolchain,
        universe,
        None,
        None,
        &empty,
        None,
        None,
    )
    .unwrap_or_else(|e| {
        panic!(
            "auto emit failed for {} with universe {}: {e}",
            source.display(),
            universe.display()
        )
    });
    assert_emitted_matches_target(&result.text, target, allowed_additions, source);
}

fn assert_emitted_matches_target(
    emitted_text: &str,
    target: &Path,
    allowed_additions: &[&str],
    source: &Path,
) {
    let target_text = std::fs::read_to_string(target)
        .unwrap_or_else(|e| panic!("read {}: {e}", target.display()));

    let mut target_lines: Vec<&str> = target_text.lines().collect();
    for addition in allowed_additions {
        let pos = target_lines.iter().position(|l| *l == *addition);
        match pos {
            Some(i) => {
                target_lines.remove(i);
            }
            None => panic!(
                "allowed addition {addition:?} not found in real target {}",
                target.display()
            ),
        }
    }
    let target_stripped = target_lines.join("\n");
    let emitted: Vec<&str> = emitted_text.lines().collect();
    let emitted_joined = emitted.join("\n");

    assert_eq!(
        emitted_joined.trim_end(),
        target_stripped.trim_end(),
        "mechanical bump of {} did not reproduce {} (modulo {:?})",
        source.display(),
        target.display(),
        allowed_additions
    );
}

/// GROMACS-2024.4: foss/2023b -> foss/2024a (hand dep map; historical).
///
/// Real maintainer PR bumps CMake, scikit-build-core (builddeps) and
/// Python, SciPy-bundle, networkx, mpi4py (deps), and hand-adds a new
/// `('pybind11', '2.12.0')` runtime dependency that no mechanical
/// toolchain-bump can invent.
#[test]
fn reproduces_gromacs_2024_4_foss_2023b_to_2024a() {
    let source = fixture("gromacs/GROMACS-2024.4-foss-2023b.eb");
    let target = fixture("gromacs/GROMACS-2024.4-foss-2024a.eb");
    let params = EmitParams {
        toolchain: foss("2024a"),
        version: None,
        source_checksum: None,
        dep_versions: deps(&[
            ("CMake", "3.29.3"),
            ("scikit-build-core", "0.11.1"),
            ("Python", "3.12.3"),
            ("SciPy-bundle", "2024.05"),
            ("networkx", "3.4.2"),
            ("mpi4py", "4.0.1"),
        ]),
    };
    assert_reproduces(
        &source,
        &target,
        &params,
        &["    ('pybind11', '2.12.0'),"],
    );
}

/// GROMACS-2024.4 auto-resolve: zero hand-fed dependency versions.
///
/// Universe holds real generation recipes at GCCcore/GCC/gfbf/gompi levels;
/// hierarchy-aware resolve fills every dep/builddep version. Residual is only
/// the maintainer-added `pybind11` line (not present in the source recipe).
#[test]
fn reproduces_gromacs_2024_4_foss_2023b_to_2024a_auto() {
    let source = fixture("gromacs/GROMACS-2024.4-foss-2023b.eb");
    let target = fixture("gromacs/GROMACS-2024.4-foss-2024a.eb");
    assert_reproduces_auto(
        &source,
        &target,
        &foss("2024a"),
        &universe_foss_2024a(),
        &["    ('pybind11', '2.12.0'),"],
    );
}

/// ScaFaCoS-1.0.4: foss/2023b -> foss/2024a.
///
/// Bumps Autotools, pkgconf (builddeps) and GSL (dep); GMP stays pinned.
/// No maintainer-added lines: the mechanical bump reproduces the real PR
/// with zero residual.
#[test]
fn reproduces_scafacos_1_0_4_foss_2023b_to_2024a() {
    let source = fixture("scafacos/ScaFaCoS-1.0.4-foss-2023b.eb");
    let target = fixture("scafacos/ScaFaCoS-1.0.4-foss-2024a.eb");
    let params = EmitParams {
        toolchain: foss("2024a"),
        version: None,
        source_checksum: None,
        dep_versions: deps(&[
            ("Autotools", "20231222"),
            ("pkgconf", "2.2.0"),
            ("GSL", "2.8"),
        ]),
    };
    assert_reproduces(&source, &target, &params, &[]);
}

#[test]
fn reproduces_scafacos_1_0_4_foss_2023b_to_2024a_auto() {
    let source = fixture("scafacos/ScaFaCoS-1.0.4-foss-2023b.eb");
    let target = fixture("scafacos/ScaFaCoS-1.0.4-foss-2024a.eb");
    assert_reproduces_auto(
        &source,
        &target,
        &foss("2024a"),
        &universe_foss_2024a(),
        &[],
    );
}

/// MDTraj-1.10.3: foss/2023b -> foss/2024a.
///
/// Bumps Python, SciPy-bundle, zlib, networkx, PyTables; netcdf4-python
/// stays pinned. Zero residual.
#[test]
fn reproduces_mdtraj_1_10_3_foss_2023b_to_2024a() {
    let source = fixture("mdtraj/MDTraj-1.10.3-foss-2023b.eb");
    let target = fixture("mdtraj/MDTraj-1.10.3-foss-2024a.eb");
    let params = EmitParams {
        toolchain: foss("2024a"),
        version: None,
        source_checksum: None,
        dep_versions: deps(&[
            ("Python", "3.12.3"),
            ("SciPy-bundle", "2024.05"),
            ("zlib", "1.3.1"),
            ("networkx", "3.4.2"),
            ("PyTables", "3.10.2"),
        ]),
    };
    assert_reproduces(&source, &target, &params, &[]);
}

#[test]
fn reproduces_mdtraj_1_10_3_foss_2023b_to_2024a_auto() {
    // networkx / PyTables are `# optional` in the source: optional marks a dep
    // optional to *include*, not frozen, so auto-resolve bumps them to the
    // generation version like any other dep, matching the maintainer PR
    // (networkx 3.4.2, PyTables 3.10.2). The `# optional` comment itself is
    // preserved verbatim; only the version token changes.
    let source = fixture("mdtraj/MDTraj-1.10.3-foss-2023b.eb");
    let empty = HashMap::new();
    let result = emit_next_generation_auto_from_path(
        &source,
        &foss("2024a"),
        &universe_foss_2024a(),
        None,
        None,
        &empty,
        None,
        None,
    )
    .expect("MDTraj auto emit");
    assert!(result.text.contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
    for pin in [
        "('Python', '3.12.3')",
        "('SciPy-bundle', '2024.05')",
        "('zlib', '1.3.1')",
    ] {
        assert!(
            result.text.contains(pin),
            "missing required pin {pin} in:\n{}",
            result.text
        );
    }
    // Optional deps bump to the generation version, matching the maintainer PR.
    assert!(
        result.text.contains("('networkx', '3.4.2'),  # optional"),
        "networkx must bump to 3.4.2 with comment preserved, got:\n{}",
        result.text
    );
    assert!(
        result.text.contains("('PyTables', '3.10.2'),  # optional"),
        "PyTables must bump to 3.10.2 with comment preserved, got:\n{}",
        result.text
    );
    assert!(!result.text.contains("('networkx', '3.2.1')"));
    assert!(!result.text.contains("('PyTables', '3.9.2')"));
}

/// Fiona-1.10.1: foss/2023b -> foss/2024a.
///
/// Bumps Python and a major-version jump on GDAL (3.9.0 -> 3.10.0);
/// Shapely stays pinned. Zero residual.
#[test]
fn reproduces_fiona_1_10_1_foss_2023b_to_2024a() {
    let source = fixture("fiona/Fiona-1.10.1-foss-2023b.eb");
    let target = fixture("fiona/Fiona-1.10.1-foss-2024a.eb");
    let params = EmitParams {
        toolchain: foss("2024a"),
        version: None,
        source_checksum: None,
        dep_versions: deps(&[("Python", "3.12.3"), ("GDAL", "3.10.0")]),
    };
    assert_reproduces(&source, &target, &params, &[]);
}

#[test]
fn reproduces_fiona_1_10_1_foss_2023b_to_2024a_auto() {
    let source = fixture("fiona/Fiona-1.10.1-foss-2023b.eb");
    let target = fixture("fiona/Fiona-1.10.1-foss-2024a.eb");
    assert_reproduces_auto(
        &source,
        &target,
        &foss("2024a"),
        &universe_foss_2024a(),
        &[],
    );
}

/// PuLP-2.8.0: foss/2023b -> foss/2024a.
///
/// Bumps Python and Cbc; GLPK stays pinned. Zero residual.
#[test]
fn reproduces_pulp_2_8_0_foss_2023b_to_2024a() {
    let source = fixture("pulp/PuLP-2.8.0-foss-2023b.eb");
    let target = fixture("pulp/PuLP-2.8.0-foss-2024a.eb");
    let params = EmitParams {
        toolchain: foss("2024a"),
        version: None,
        source_checksum: None,
        dep_versions: deps(&[("Python", "3.12.3"), ("Cbc", "2.10.12")]),
    };
    assert_reproduces(&source, &target, &params, &[]);
}

#[test]
fn reproduces_pulp_2_8_0_foss_2023b_to_2024a_auto() {
    let source = fixture("pulp/PuLP-2.8.0-foss-2023b.eb");
    let target = fixture("pulp/PuLP-2.8.0-foss-2024a.eb");
    assert_reproduces_auto(
        &source,
        &target,
        &foss("2024a"),
        &universe_foss_2024a(),
        &[],
    );
}

/// numba-0.60.0: foss/2023b -> foss/2024a.
///
/// Bumps Python and SciPy-bundle. Zero residual.
#[test]
fn reproduces_numba_0_60_0_foss_2023b_to_2024a() {
    let source = fixture("numba/numba-0.60.0-foss-2023b.eb");
    let target = fixture("numba/numba-0.60.0-foss-2024a.eb");
    let params = EmitParams {
        toolchain: foss("2024a"),
        version: None,
        source_checksum: None,
        dep_versions: deps(&[("Python", "3.12.3"), ("SciPy-bundle", "2024.05")]),
    };
    assert_reproduces(&source, &target, &params, &[]);
}

#[test]
fn reproduces_numba_0_60_0_foss_2023b_to_2024a_auto() {
    let source = fixture("numba/numba-0.60.0-foss-2023b.eb");
    let target = fixture("numba/numba-0.60.0-foss-2024a.eb");
    assert_reproduces_auto(
        &source,
        &target,
        &foss("2024a"),
        &universe_foss_2024a(),
        &[],
    );
}

// ---------------------------------------------------------------------------
// CLI path: the same known bumps an operator or the annual-bump skill runs
// via `eb-stack bump --easyconfigs …`. CI must exercise this surface, not
// only the library API, because flag wiring and exit status are the contract.
// ---------------------------------------------------------------------------

/// Run `eb-stack bump --easyconfigs` and return the written file contents.
fn cli_auto_bump(source: &Path, out_name: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join(out_name);
    let status = Command::new(bin)
        .args([
            "bump",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2024a",
            "--easyconfigs",
            universe_foss_2024a().to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn eb-stack bump");
    assert!(
        status.success(),
        "eb-stack bump failed for {}: {status}",
        source.display()
    );
    std::fs::read_to_string(&out).expect("read CLI output")
}

/// CLI auto-bump must match library emit (modulo residual additions).
fn assert_cli_reproduces(source: &Path, target: &Path, allowed_additions: &[&str], out_name: &str) {
    let emitted = cli_auto_bump(source, out_name);
    assert_emitted_matches_target(&emitted, target, allowed_additions, source);
}

#[test]
fn cli_reproduces_gromacs_2024_4_foss_2023b_to_2024a() {
    assert_cli_reproduces(
        &fixture("gromacs/GROMACS-2024.4-foss-2023b.eb"),
        &fixture("gromacs/GROMACS-2024.4-foss-2024a.eb"),
        &["    ('pybind11', '2.12.0'),"],
        "GROMACS-2024.4-foss-2024a.eb",
    );
}

#[test]
fn cli_reproduces_scafacos_1_0_4_foss_2023b_to_2024a() {
    assert_cli_reproduces(
        &fixture("scafacos/ScaFaCoS-1.0.4-foss-2023b.eb"),
        &fixture("scafacos/ScaFaCoS-1.0.4-foss-2024a.eb"),
        &[],
        "ScaFaCoS-1.0.4-foss-2024a.eb",
    );
}

#[test]
fn cli_reproduces_fiona_1_10_1_foss_2023b_to_2024a() {
    assert_cli_reproduces(
        &fixture("fiona/Fiona-1.10.1-foss-2023b.eb"),
        &fixture("fiona/Fiona-1.10.1-foss-2024a.eb"),
        &[],
        "Fiona-1.10.1-foss-2024a.eb",
    );
}

#[test]
fn cli_reproduces_pulp_2_8_0_foss_2023b_to_2024a() {
    assert_cli_reproduces(
        &fixture("pulp/PuLP-2.8.0-foss-2023b.eb"),
        &fixture("pulp/PuLP-2.8.0-foss-2024a.eb"),
        &[],
        "PuLP-2.8.0-foss-2024a.eb",
    );
}

#[test]
fn cli_reproduces_numba_0_60_0_foss_2023b_to_2024a() {
    assert_cli_reproduces(
        &fixture("numba/numba-0.60.0-foss-2023b.eb"),
        &fixture("numba/numba-0.60.0-foss-2024a.eb"),
        &[],
        "numba-0.60.0-foss-2024a.eb",
    );
}

#[test]
fn cli_reproduces_mdtraj_required_pins_and_optional_bumps() {
    // Same residual contract as the library MDTraj auto test: required deps
    // bump; `# optional` comment is preserved while the version token moves.
    let emitted = cli_auto_bump(
        &fixture("mdtraj/MDTraj-1.10.3-foss-2023b.eb"),
        "MDTraj-1.10.3-foss-2024a.eb",
    );
    assert!(emitted.contains("toolchain = {'name': 'foss', 'version': '2024a'}"));
    for pin in [
        "('Python', '3.12.3')",
        "('SciPy-bundle', '2024.05')",
        "('zlib', '1.3.1')",
        "('networkx', '3.4.2'),  # optional",
        "('PyTables', '3.10.2'),  # optional",
    ] {
        assert!(
            emitted.contains(pin),
            "CLI MDTraj missing {pin} in:\n{emitted}"
        );
    }
}
