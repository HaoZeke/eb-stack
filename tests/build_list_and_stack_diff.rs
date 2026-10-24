//! Integration tests: build-list order and markdown stack-diff via library + CLI.

use eb_stack::{
    classify_stack_diff, dep_map_from_universe, format_build_list, format_stack_diff_markdown,
    ordered_build_paths, solve_from_easyconfigs_with_baseline_version_and_extras,
    PackageChangeKind, SolveExtraOut, StackLock, Universe,
};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/gromacs_2025_to_next")
        .join(rel)
}

fn load_json<T: serde::de::DeserializeOwned>(rel: &str) -> T {
    serde_json::from_str(&std::fs::read_to_string(fixture(rel)).unwrap()).unwrap()
}

#[test]
fn library_solve_writes_build_list_and_stack_diff() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_out = tmp.path().join("stack.lock.json");
    let sbom_out = tmp.path().join("stack.cdx.json");
    let build_list_out = tmp.path().join("build.list");
    let stack_diff_out = tmp.path().join("stack.diff.md");

    let easyconfigs = fixture("easyconfigs");
    let lock = solve_from_easyconfigs_with_baseline_version_and_extras(
        &[easyconfigs.as_path()],
        &fixture("policy_prefer_newer.json"),
        Some(&easyconfigs),
        Some("2025a"),
        &lock_out,
        Some(&sbom_out),
        SolveExtraOut {
            build_list_out: Some(&build_list_out),
            stack_diff_out: Some(&stack_diff_out),
            ..Default::default()
        },
    )
    .expect("solve");

    assert!(lock_out.is_file());
    assert!(sbom_out.is_file());

    let bl = std::fs::read_to_string(&build_list_out).expect("read build list");
    assert!(!bl.is_empty(), "build list must be non-empty");
    let lines: Vec<&str> = bl.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), lock.packages.len());
    for p in &lock.packages {
        assert!(
            lines.iter().any(|l| *l == p.easyconfig_path),
            "missing path {} in\n{bl}",
            p.easyconfig_path
        );
    }
    let g_path = &lock.package("GROMACS").unwrap().easyconfig_path;
    let g_idx = lines.iter().position(|l| *l == g_path).unwrap();
    for dep in ["OpenBLAS", "OpenMPI", "FFTW"] {
        let d_path = &lock.package(dep).unwrap().easyconfig_path;
        let d_idx = lines.iter().position(|l| *l == d_path).unwrap();
        assert!(
            d_idx < g_idx,
            "{dep} must appear before GROMACS in build list"
        );
    }

    let md = std::fs::read_to_string(&stack_diff_out).expect("read stack diff");
    assert!(!md.is_empty());
    for label in ["unchanged", "version-bumped", "GROMACS", "2024.1", "2025.0"] {
        assert!(md.contains(label), "stack diff missing {label:?}:\n{md}");
    }
    // Paths from both sides of the GROMACS bump.
    let baseline_g = easyconfigs
        .join("foss-2025a/GROMACS-2024.1-foss-2025a.eb")
        .display()
        .to_string();
    let sol_g = lock.package("GROMACS").unwrap();
    assert!(md.contains(&baseline_g), "{md}");
    assert!(md.contains(&sol_g.easyconfig_path), "{md}");
}

