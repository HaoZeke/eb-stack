//! Real easybuild-easyblocks and easybuild-easyconfigs trees.
//!
//! Ignored unless run by name. Set `EB_EASYBLOCKS`, `EB_EASYCONFIGS_DEVELOP`,
//! and `EB_EASYCONFIGS_V541`. `EB_REACH_OUT` is where the reports are written.

use eb_stack::easyblock_reach::{reach, read_sources, write_report, ReachQuery, ReachReport};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn env_path(key: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).unwrap_or_else(|_| panic!("{key} is required")))
}

fn rg_files(dir: &Path, pattern: &str) -> BTreeSet<String> {
    let output = Command::new("rg")
        .args(["-l", "--glob", "*.eb", pattern])
        .arg(dir)
        .output()
        .expect("rg");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn paths_of(report: &ReachReport) -> HashSet<String> {
    report
        .hits
        .iter()
        .map(|hit| hit.easyconfig.clone())
        .collect()
}

#[test]
#[ignore = "needs EB_EASYBLOCKS, EB_EASYCONFIGS_DEVELOP, and EB_EASYCONFIGS_V541"]
fn develop_and_v541_match_grep() {
    let blocks = env_path("EB_EASYBLOCKS");
    let out_root =
        std::env::var("EB_REACH_OUT").unwrap_or_else(|_| "/tmp/easyblock-reach-real".into());
    let cases = [
        (
            "CMakeMake",
            "easybuild/easyblocks/generic/cmakemake.py",
            false,
        ),
        (
            "PythonPackage",
            "easybuild/easyblocks/generic/pythonpackage.py",
            false,
        ),
        ("Cargo", "easybuild/easyblocks/generic/cargo.py", false),
        ("EB_GROMACS", "easybuild/easyblocks/g/gromacs.py", true),
    ];
    for (label, tree_key) in [
        ("develop", "EB_EASYCONFIGS_DEVELOP"),
        ("v5.4.1", "EB_EASYCONFIGS_V541"),
    ] {
        let ecs = env_path(tree_key);
        let cmake_literal = rg_files(&ecs, "easyblock\\s*=\\s*['\"]CMakeMake['\"]");
        let bundle_literal = rg_files(&ecs, "easyblock\\s*=\\s*['\"]PythonBundle['\"]");
        let cargo_literal = rg_files(&ecs, "easyblock\\s*=\\s*['\"]Cargo['\"]");
        let cargo_sub = rg_files(&ecs, "easyblock\\s*=\\s*['\"]Cargo(Python|Meson)");
        let gromacs_named = rg_files(&ecs, "^name\\s*=\\s*['\"]GROMACS['\"]");
        let mut gromacs_implied = BTreeSet::new();
        for path in &gromacs_named {
            let text = std::fs::read_to_string(path).unwrap();
            if !text
                .lines()
                .any(|line| line.trim_start().starts_with("easyblock"))
            {
                gromacs_implied.insert(path.clone());
            }
        }
        println!(
            "{label} grep CMakeMake={} PythonBundle={} Cargo={} Cargo-subclass={} GROMACS-implied={}",
            cmake_literal.len(),
            bundle_literal.len(),
            cargo_literal.len(),
            cargo_sub.len(),
            gromacs_implied.len()
        );
        for (class_name, rel, reverse) in cases {
            let path = blocks.join(rel);
            let changed = read_sources(std::slice::from_ref(&path)).unwrap();
            let report = reach(&ReachQuery {
                roots: vec![blocks.clone()],
                easyconfigs: vec![ecs.clone()],
                changed,
                installed: BTreeSet::new(),
                reverse,
                generated_at: "2026-10-22T00:00:00Z".into(),
            })
            .unwrap_or_else(|error| panic!("{label} {class_name}: {error}"));
            println!(
                "{label} {class_name} easyconfigs={} hits={} classes={} reverse={} skipped={}",
                report.easyconfig_count,
                report.hits.len(),
                report.reached_classes.len(),
                report.reverse.len(),
                report.skipped.len()
            );
            write_report(
                &report,
                &PathBuf::from(&out_root).join(label).join(class_name),
            )
            .unwrap();
            let found = paths_of(&report);
            match class_name {
                "CMakeMake" => {
                    assert!(report
                        .reached_classes
                        .iter()
                        .any(|name| name == "EB_GROMACS"));
                    assert!(report.easyconfig_count >= cmake_literal.len());
                    for file in &cmake_literal {
                        assert!(found.contains(file), "missing {file}");
                    }
                    for file in &gromacs_implied {
                        assert!(found.contains(file), "CMakeMake missed {file}");
                    }
                }
                "PythonPackage" => {
                    assert!(report.easyconfig_count > bundle_literal.len());
                }
                "Cargo" => {
                    assert!(report.easyconfig_count >= cargo_literal.len());
                    for file in cargo_literal.iter().chain(cargo_sub.iter()) {
                        assert!(found.contains(file), "missing {file}");
                    }
                }
                "EB_GROMACS" => {
                    assert!(!gromacs_implied.is_empty());
                    for file in &gromacs_implied {
                        assert!(found.contains(file), "missing {file}");
                    }
                    let starts: HashSet<_> =
                        report.hits.iter().map(|hit| hit.module.as_str()).collect();
                    assert!(report
                        .reverse
                        .iter()
                        .all(|row| !starts.contains(row.module.as_str())));
                    let dep_tuples = rg_files(&ecs, r"\('GROMACS',");
                    println!("{label} EB_GROMACS dependency tuples={}", dep_tuples.len());
                    if dep_tuples.is_empty() {
                        assert!(report.reverse.is_empty());
                    } else {
                        assert!(!report.reverse.is_empty());
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    let bin = env!("CARGO_BIN_EXE_eb-stack");
    let develop = env_path("EB_EASYCONFIGS_DEVELOP");
    let cli_out = PathBuf::from(&out_root).join("cli-cmakemake");
    let status = Command::new(bin)
        .args([
            "easyblock-reach",
            "--easyblocks",
            blocks.to_str().unwrap(),
            "--easyconfigs",
            develop.to_str().unwrap(),
            "--file",
            blocks
                .join("easybuild/easyblocks/generic/cmakemake.py")
                .to_str()
                .unwrap(),
            "--generated-at",
            "2026-10-22T00:00:00Z",
            "--out-dir",
            cli_out.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(cli_out.join("reach.toml").is_file());
    assert!(cli_out.join("reach.md").is_file());
}
