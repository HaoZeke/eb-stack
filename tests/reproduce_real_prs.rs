//! Mechanical reproduction suite: prove `eb-stack`'s toolchain-bump emit
//! reproduces real EasyBuild maintainer PRs byte-for-byte (modulo any
//! genuinely-new dependency lines the maintainer hand-added, which no
//! mechanical bump can invent).
//!
//! Each fixture pair is a real `(source, target)` easyconfig lifted from a
//! local EasyBuild checkout: same application version, adjacent `foss`
//! toolchain generation (`2023b` -> `2024a`). The library is driven with
//! exactly the toolchain + per-dependency version map that the diff between
//! source and target actually shows; the emitted text is then compared
//! against the real target text line-for-line.
//!
//! A pair only belongs here if it reproduces cleanly: every allowed
//! addition below was verified (before this file was committed) to be the
//! *entire* residual between the mechanical bump and the real PR.

use eb_stack::{emit_next_generation_from_path, EmitParams, Toolchain};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/repro_fixtures")
        .join(rel)
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
    let emitted: Vec<&str> = result.text.lines().collect();
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

/// GROMACS-2024.4: foss/2023b -> foss/2024a.
///
/// Real maintainer PR bumps CMake, scikit-build-core (builddeps) and
/// Python, SciPy-bundle, networkx, mpi4py (deps), and hand-adds a new
/// `('pybind11', '2.12.0')` runtime dependency that no mechanical
/// toolchain-bump can invent. This is the proven exemplar for the method.
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
