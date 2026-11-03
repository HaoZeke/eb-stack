//! A foss solve warns when an LLVM-family easystack entry looks like site layer.

use std::path::PathBuf;
use std::process::Command;

fn easyconfigs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next/easyconfigs")
}

fn solve(stack: &str) -> std::process::Output {
    let tmp = tempfile::tempdir().unwrap();
    let easystack = tmp.path().join("baseline.yml");
    std::fs::write(&easystack, stack).unwrap();
    let lock = tmp.path().join("stack.lock.json");
    let diff = tmp.path().join("stack.diff.md");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(easyconfigs())
        .args(["--toolchain", "foss/2025b", "--root", "GROMACS"])
        .arg("--baseline-easystack")
        .arg(&easystack)
        .arg("--lock-out")
        .arg(&lock)
        .arg("--stack-diff-out")
        .arg(&diff)
        .output()
        .expect("run eb-stack");
    // Keep the temp dir alive until the command has finished reading it.
    drop(tmp);
    output
}

#[test]
fn a_foss_solve_warns_that_an_lfoss_easystack_entry_is_not_site_layer() {
    let output = solve("easyconfigs:\n  - GROMACS-2025.0-lfoss-2025b.eb\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "solve failed: {stderr}\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let solved = easyconfigs().join("foss-2025b/GROMACS-2025.0-foss-2025b.eb");
    let expected = format!(
        "warning: GROMACS (GROMACS-2025.0-lfoss-2025b.eb) is the LLVM family (lfoss). A foss solve cannot reach it, so the site layer includes GROMACS ({}). Keep one lock per toolchain family: solve lfoss on its own, or pass --baseline-also-toolchain lfoss/2025b.",
        solved.display()
    );
    assert!(
        stderr.contains(&expected),
        "stderr missing the LLVM warning:\n{stderr}\nexpected:\n{expected}"
    );
    assert!(
        stderr.contains("GROMACS-2025.0-lfoss-2025b.eb"),
        "the unresolved easystack entry is still reported: {stderr}"
    );
}

#[test]
fn a_version_bumped_lfoss_name_is_not_reported_as_site_layer() {
    let output = solve(
        "easyconfigs:\n  - GROMACS-2024.4-foss-2025b.eb\n  - OpenBLAS-0.3.32-lfoss-2025b.eb\n",
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "solve failed: {stderr}\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("OpenBLAS-0.3.32-lfoss-2025b.eb"),
        "an entry with no recipe is reported: {stderr}"
    );
    assert!(
        !stderr.contains("LLVM family"),
        "a version bump is not site layer: {stderr}"
    );
}
