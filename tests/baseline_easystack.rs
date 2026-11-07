//! A baseline read from easystacks: what a software layer already installs.

use eb_stack::{classify_stack_diff, PackageChangeKind, StackLock};
use std::path::{Path, PathBuf};
use std::process::Command;

fn easyconfigs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next/easyconfigs")
}

fn kinds(base: &StackLock, solved: &StackLock) -> Vec<(String, PackageChangeKind)> {
    classify_stack_diff(base, solved)
        .into_iter()
        .map(|c| (c.name, c.kind))
        .collect()
}

fn solve(dir: &Path, extra: &[&str]) -> (std::process::Output, StackLock, StackLock) {
    let lock = dir.join("stack.lock.json");
    let diff = dir.join("stack.diff.md");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    cmd.args(["stack", "solve", "--easyconfigs"])
        .arg(easyconfigs())
        .args([
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--lock-out",
        ])
        .arg(&lock)
        .arg("--stack-diff-out")
        .arg(&diff)
        .args(extra);
    let out = cmd.output().expect("run eb-stack");
    assert!(
        out.status.success(),
        "solve failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let solved: StackLock = serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
    // The diff names the baseline's versions; rebuild the baseline lock from the
    // library so the comparison is on data, not on markdown.
    let md = std::fs::read_to_string(&diff).unwrap();
    assert!(
        md.contains("Baseline (`foss-2025b`) to solved (")
            && !md.contains("baseline-from-easystack"),
        "{md}"
    );
    let stacks: Vec<PathBuf> = extra
        .windows(2)
        .filter(|w| w[0] == "--baseline-easystack")
        .map(|w| PathBuf::from(w[1]))
        .collect();
    let stack_refs: Vec<&Path> = stacks.iter().map(PathBuf::as_path).collect();
    let repo = extra
        .windows(2)
        .find(|w| w[0] == "--baseline-commits-from")
        .map(|w| PathBuf::from(w[1]));
    let tree = eb_stack::parse_easyconfig_trees(&[easyconfigs().as_path()]).unwrap();
    let tc = eb_stack::Toolchain {
        name: "foss".into(),
        version: "2025b".into(),
    };
    let members = eb_stack::hierarchy::hierarchy_for_with_tree(&tc, None, &tree.candidates)
        .map(|h| h.members)
        .unwrap_or_default();
    let (baseline, _) = eb_stack::baseline_from_easystacks(
        &stack_refs,
        &tree.candidates,
        repo.as_deref(),
        &[(tc, members)],
    )
    .unwrap();
    (out, baseline, solved)
}

#[test]
fn an_easystack_baseline_is_its_entries_and_their_pinned_closure() {
    let tmp = tempfile::tempdir().unwrap();
    let stack = tmp.path().join("eessi.yml");
    std::fs::write(
        &stack,
        "easyconfigs:\n  - GROMACS-2024.4-foss-2025b.eb\n  - Missing-1.0-foss-2025b.eb\n",
    )
    .unwrap();
    let (out, baseline, solved) = solve(
        tmp.path(),
        &["--baseline-easystack", stack.to_str().unwrap()],
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Missing-1.0-foss-2025b.eb"),
        "an entry with no recipe is reported: {stderr}"
    );
    let base: Vec<(String, String)> = baseline
        .packages
        .iter()
        .map(|p| (p.name.clone(), p.version.clone()))
        .collect();
    assert_eq!(
        base,
        [
            ("FFTW", "3.3.10"),
            ("GROMACS", "2024.4"),
            ("OpenBLAS", "0.3.24"),
            ("OpenMPI", "4.1.6"),
            ("Python", "3.12.3"),
        ]
        .map(|(n, v)| (n.to_string(), v.to_string()))
    );
    let k = kinds(&baseline, &solved);
    for (name, kind) in [
        ("FFTW", PackageChangeKind::Unchanged),
        ("Python", PackageChangeKind::Unchanged),
        ("GROMACS", PackageChangeKind::VersionBumped),
        ("OpenBLAS", PackageChangeKind::VersionBumped),
    ] {
        assert!(
            k.contains(&(name.to_string(), kind)),
            "{name} should be {kind:?}: {k:?}"
        );
    }
}

#[test]
fn a_from_commit_entry_is_read_from_its_commit_not_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("easyconfigs-repo");
    let dir = repo.join("easybuild/easyconfigs/g/GROMACS");
    std::fs::create_dir_all(&dir).unwrap();
    // The tree's GROMACS-2024.4 pins OpenBLAS 0.3.24; the commit's pins 0.3.23.
    let text =
        std::fs::read_to_string(easyconfigs().join("foss-2025b/GROMACS-2024.4-foss-2025b.eb"))
            .unwrap()
            .replace("('OpenBLAS', '0.3.24')", "('OpenBLAS', '0.3.23')");
    std::fs::write(dir.join("GROMACS-2024.4-foss-2025b.eb"), text).unwrap();
    let git = |args: &[&str]| {
        let st = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "fixture"]);
    let sha = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let stack = tmp.path().join("eessi.yml");
    std::fs::write(
        &stack,
        format!("easyconfigs:\n  - GROMACS-2024.4-foss-2025b.eb:\n      options:\n        from-commit: {sha}\n"),
    )
    .unwrap();
    let (_, baseline, _) = solve(
        tmp.path(),
        &[
            "--baseline-easystack",
            stack.to_str().unwrap(),
            "--baseline-commits-from",
            repo.to_str().unwrap(),
        ],
    );
    let openblas = baseline.package("OpenBLAS").expect("OpenBLAS in baseline");
    assert_eq!(openblas.version, "0.3.23");
}