#[test]
fn pure_formatters_match_shipped_solve_outputs() {
    let universe: Universe = load_json("universe_next.json");
    let baseline: StackLock = load_json("baseline.lock.json");
    let policy = load_json("policy_prefer_newer.json");
    let lock = eb_stack::select_stack(&universe, &policy, Some(&baseline)).unwrap();
    let dep_map = dep_map_from_universe(&lock, &universe);

    let paths = ordered_build_paths(&lock, &dep_map);
    let mut unique = BTreeSet::new();
    for p in &paths {
        assert!(unique.insert(p.as_str()), "duplicate path {p}");
    }
    assert_eq!(paths.len(), lock.packages.len());

    let text = format_build_list(&lock, &dep_map);
    assert_eq!(text, paths.join("\n") + "\n");

    let changes = classify_stack_diff(&baseline, &lock);
    assert!(changes
        .iter()
        .any(|c| c.kind == PackageChangeKind::Unchanged && c.name == "FFTW"));
    assert!(changes
        .iter()
        .any(|c| c.kind == PackageChangeKind::VersionBumped && c.name == "GROMACS"));

    let md = format_stack_diff_markdown(&baseline, &lock);
    assert!(md.starts_with("# Stack diff\n"));
    assert!(md.contains("**version-bumped**:"));
}

#[test]
fn cli_solve_json_legacy_surface_is_removed() {
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["solve-json", "--help"])
        .output()
        .expect("spawn eb-stack");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_solve_without_sbom_flag_writes_lock_only() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_out = tmp.path().join("stack.lock.json");
    // Intentionally omit --sbom-out: core solve must not default-write an SBOM.
    let status = Command::new(bin)
        .current_dir(tmp.path())
        .args([
            "stack",
            "solve",
            "--easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--stack-policy",
            fixture("policies/prefer_newer.json").to_str().unwrap(),
            "--baseline-easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--lock-out",
            lock_out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(status.success());
    assert!(lock_out.is_file());
    // No default SBOM filename (neither cwd default nor lock-sibling).
    assert!(
        !tmp.path().join("stack.cdx.json").exists(),
        "solve must not write stack.cdx.json without --sbom-out"
    );
    assert!(!tmp.path().join("build.list").exists());
    let names: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|n| n.ends_with(".cdx.json")),
        "unexpected SBOM files without --sbom-out: {names:?}"
    );
}

#[test]
fn cli_solve_with_sbom_flag_writes_sbom() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_out = tmp.path().join("stack.lock.json");
    let sbom_out = tmp.path().join("stack.cdx.json");
    let status = Command::new(bin)
        .args([
            "stack",
            "solve",
            "--easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--stack-policy",
            fixture("policies/prefer_newer.json").to_str().unwrap(),
            "--baseline-easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--lock-out",
            lock_out.to_str().unwrap(),
            "--sbom-out",
            sbom_out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(status.success());
    assert!(lock_out.is_file());
    assert!(sbom_out.is_file());
}

/// End-to-end CLI: `stack solve --baseline-easyconfigs` asserts lock *content* and
/// stack-diff text for the known prefer_newer upgrade outcome (not mere file existence).
#[test]
fn cli_solve_baseline_asserts_lock_versions_and_stack_diff() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_out = tmp.path().join("stack.lock.json");
    let stack_diff_out = tmp.path().join("stack.diff.md");
    let status = Command::new(bin)
        .args([
            "stack",
            "solve",
            "--easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--stack-policy",
            fixture("policies/prefer_newer.json").to_str().unwrap(),
            "--baseline-easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--lock-out",
            lock_out.to_str().unwrap(),
            "--stack-diff-out",
            stack_diff_out.to_str().unwrap(),
        ])
        .status()
        .expect("spawn eb-stack stack solve");
    assert!(status.success(), "eb-stack stack solve failed: {status}");
    assert!(lock_out.is_file());
    assert!(stack_diff_out.is_file());

    let lock: StackLock =
        serde_json::from_str(&std::fs::read_to_string(&lock_out).unwrap()).expect("lock json");
    assert_eq!(lock.toolchain.name, "foss");
    assert_eq!(lock.toolchain.version, "2025b");
    assert_eq!(lock.solver.engine, "resolvo_cdcl_sat");
    // prefer_newer + require_upgrade from 2025a baseline → documented stack.
    assert_eq!(lock.package("GROMACS").unwrap().version, "2025.0");
    assert_eq!(lock.package("OpenBLAS").unwrap().version, "0.3.27");
    assert_eq!(lock.package("OpenMPI").unwrap().version, "5.0.3");
    assert_eq!(lock.package("FFTW").unwrap().version, "3.3.10");
    assert_eq!(lock.package("Python").unwrap().version, "3.12.3");

    let md = std::fs::read_to_string(&stack_diff_out).expect("stack diff");
    assert!(
        md.contains("version-bumped") || md.contains("**version-bumped**"),
        "{md}"
    );
    assert!(md.contains("GROMACS"), "diff must mention GROMACS:\n{md}");
    assert!(
        md.contains("2024.1"),
        "diff must show baseline GROMACS 2024.1:\n{md}"
    );
    assert!(
        md.contains("2025.0"),
        "diff must show selected GROMACS 2025.0:\n{md}"
    );
    assert!(md.contains("OpenBLAS"), "{md}");
}

