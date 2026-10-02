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
    assert!(md.contains("baseline-from-easystack-foss-2025b"), "{md}");
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
        &tc,
        &members,
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