#[test]
fn the_build_list_can_leave_out_what_the_baseline_provides() {
    let tmp = tempfile::tempdir().unwrap();
    let stack = tmp.path().join("eessi.yml");
    std::fs::write(&stack, "easyconfigs:\n  - GROMACS-2024.4-foss-2025b.eb\n").unwrap();
    let full = tmp.path().join("full.list");
    let site = tmp.path().join("site.list");
    for (out, exclude) in [(&full, false), (&site, true)] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        cmd.args(["stack", "solve", "--easyconfigs"])
            .arg(easyconfigs())
            .args([
                "--toolchain",
                "foss/2025b",
                "--root",
                "GROMACS",
                "--lock-out",
            ])
            .arg(tmp.path().join("stack.lock.json"))
            .arg("--baseline-easystack")
            .arg(&stack)
            .arg("--build-list-out")
            .arg(out);
        if exclude {
            cmd.arg("--build-list-excludes-baseline");
        }
        let st = cmd.output().unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );
    }
    let names = |p: &Path| -> Vec<String> {
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .map(|l| l.rsplit('/').next().unwrap().to_string())
            .collect()
    };
    let full = names(&full);
    let site = names(&site);
    // FFTW 3.3.10 and Python 3.12.3 are the same modules the baseline closure
    // holds; GROMACS, OpenBLAS and OpenMPI move to versions it does not.
    for provided in ["FFTW-3.3.10-foss-2025b.eb", "Python-3.12.3-foss-2025b.eb"] {
        assert!(full.contains(&provided.to_string()), "{full:?}");
        assert!(!site.contains(&provided.to_string()), "{site:?}");
    }
    for built in [
        "GROMACS-2025.0-foss-2025b.eb",
        "OpenBLAS-0.3.27-foss-2025b.eb",
    ] {
        assert!(site.contains(&built.to_string()), "{site:?}");
    }
    assert_eq!(site.len() + 2, full.len(), "full {full:?} site {site:?}");
}

fn one(name: &str, tc: &str) -> eb_stack::Candidate {
    eb_stack::Candidate {
        easyblock: None,
        name: name.into(),
        version: "1.0".into(),
        toolchain: eb_stack::Toolchain {
            name: tc.into(),
            version: "2026.1".into(),
        },
        versionsuffix: None,
        versionprefix: None,
        easyconfig_path: format!("{name}-1.0-{tc}-2026.1.eb"),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
        moduleclass: None,
    }
}

#[test]
fn a_second_family_stays_in_the_baseline_only_when_named() {
    let tmp = tempfile::tempdir().unwrap();
    let stack = tmp.path().join("eessi.yml");
    std::fs::write(
        &stack,
        "easyconfigs:\n  - A-1.0-foss-2026.1.eb\n  - B-1.0-lfoss-2026.1.eb\n",
    )
    .unwrap();
    let cands = [one("A", "foss"), one("B", "lfoss")];
    let tc = |n: &str| eb_stack::Toolchain {
        name: n.into(),
        version: "2026.1".into(),
    };
    let names = |families: &[(eb_stack::Toolchain, Vec<eb_stack::Toolchain>)]| -> Vec<String> {
        let (lock, unresolved) =
            eb_stack::baseline_from_easystacks(&[stack.as_path()], &cands, None, families).unwrap();
        assert!(unresolved.is_empty(), "{unresolved:?}");
        lock.packages.into_iter().map(|p| p.name).collect()
    };
    assert_eq!(names(&[(tc("foss"), Vec::new())]), ["A"]);
    assert_eq!(
        names(&[(tc("foss"), Vec::new()), (tc("lfoss"), Vec::new())]),
        ["A", "B"]
    );
}