/// A stack diff with no baseline must name flags the CLI actually accepts.
#[test]
fn stack_diff_without_a_baseline_names_the_real_flags() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // prefer_newer.json demands a baseline via require_upgrade, which fails
    // before the diff is written. This policy solves, then the diff refuses.
    let policy = tmp.path().join("policy.json");
    std::fs::write(
        &policy,
        r#"{"toolchain":{"name":"foss","version":"2025b"},"roots":["GROMACS"],"objective":"prefer_newer"}"#,
    )
    .expect("policy");
    let err = eb_stack::solve_from_easyconfigs_with_extras(
        &[fixture("easyconfigs").as_path()],
        &policy,
        None,
        &tmp.path().join("lock.json"),
        None,
        SolveExtraOut {
            stack_diff_out: Some(&tmp.path().join("diff.md")),
            ..Default::default()
        },
    )
    .expect_err("no baseline");
    let msg = err.to_string();
    assert!(
        msg.contains("--baseline-easyconfigs"),
        "must name --baseline-easyconfigs: {msg}"
    );
    assert!(
        msg.contains("--baseline-easystack"),
        "must name --baseline-easystack: {msg}"
    );
    assert!(
        !msg.contains("--baseline /"),
        "must not name a flag that is not accepted: {msg}"
    );
}

/// The how-to prints the build list and stack diff of its own command.
/// A shortened sample drifts the first time someone compares a real run.
#[test]
fn emit_reports_howto_matches_the_gromacs_solve() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().expect("tempdir");
    let build_list = tmp.path().join("build.list");
    let stack_diff = tmp.path().join("stack.diff.md");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(&root)
        .args([
            "stack",
            "solve",
            "--easyconfigs",
            "fixtures/gromacs_2025_to_next/easyconfigs",
            "--stack-policy",
            "fixtures/gromacs_2025_to_next/policies/prefer_newer.json",
            "--baseline-easyconfigs",
            "fixtures/gromacs_2025_to_next/easyconfigs",
            "--lock-out",
        ])
        .arg(tmp.path().join("stack.lock.json"))
        .arg("--sbom-out")
        .arg(tmp.path().join("stack.cdx.json"))
        .arg("--build-list-out")
        .arg(&build_list)
        .arg("--stack-diff-out")
        .arg(&stack_diff)
        .status()
        .expect("spawn");
    assert!(status.success());
    let list = std::fs::read_to_string(&build_list).expect("build list");
    let diff = std::fs::read_to_string(&stack_diff).expect("stack diff");
    assert!(
        diff.starts_with("# Stack diff\n\nBaseline (`foss-2025a`) to solved ("),
        "{diff}"
    );
    let howto =
        std::fs::read_to_string(root.join("docs/orgmode/howto/emit-reports.org")).expect("howto");
    assert!(
        howto.contains(list.trim_end()),
        "emit-reports.org build list drifted from the solve:\n{list}"
    );
    assert!(
        howto.contains(diff.trim_end()),
        "emit-reports.org stack diff drifted from the solve:\n{diff}"
    );
}
