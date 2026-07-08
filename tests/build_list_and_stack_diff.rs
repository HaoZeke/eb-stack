//! Integration tests: build-list order and markdown stack-diff via library + CLI.

use eb_stack::{
    classify_stack_diff, dep_map_from_universe, format_build_list, format_stack_diff_markdown,
    ordered_build_paths, solve_to_files_with_extras, PackageChangeKind, SolveExtraOut, StackLock,
    Universe,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
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

    let lock = solve_to_files_with_extras(
        &fixture("universe_next.json"),
        &fixture("policy_prefer_newer.json"),
        Some(&fixture("baseline.lock.json")),
        &lock_out,
        &sbom_out,
        SolveExtraOut {
            build_list_out: Some(&build_list_out),
            stack_diff_out: Some(&stack_diff_out),
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
        assert!(d_idx < g_idx, "{dep} must appear before GROMACS in build list");
    }

    let md = std::fs::read_to_string(&stack_diff_out).expect("read stack diff");
    assert!(!md.is_empty());
    for label in ["unchanged", "version-bumped", "GROMACS", "2024.1", "2025.0"] {
        assert!(md.contains(label), "stack diff missing {label:?}:\n{md}");
    }
    // Paths from both sides of the GROMACS bump.
    let baseline: StackLock = load_json("baseline.lock.json");
    let base_g = baseline.package("GROMACS").unwrap();
    let sol_g = lock.package("GROMACS").unwrap();
    assert!(md.contains(&base_g.easyconfig_path), "{md}");
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
fn cli_solve_json_writes_both_artifacts() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_out = tmp.path().join("lock.json");
    let sbom_out = tmp.path().join("sbom.json");
    let build_list_out = tmp.path().join("build.list");
    let stack_diff_out = tmp.path().join("diff.md");

    let run = |dest: &Path| {
        let status = Command::new(bin)
            .args([
                "solve-json",
                "--universe",
                fixture("universe_next.json").to_str().unwrap(),
                "--policy",
                fixture("policy_prefer_newer.json").to_str().unwrap(),
                "--baseline",
                fixture("baseline.lock.json").to_str().unwrap(),
                "--lock-out",
                lock_out.to_str().unwrap(),
                "--sbom-out",
                sbom_out.to_str().unwrap(),
                "--build-list-out",
                dest.join("build.list").to_str().unwrap(),
                "--stack-diff-out",
                dest.join("diff.md").to_str().unwrap(),
            ])
            .status()
            .expect("spawn eb-stack");
        assert!(status.success(), "eb-stack solve-json failed: {status}");
    };

    // Double launch: both runs must produce non-empty artifacts (no flaky empty outputs).
    let run1 = tmp.path().join("run1");
    let run2 = tmp.path().join("run2");
    std::fs::create_dir_all(&run1).unwrap();
    std::fs::create_dir_all(&run2).unwrap();
    run(&run1);
    run(&run2);

    for dir in [&run1, &run2] {
        let bl = std::fs::read_to_string(dir.join("build.list")).unwrap();
        let md = std::fs::read_to_string(dir.join("diff.md")).unwrap();
        assert!(!bl.trim().is_empty(), "empty build list in {}", dir.display());
        assert!(!md.trim().is_empty(), "empty stack diff in {}", dir.display());
        assert!(bl.lines().any(|l| l.contains("GROMACS")), "{bl}");
        assert!(
            md.contains("version-bumped") || md.contains("unchanged"),
            "{md}"
        );
    }

    // Same ordered paths on both runs.
    let a = std::fs::read_to_string(run1.join("build.list")).unwrap();
    let b = std::fs::read_to_string(run2.join("build.list")).unwrap();
    assert_eq!(a, b);
}

#[test]
fn cli_solve_without_extras_still_writes_lock_and_sbom() {
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let tmp = tempfile::tempdir().expect("tempdir");
    let lock_out = tmp.path().join("stack.lock.json");
    let sbom_out = tmp.path().join("stack.cdx.json");
    let status = Command::new(bin)
        .args([
            "solve",
            "--easyconfigs",
            fixture("easyconfigs").to_str().unwrap(),
            "--policy",
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
    // No default extra artifacts when flags omitted.
    assert!(!tmp.path().join("build.list").exists());
}
