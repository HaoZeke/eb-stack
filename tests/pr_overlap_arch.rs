//! Per-architecture bot build set.
//!
//! The shared easystack already lists GDRCopy. The pull requests add unrelated
//! entries. GDRCopy is installed on x86_64 and not on riscv64, so only the
//! riscv64 build set includes it and its missing dependency.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_arch")
}

fn run(out: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--pr"])
        .arg(root().join("prs.json"))
        .arg("--easyconfigs")
        .arg(root().join("release"))
        .args(["--generated-at", "2026-10-30T00:00:00Z"])
        .args(["--repo-url", "https://github.com/EESSI/software-layer"])
        .args(["--eessi-version", "2026.06"])
        .arg("--out-dir")
        .arg(out)
        .args(extra)
        .output()
        .expect("run pr-overlap")
}

fn to_build(text: &str) -> Vec<String> {
    let mut modules = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "to_build = [" {
            inside = true;
            continue;
        }
        if inside && line == "]" {
            inside = false;
            continue;
        }
        if inside {
            let module = line.trim().trim_matches(',').trim_matches('"');
            if !module.is_empty() {
                modules.push(module.to_string());
            }
        }
    }
    modules
}

#[test]
fn added_lines_leave_out_an_old_entry_the_bot_would_build() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("added");
    let installed = root().join("riscv64.txt");
    let status = run(
        &out,
        &[
            "--installed",
            installed.to_str().expect("utf8"),
            "--installed-label",
            "riscv64",
        ],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let record = std::fs::read_to_string(out.join("prs/1775.toml")).expect("record");
    let modules = to_build(&record);
    assert!(
        modules
            .iter()
            .any(|module| module == "nvtop/3.3.2-GCCcore-15.2.0"),
        "{modules:?}"
    );
    assert!(
        !modules.iter().any(|module| module.contains("GDRCopy")),
        "added lines must not include GDRCopy, got {modules:?}"
    );
    assert!(!record.contains("build_sets"));
}