#[test]
fn a_filtered_dependency_is_neither_solved_nor_built() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = tmp.path().join("stack.lock.json");
    let list = tmp.path().join("build.list");
    let st = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(easyconfigs())
        .args([
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--filter-deps",
            "FFTW,Python",
        ])
        .arg("--lock-out")
        .arg(&lock)
        .arg("--build-list-out")
        .arg(&list)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let solved: StackLock = serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
    for gone in ["FFTW", "Python"] {
        assert!(solved.package(gone).is_none(), "{gone} filtered but locked");
    }
    assert!(solved.package("GROMACS").is_some());
    assert!(!std::fs::read_to_string(&list).unwrap().contains("FFTW"));
}

fn write_easyconfig(dir: &std::path::Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A root that lives on a subtoolchain of foss/2026.1 is a candidate. The
/// hierarchy walk (GCCcore, GCC, gfbf, gompi, foss) is what finds it.
#[test]
fn a_root_on_gcccore_resolves_for_foss_2026_1() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("easyconfigs");
    write_easyconfig(
        &tree,
        "c/CMake/CMake-3.31.11-GCCcore-15.2.0.eb",
        "name = 'CMake'\nversion = '3.31.11'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\nhomepage = 'https://cmake.org'\ndescription = 'fixture'\n",
    );
    let lock = tmp.path().join("stack.lock.json");
    let st = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(&tree)
        .args([
            "--toolchain",
            "foss/2026.1",
            "--root",
            "CMake",
            "--lock-out",
        ])
        .arg(&lock)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let solved: StackLock = serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
    assert_eq!(solved.package("CMake").unwrap().version, "3.31.11");
}

/// No GROMACS easyconfig on foss/2026.1 is a missing recipe, and the error
/// names the generations that do have one.
#[test]
fn a_root_missing_from_the_generation_names_the_nearest_easyconfig() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("easyconfigs");
    write_easyconfig(
        &tree,
        "f/FFTW/FFTW-3.3.10-foss-2026.1.eb",
        "name = 'FFTW'\nversion = '3.3.10'\ntoolchain = {'name': 'foss', 'version': '2026.1'}\nhomepage = 'https://www.fftw.org'\ndescription = 'fixture'\n",
    );
    write_easyconfig(
        &tree,
        "g/GROMACS/GROMACS-2026.2-foss-2025b.eb",
        "name = 'GROMACS'\nversion = '2026.2'\ntoolchain = {'name': 'foss', 'version': '2025b'}\nhomepage = 'https://www.gromacs.org'\ndescription = 'fixture'\n",
    );
    write_easyconfig(
        &tree,
        "g/GROMACS/GROMACS-2026.2-foss-2025b-CUDA-12.9.1.eb",
        "name = 'GROMACS'\nversion = '2026.2'\nversionsuffix = '-CUDA-12.9.1'\ntoolchain = {'name': 'foss', 'version': '2025b'}\nhomepage = 'https://www.gromacs.org'\ndescription = 'fixture'\n",
    );
    write_easyconfig(
        &tree,
        "g/GROMACS/GROMACS-2024.4-foss-2024a.eb",
        "name = 'GROMACS'\nversion = '2024.4'\ntoolchain = {'name': 'foss', 'version': '2024a'}\nhomepage = 'https://www.gromacs.org'\ndescription = 'fixture'\n",
    );
    let st = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(&tree)
        .args([
            "--toolchain",
            "foss/2026.1",
            "--root",
            "GROMACS",
            "--lock-out",
        ])
        .arg(tmp.path().join("stack.lock.json"))
        .output()
        .unwrap();
    assert!(!st.status.success());
    let err = String::from_utf8_lossy(&st.stderr);
    assert!(
        err.contains("no candidates for root package GROMACS on foss/2026.1"),
        "{err}"
    );
    assert!(err.contains("GCCcore-15.2.0"), "{err}");
    assert!(err.contains("gompi-2026.1"), "{err}");
    assert!(
        err.contains("Nearest: GROMACS-2026.2-foss-2025b.eb on foss/2025b"),
        "{err}"
    );
    assert!(err.contains("--toolchain foss/2025b"), "{err}");
    assert!(!err.contains("impi"), "{err}");
}
