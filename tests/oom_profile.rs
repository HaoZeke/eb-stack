//! Out-of-memory warnings for a plan, from EESSI software-layer bot jobs.
//!
//! Each easystack is the lines a real pull request added. The libxc build
//! in pull request 1772 was killed twice on zen4. Highway in 1692 ran out on
//! aarch64/generic. hipBLASLt for rocBLAS in 1549 needed about 90 GiB. The
//! PETSc test step in 1608 ran out on zen5 before the hooks halved it.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/oom_profile_fixtures")
        .join(name)
}

fn arg(name: &str) -> String {
    fixture(name).to_str().unwrap().to_string()
}

fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn libxc_from_pr_1772_warns_on_zen4_only() {
    let easystack = arg("easystacks/pr1772-eessi-2026.06-eb-5.4.0-2026.1.yml");
    let (code, stdout, stderr) = run(&["stack", "oom", "--easystack", &easystack]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let first = "libxc-7.0.0-GCC-15.2.0.eb may run out of memory on x86_64/amd/zen4 (eessi-bot-mc-aws amd-zen4): 8 x 4.0 GiB = 32.0 GiB, the job has 32.0 GiB (software-layer #1772)";
    assert!(stdout.contains(first), "{stdout}");
    assert!(
        stdout.contains("  parallel 8: 16 cores, eb_hooks.py divides libxc by 2\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  remedy: maxparallel = 7 in libxc-7.0.0-GCC-15.2.0.eb, or a libxc limit of 7 for x86_64/amd/zen4 in PARALLELISM_LIMITS in software-layer-scripts eb_hooks.py; 4.0 GiB per process is a lower bound, so 7 may still be too many\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  remedy: build x86_64/amd/zen4 on a node with more memory: eessi-bot-surf amd-zen4 (16 cores, 180.0 GiB, accel nvidia/cc90)\n"),
        "{stdout}"
    );
    assert!(stdout.contains("divider of 2.5 or 3"), "{stdout}");
    // aarch64/generic built the same easyconfig at 8 cores in that pull request.
    assert!(!stdout.contains("on aarch64/generic"), "{stdout}");
    assert_eq!(
        stdout.matches("may run out of memory").count(),
        1,
        "{stdout}"
    );
    assert!(
        stdout.ends_with("oom=1 fits=14 builds=5 nodes=15\n"),
        "{stdout}"
    );
}

#[test]
fn an_installed_libxc_is_not_checked() {
    let (code, stdout, stderr) = run(&[
        "stack",
        "oom",
        "--arch",
        "x86_64/amd/zen4",
        "--easystack",
        &arg("easystacks/pr1772-eessi-2026.06-eb-5.4.0-2026.1.yml"),
        "--installed",
        &arg("installed-libxc.txt"),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "oom=0 fits=0 builds=4 nodes=2\n");
}

#[test]
fn highway_from_pr_1692_warns_on_aarch64_generic_through_eon() {
    let (code, stdout, stderr) = run(&[
        "stack",
        "oom",
        "--arch",
        "aarch64/generic",
        "--arch",
        "x86_64/amd/zen4",
        "--easystack",
        &arg("easystacks/pr1692-eessi-2026.06-eb-5.4.0-2026.1.yml"),
        "--easyconfigs",
        &arg("easyconfigs"),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("Highway-1.4.0-GCCcore-15.2.0.eb may run out of memory on aarch64/generic (eessi-bot-mc-aws generic): 16 x 1.9 GiB = 30.4 GiB, the job has 29.8 GiB (software-layer #1692, software-layer-scripts #317) (required by eOn-3.6.0-foss-2026.1.eb)\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  parallel 16: 16 cores, no eb_hooks.py limit for Highway\n"),
        "{stdout}"
    );
    assert!(stdout.contains("#317 halves Highway"), "{stdout}");
    assert!(!stdout.contains("on x86_64/amd/zen4"), "{stdout}");
    assert!(
        stdout.ends_with("oom=1 fits=0 builds=2 nodes=3\n"),
        "{stdout}"
    );
}

#[test]
fn hipblaslt_from_pr_1549_needs_a_bigger_node_not_fewer_cores() {
    let (code, stdout, stderr) = run(&[
        "stack",
        "oom",
        "--arch",
        "x86_64/amd/zen2",
        "--easystack",
        &arg("easystacks/pr1549-eessi-2025.06-eb-5.3.1-2025a.yml"),
        "--easyconfigs",
        &arg("easyconfigs"),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("hipBLASLt-0.12.1-rocm-compilers-19.0.0-ROCm-6.4.1.eb may run out of memory on x86_64/amd/zen2 (eessi-bot-mc-aws amd-zen2): 90.0 GiB at any parallel level, the job has 27.9 GiB"),
        "{stdout}"
    );
    assert!(
        stdout.contains("(required by rocBLAS-4.4.0-rocm-compilers-19.0.0-ROCm-6.4.1.eb)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "  remedy: a lower parallel level does not help: one process already needs 90.0 GiB\n"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("-xl build nodes"), "{stdout}");
    assert!(
        stdout.contains(
            "rocBLAS-4.4.0-rocm-compilers-19.0.0-ROCm-6.4.1.eb may run out of memory on x86_64/amd/zen2"
        ),
        "{stdout}"
    );
    assert!(
        stdout.ends_with("oom=2 fits=0 builds=2 nodes=1\n"),
        "{stdout}"
    );
}

#[test]
fn petsc_from_pr_1608_warns_only_with_the_hooks_from_before_298() {
    let easystack = arg("easystacks/pr1608-eessi-2025.06-eb-5.4.0-2025b.yml");
    let easyconfigs = arg("easyconfigs");
    let (code, stdout, stderr) = run(&[
        "stack",
        "oom",
        "--arch",
        "x86_64/amd/zen5",
        "--easystack",
        &easystack,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "oom=0 fits=1 builds=3 nodes=1\n");

    let (code, stdout, stderr) = run(&[
        "stack",
        "oom",
        "--easystack",
        &easystack,
        "--easyconfigs",
        &easyconfigs,
        "--resources",
        &arg("resources-before-298.toml"),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("PETSc-3.24.0-foss-2025b.eb may run out of memory on x86_64/amd/zen5 (eessi-bot-aws-eu-south amd-zen5): 16 x 2.0 GiB = 32.0 GiB, the job has 31.5 GiB (software-layer #1608, software-layer-scripts #298) (required by slepc4py-3.24.0-foss-2025b.eb)\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  parallel 16: 16 cores, no eb_hooks.py limit for PETSc\n"),
        "{stdout}"
    );
    assert!(
        stdout.ends_with("oom=1 fits=0 builds=3 nodes=1\n"),
        "{stdout}"
    );
}

#[test]
fn a_target_without_a_bot_node_is_reported_not_checked() {
    let (code, stdout, stderr) = run(&[
        "stack",
        "oom",
        "--arch",
        "aarch64/aws/graviton4",
        "--easystack",
        &arg("easystacks/pr1772-eessi-2026.06-eb-5.4.0-2026.1.yml"),
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        "aarch64/aws/graviton4 has no bot node in the bot resource table, so it is not checked\noom=0 fits=0 builds=5 nodes=0\n"
    );
}

#[test]
fn an_easystack_is_required_and_a_directory_is_refused() {
    let (code, _, stderr) = run(&["stack", "oom"]);
    assert_eq!(code, 2, "{stderr}");
    let (code, _, stderr) = run(&["stack", "oom", "--easystack", &arg("easystacks")]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("is a directory"), "{stderr}");
}

/// Each `eb-stack stack oom` block in the how-to prints the example block
/// that follows it, run from the repository root.
#[test]
fn the_how_to_shows_the_real_output() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let page = std::fs::read_to_string(root.join("docs/orgmode/howto/oom-profile.org")).unwrap();
    let mut lines = page.lines();
    let mut checked = 0;
    while let Some(line) = lines.next() {
        if line != "#+begin_src bash" {
            continue;
        }
        let command: String = lines
            .by_ref()
            .take_while(|l| *l != "#+end_src")
            .map(|l| l.trim().trim_end_matches('\\').trim())
            .collect::<Vec<_>>()
            .join(" ");
        let args: Vec<&str> = command.split_whitespace().collect();
        assert_eq!(&args[..3], ["eb-stack", "stack", "oom"], "{command}");
        lines.by_ref().find(|l| *l == "#+begin_example").unwrap();
        let expected: String = lines
            .by_ref()
            .take_while(|l| *l != "#+end_example")
            .map(|l| format!("{l}\n"))
            .collect();
        let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
            .args(&args[1..])
            .current_dir(&root)
            .output()
            .expect("run eb-stack");
        assert!(output.status.success(), "{command}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "{command}"
        );
        checked += 1;
    }
    assert_eq!(checked, 5);
}