#[test]
fn each_architecture_build_set_drops_only_what_that_architecture_has_installed() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("arches");
    let x86 = format!("x86_64={}", root().join("x86_64.txt").display());
    let riscv = format!("riscv64={}", root().join("riscv64.txt").display());
    let status = run(
        &out,
        &[
            "--arch-installed",
            &x86,
            "--arch-installed",
            &riscv,
            "--merge-order",
            "--impact",
            "1775",
        ],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).expect("utf8");
    assert!(
        stdout.contains("prs=2 records=2 shared_modules=2"),
        "{stdout}"
    );

    let record = std::fs::read_to_string(out.join("prs/1775.toml")).expect("1775");
    assert!(
        record.contains("build_scope = \"changed-easystack\""),
        "{record}"
    );
    assert!(record.contains("arch = \"riscv64\""), "{record}");
    assert!(record.contains("arch = \"x86_64\""), "{record}");
    let sections: Vec<&str> = record.split("[[build_sets]]").collect();
    let x86_set = sections
        .iter()
        .find(|section| section.contains("arch = \"x86_64\""))
        .expect("x86 set");
    let riscv_set = sections
        .iter()
        .find(|section| section.contains("arch = \"riscv64\""))
        .expect("riscv set");
    let x86_modules = to_build(x86_set);
    let riscv_modules = to_build(riscv_set);
    assert_eq!(x86_modules, ["nvtop/3.3.2-GCCcore-15.2.0"]);
    assert!(
        !x86_modules.iter().any(|module| module.contains("GDRCopy")),
        "{x86_modules:?}"
    );
    assert!(
        !x86_modules.iter().any(|module| module.starts_with("Lib/")),
        "an installed GDRCopy is a leaf, so Lib stays out of x86_64: {x86_modules:?}"
    );
    assert_eq!(
        riscv_modules,
        [
            "GDRCopy/2.5.2-GCCcore-15.2.0",
            "Lib/1.0-GCCcore-15.2.0",
            "nvtop/3.3.2-GCCcore-15.2.0",
        ]
    );
    assert!(
        riscv_set.contains("carried = [\"GDRCopy/2.5.2-GCCcore-15.2.0\"]")
            || riscv_set.contains("carried = [\n  \"GDRCopy/2.5.2-GCCcore-15.2.0\",\n]"),
        "{riscv_set}"
    );

    let report = std::fs::read_to_string(out.join("overlap.md")).expect("report");
    assert!(report.contains("## x86_64"), "{report}");
    assert!(report.contains("## riscv64"), "{report}");
    assert!(
        report.contains(
            "Already in the changed easystack and not installed: `GDRCopy/2.5.2-GCCcore-15.2.0`."
        ),
        "{report}"
    );
    let riscv_at = report.find("## riscv64").expect("riscv heading");
    let x86_at = report.find("## x86_64").expect("x86 heading");
    let (first, second) = if x86_at < riscv_at {
        (&report[x86_at..riscv_at], &report[riscv_at..])
    } else {
        (&report[riscv_at..x86_at], &report[x86_at..])
    };
    let (x86_report, riscv_report) = if x86_at < riscv_at {
        (first, second)
    } else {
        (second, first)
    };
    assert!(
        !x86_report.contains("GDRCopy"),
        "x86_64 report must not list GDRCopy: {x86_report}"
    );
    assert!(riscv_report.contains("`GDRCopy/2.5.2-GCCcore-15.2.0`"));
    assert!(riscv_report.contains("`Lib/1.0-GCCcore-15.2.0`"));

    let order = std::fs::read_to_string(out.join("merge-order.toml")).expect("order");
    assert!(order.contains("arch = \"riscv64\""), "{order}");
    assert!(
        order.contains("\"GDRCopy/2.5.2-GCCcore-15.2.0\""),
        "merge order consumes the riscv build set: {order}"
    );
    let impact = std::fs::read_to_string(out.join("merge-impact.toml")).expect("impact");
    assert!(
        impact.contains("\"GDRCopy/2.5.2-GCCcore-15.2.0\""),
        "impact consumes the riscv build set: {impact}"
    );
    assert!(impact.contains("\"Lib/1.0-GCCcore-15.2.0\""), "{impact}");

    for name in [
        "overlap.md",
        "overlap.toml",
        "merge-order.toml",
        "merge-impact.toml",
        "prs/1775.toml",
        "prs/1800.toml",
    ] {
        let got =
            std::fs::read_to_string(out.join(name)).unwrap_or_else(|_| panic!("missing {name}"));
        let expected = std::fs::read_to_string(root().join("expected").join(name))
            .unwrap_or_else(|_| panic!("missing expected {name}"));
        assert_eq!(got, expected, "{name}");
    }
}

#[test]
fn one_bot_build_set_replaces_to_build() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("bot");
    let installed = root().join("riscv64.txt");
    let status = run(
        &out,
        &[
            "--bot-build-set",
            "--installed",
            installed.to_str().expect("utf8"),
            "--installed-label",
            "riscv64",
            "--merge-order",
            "--impact",
            "1775",
        ],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let record = std::fs::read_to_string(out.join("prs/1775.toml")).expect("record");
    assert!(!record.contains("[[build_sets]]"), "{record}");
    assert_eq!(
        to_build(&record),
        [
            "GDRCopy/2.5.2-GCCcore-15.2.0".to_string(),
            "Lib/1.0-GCCcore-15.2.0".to_string(),
            "nvtop/3.3.2-GCCcore-15.2.0".to_string(),
        ]
    );
    let order = std::fs::read_to_string(out.join("merge-order.toml")).expect("order");
    assert!(
        order.contains("## Merge order\n") || order.contains("pr = 1775"),
        "{order}"
    );
    assert!(
        order.contains("\"GDRCopy/2.5.2-GCCcore-15.2.0\""),
        "{order}"
    );
    let report = std::fs::read_to_string(out.join("overlap.md")).expect("report");
    assert!(report.contains(
        "The build set is every entry at the head of each changed easystack, plus dependencies that are not installed."
    ));
    assert!(report.contains("## Merge order\n"));
    assert!(!report.contains("## Merge order ("));
}
