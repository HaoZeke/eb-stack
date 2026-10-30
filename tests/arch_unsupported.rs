//! Per-architecture unsupported entries, seeded from EESSI software-layer bot failures.
//!
//! The easystack is shaped like pull request 1775: GDRCopy is already in the
//! shared file, and the pull request adds the other lines. The riscv64 bot
//! failed on GDRCopy.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/arch_unsupported_fixtures")
        .join(name)
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_shared_plan_that_includes_gdrcopy_fails_on_riscv64() {
    let easystack = fixture("easystacks/eessi-2026.06-eb-5.4.0-2026.1.yml");
    let output = run(&[
        "stack",
        "arch",
        "--arch",
        "riscv64",
        "--easystack",
        easystack.to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
        "--installed",
        fixture("installed-riscv64.txt").to_str().unwrap(),
        "--additional-arch",
        fixture("additional_arch/riscv64/generic/eessi-2026.06-eb-5.4.0-2026.1.yml")
            .to_str()
            .unwrap(),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert!(
        !output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let unsupported = "GDRCopy-2.5.2-GCCcore-15.2.0.eb is unsupported on riscv64: needs CUDA; gdrconfig.h errors with architecture is not supported (software-layer #1775, #1822)";
    let flagged = "GDRCopy-2.5.2-GCCcore-15.2.0.eb is flagged on riscv64: it has never been installed there and is not in the riscv64 additional_arch easystack";
    assert!(stdout.contains(unsupported), "{stdout}");
    assert!(stdout.contains(flagged), "{stdout}");
    assert!(
        stdout.contains("parse_failed=0 unsupported=1 flagged=1\n"),
        "{stdout}"
    );
    assert!(stderr.contains(unsupported), "{stderr}");
    assert!(!stdout.contains("required by"), "{stdout}");
}

#[test]
fn a_plan_that_only_depends_on_gdrcopy_names_the_depender() {
    let output = run(&[
        "stack",
        "arch",
        "--arch",
        "riscv64",
        "--easystack",
        fixture("easystacks/nvtop-only.yml").to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("GDRCopy-2.5.2-GCCcore-15.2.0.eb is unsupported on riscv64:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("(required by nvtop-3.3.2-GCCcore-15.2.0.eb)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("parse_failed=0 unsupported=1 flagged=0\n"),
        "{stdout}"
    );
}

#[test]
fn bwa_mem2_fails_on_aarch64_and_zlib_does_not() {
    let bwa = run(&[
        "stack",
        "arch",
        "--arch",
        "aarch64",
        "--easystack",
        fixture("easystacks/bwa-mem2.yml").to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
    ]);
    let stdout = text(&bwa.stdout);
    assert_eq!(
        bwa.status.code(),
        Some(1),
        "{stdout}\n{}",
        text(&bwa.stderr)
    );
    assert!(
        stdout.contains("bwa-mem2-2.3-GCC-14.3.0.eb is unsupported on aarch64:"),
        "{stdout}"
    );
    assert!(stdout.contains("-msse"), "{stdout}");
    assert!(stdout.contains("#1876"), "{stdout}");

    let zlib = run(&[
        "stack",
        "arch",
        "--arch",
        "riscv64",
        "--easystack",
        fixture("easystacks/zlib.yml").to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
        "--installed",
        fixture("installed-riscv64.txt").to_str().unwrap(),
        "--additional-arch",
        fixture("additional_arch/riscv64/generic").to_str().unwrap(),
    ]);
    let stdout = text(&zlib.stdout);
    assert!(
        zlib.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        text(&zlib.stderr)
    );
    assert!(
        stdout.contains("parse_failed=0 unsupported=0 flagged=0\n"),
        "{stdout}"
    );
}

#[test]
fn an_additional_arch_entry_is_not_flagged_and_x86_64_can_plan_gdrcopy() {
    let output = run(&[
        "stack",
        "arch",
        "--arch",
        "riscv64",
        "--easystack",
        fixture("easystacks/eessi-2026.06-eb-5.4.0-2026.1.yml")
            .to_str()
            .unwrap(),
        "--installed",
        fixture("installed-riscv64.txt").to_str().unwrap(),
        "--additional-arch",
        fixture("additional_arch/riscv64/generic/gdrcopy.yml")
            .to_str()
            .unwrap(),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(stdout.contains("is unsupported on riscv64:"), "{stdout}");
    assert!(
        !stdout.contains("is flagged on"),
        "additional_arch suppresses the cheap check:\n{stdout}"
    );
    assert!(
        stdout.contains("parse_failed=0 unsupported=1 flagged=0\n"),
        "{stdout}"
    );

    let x86 = run(&[
        "stack",
        "arch",
        "--arch",
        "x86_64",
        "--easystack",
        fixture("easystacks/eessi-2026.06-eb-5.4.0-2026.1.yml")
            .to_str()
            .unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
        "--installed",
        fixture("installed-x86_64.txt").to_str().unwrap(),
    ]);
    let stdout = text(&x86.stdout);
    assert!(
        x86.status.success(),
        "stdout:\n{stdout}\nstderr:\n{}",
        text(&x86.stderr)
    );
    assert!(
        stdout.contains("parse_failed=0 unsupported=0 flagged=0\n"),
        "{stdout}"
    );
}

#[test]
fn a_missing_easystack_is_a_usage_error() {
    let output = run(&["stack", "arch"]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--easystack"), "{stderr}");
}

#[test]
fn go_local_archs_missing_riscv64_is_reported_before_the_build() {
    let output = run(&[
        "stack",
        "arch",
        "--arch",
        "riscv64",
        "--easystack",
        fixture("easystacks/go-1.26.7.yml").to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("Go-1.26.7.eb failed to parse on riscv64:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("parse_failed=1 unsupported=0 flagged=0\n"),
        "{stdout}"
    );
    assert!(
        stderr.contains("Go-1.26.7.eb failed to parse on riscv64:"),
        "{stderr}"
    );
}

#[test]
fn stack_batch_refuses_a_plan_that_depends_on_an_unsupported_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let nvtop = run(&[
        "stack",
        "batch",
        "--root",
        "nvtop-3.3.2-GCCcore-15.2.0.eb",
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
        "--installed",
        fixture("installed-empty.txt").to_str().unwrap(),
        "--eessi-version",
        "2026.06",
        "--easybuild-version",
        "5.4.0",
        "--generation",
        "2026.1",
        "--generated-at",
        "2026-10-29T22:12:00Z",
        "--out-dir",
        dir.path().to_str().unwrap(),
    ]);
    let stderr = text(&nvtop.stderr);
    assert_eq!(
        nvtop.status.code(),
        Some(1),
        "{}\n{stderr}",
        text(&nvtop.stdout)
    );
    assert!(
        stderr.contains("GDRCopy-2.5.2-GCCcore-15.2.0.eb is unsupported on riscv64:"),
        "{stderr}"
    );
    assert!(
        stderr.contains("(required by nvtop-3.3.2-GCCcore-15.2.0.eb)"),
        "{stderr}"
    );
    assert!(
        !dir.path().join("plan.json").exists(),
        "a refused plan is not written"
    );

    let zlib_dir = tempfile::tempdir().expect("tempdir");
    let zlib = run(&[
        "stack",
        "batch",
        "--root",
        "zlib-1.3.1-GCCcore-15.2.0.eb",
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
        "--installed",
        fixture("installed-empty.txt").to_str().unwrap(),
        "--eessi-version",
        "2026.06",
        "--easybuild-version",
        "5.4.0",
        "--generation",
        "2026.1",
        "--generated-at",
        "2026-10-29T22:12:00Z",
        "--out-dir",
        zlib_dir.path().to_str().unwrap(),
    ]);
    assert!(
        zlib.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&zlib.stdout),
        text(&zlib.stderr)
    );

    let x86_dir = tempfile::tempdir().expect("tempdir");
    let x86 = run(&[
        "stack",
        "batch",
        "--arch",
        "x86_64",
        "--root",
        "GDRCopy-2.5.2-GCCcore-15.2.0.eb",
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
        "--installed",
        fixture("installed-empty.txt").to_str().unwrap(),
        "--eessi-version",
        "2026.06",
        "--easybuild-version",
        "5.4.0",
        "--generation",
        "2026.1",
        "--generated-at",
        "2026-10-29T22:12:00Z",
        "--out-dir",
        x86_dir.path().to_str().unwrap(),
    ]);
    assert!(
        x86.status.success(),
        "GDRCopy is unsupported on riscv64, and this plan is only for x86_64:\n{}\n{}",
        text(&x86.stdout),
        text(&x86.stderr)
    );
}
