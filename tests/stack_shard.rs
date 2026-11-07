//! `stack shard` over a small wave-1-shaped build list.
//!
//! The fixture tree has OpenFOAM, ParaView, and PETSc with Hypre on GCCcore
//! 15.2.0. OpenFOAM and ParaView both need HDF5 (and its Szip); OpenFOAM and
//! PETSc both need MathLib. zlib is installed through a module tree and CMake
//! through an installed list, so neither is built. PETSc needs Hypre, which is
//! a line of its own, so those two lines share a shard with Hypre first. The
//! other dependencies two shards need go to the prelude.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/stack_shard_fixtures")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

fn shard(list: &str, out: &Path, extra: &[&str]) -> std::process::Output {
    let mut args: Vec<String> = [
        "stack",
        "shard",
        "--build-list",
        &fixture(list),
        "--easyconfigs",
        &fixture("easyconfigs"),
        "--installed",
        &fixture("installed.txt"),
        "--installed-modules-dir",
        &fixture("modules/all"),
        "--generated-at",
        "2026-11-03T00:00:00Z",
        "--out-dir",
        out.to_str().unwrap(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(extra.iter().map(|s| s.to_string()));
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(&args)
        .output()
        .expect("run eb-stack")
}

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn wave_one_splits_into_three_shards_and_a_prelude() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("shards");
    let run = shard("wave1.list", &out, &["--shards", "4"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "shards=3 prelude=3 lines=4\n"
    );
    assert_eq!(
        listing(&out),
        [
            "plan.json",
            "prelude.list",
            "shard-01.list",
            "shard-02.list",
            "shard-03.list"
        ]
    );
    // The prelude holds what two shards need, in dependency order: Szip
    // before HDF5.
    let prelude = read(out.join("prelude.list"));
    let prelude: Vec<&str> = prelude.lines().collect();
    let mut sorted = prelude.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        [
            "HDF5-1.14.6-GCCcore-15.2.0.eb",
            "MathLib-1.0-GCCcore-15.2.0.eb",
            "Szip-2.1.1-GCCcore-15.2.0.eb"
        ]
    );
    let at = |name: &str| prelude.iter().position(|l| l.starts_with(name)).unwrap();
    assert!(at("Szip") < at("HDF5"));

    // Heaviest group first: PETSc with Hypre and SuiteSparse weighs 3.
    // Lines are verbatim, the line PETSc needs first.
    assert_eq!(
        read(out.join("shard-01.list")),
        "Hypre-2.33.0-GCCcore-15.2.0.eb\nPETSc-3.23.0-GCCcore-15.2.0.eb --include-easyblocks=/sw/easyblocks/petsc.py\n"
    );
    assert_eq!(
        read(out.join("shard-02.list")),
        "ParaView-6.0.0-GCCcore-15.2.0.eb\n"
    );
    assert_eq!(
        read(out.join("shard-03.list")),
        "OpenFOAM-13-GCCcore-15.2.0.eb --hooks=/sw/hooks/openfoam.py\n"
    );

    let plan: Value = serde_json::from_str(&read(out.join("plan.json"))).unwrap();
    assert_eq!(plan["generated_at"], "2026-11-03T00:00:00Z");
    assert_eq!(plan["shards_requested"], 4);
    assert_eq!(plan["inputs"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(plan["prelude"]["weight"], 3);
    let weights: Vec<u64> = plan["shards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["weight"].as_u64().unwrap())
        .collect();
    assert_eq!(weights, [3, 2, 1]);
    assert_eq!(
        plan["shards"][0]["modules"],
        serde_json::json!([
            "Hypre/2.33.0-GCCcore-15.2.0",
            "PETSc/3.23.0-GCCcore-15.2.0",
            "SuiteSparse/7.10.0-GCCcore-15.2.0"
        ])
    );
    // Installed modules from both sources are not in any closure.
    let text = read(out.join("plan.json"));
    assert!(!text.contains("\"zlib/"), "zlib is installed: {text}");
    assert!(!text.contains("\"CMake/"), "CMake is installed: {text}");
    let lines = plan["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["line"], 2);
    assert_eq!(lines[0]["shard"], "shard-03");
    assert_eq!(lines[2]["group"], lines[3]["group"]);
    assert_eq!(
        lines[0]["closure"].as_array().unwrap().len(),
        4,
        "OpenFOAM, MathLib, HDF5, Szip: {}",
        lines[0]["closure"]
    );
}

#[test]
fn weights_balance_the_shards_by_time() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("shards");
    let weights = fixture("weights.tsv");
    let run = shard(
        "wave1.list",
        &out,
        &["--shards", "2", "--weights", &weights],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "shards=2 prelude=3 lines=4\n"
    );
    // ParaView with Qt6 is 4200 s; OpenFOAM (3600 s) and the PETSc group
    // (600 + 300 + 400 s) share the other shard.
    assert_eq!(
        read(out.join("shard-01.list")),
        "ParaView-6.0.0-GCCcore-15.2.0.eb\n"
    );
    assert_eq!(
        read(out.join("shard-02.list")),
        "OpenFOAM-13-GCCcore-15.2.0.eb --hooks=/sw/hooks/openfoam.py\nHypre-2.33.0-GCCcore-15.2.0.eb\nPETSc-3.23.0-GCCcore-15.2.0.eb --include-easyblocks=/sw/easyblocks/petsc.py\n"
    );
    let plan: Value = serde_json::from_str(&read(out.join("plan.json"))).unwrap();
    assert_eq!(plan["shards"][0]["weight"], 4200);
    assert_eq!(plan["shards"][1]["weight"], 4900);
    assert!(plan["inputs"]["weights_sha256"].is_string());
}

#[test]
fn the_same_inputs_give_identical_outputs() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    for out in [&a, &b] {
        let run = shard("wave1.list", out, &["--shards", "4"]);
        assert!(run.status.success());
    }
    for name in listing(&a) {
        assert_eq!(
            std::fs::read(a.join(&name)).unwrap(),
            std::fs::read(b.join(&name)).unwrap(),
            "{name} differs"
        );
    }
    assert_eq!(listing(&a), listing(&b));
}

#[test]
fn a_rerun_with_fewer_shards_leaves_no_stale_shard_list() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("shards");
    assert!(shard("wave1.list", &out, &["--shards", "4"])
        .status
        .success());
    let run = shard("wave1.list", &out, &["--shards", "1"]);
    assert!(run.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "shards=1 prelude=3 lines=4\n"
    );
    assert_eq!(
        listing(&out),
        ["plan.json", "prelude.list", "shard-01.list"]
    );
    assert_eq!(read(out.join("shard-01.list")).lines().count(), 4);
}

#[test]
fn an_unresolved_line_fails_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("shards");
    let run = shard("unresolved.list", &out, &["--shards", "4"]);
    assert_eq!(run.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("line 2: Missing-1.0-GCCcore-15.2.0.eb: no such easyconfig"),
        "{stderr}"
    );
    assert!(
        stderr.contains("line 3: Broken-1.0-GCCcore-15.2.0.eb"),
        "{stderr}"
    );
    assert!(stderr.contains("needs NoSuchLib ==9.9"), "{stderr}");
    assert!(!out.exists(), "nothing is written");
    assert!(run.stdout.is_empty());
}

#[test]
fn fetched_and_unparsed_lines_are_planned_as_opaque_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("shards");
    let run = shard("opaque.list", &out, &["--shards", "4"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let plan: Value = serde_json::from_str(&read(out.join("plan.json"))).unwrap();
    let lines = plan["lines"].as_array().unwrap();
    let by_file = |f: &str| {
        lines
            .iter()
            .find(|l| l["easyconfig"] == f)
            .unwrap_or_else(|| panic!("{f} not in the plan"))
            .clone()
    };
    let fetched = by_file("Bowtie2-2.5.5-GCC-15.2.0.eb");
    assert_eq!(
        fetched["opaque"],
        "fetched by EasyBuild (--from-pr or --from-commit)"
    );
    assert_eq!(
        fetched["closure"],
        serde_json::json!(["Bowtie2-2.5.5-GCC-15.2.0.eb"])
    );
    let unparsed = by_file("Opaque-1.0.eb");
    assert_eq!(
        unparsed["opaque"],
        "on the robot path, not parsed by eb-stack"
    );
    assert!(by_file("OpenFOAM-13-GCCcore-15.2.0.eb")["opaque"].is_null());
    // The opaque line is written to its shard as it stands, flags and all.
    let shard_file = format!("{}.list", fetched["shard"].as_str().unwrap());
    assert!(read(out.join(&shard_file)).contains(
        "Bowtie2-2.5.5-GCC-15.2.0.eb --from-commit=e71b392cfab2449cb64f0ac5569cba75f0c5290d --include-systems=snel-zen2"
    ));
}

#[test]
fn zero_shards_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("shards");
    let run = shard("wave1.list", &out, &["--shards", "0"]);
    assert_eq!(run.status.code(), Some(2));
    assert!(!out.exists());
}

#[test]
fn the_mcp_tool_writes_the_same_files_as_the_command() {
    if std::env::var_os("EASYBUILD_FILTER_DEPS").is_some() {
        panic!("unset EASYBUILD_FILTER_DEPS: the CLI run below ignores it, the tool would not");
    }
    let tmp = tempfile::tempdir().unwrap();
    let cli = tmp.path().join("cli");
    assert!(shard("wave1.list", &cli, &["--shards", "4"])
        .status
        .success());
    let mcp = tmp.path().join("mcp");
    let response = eb_stack::mcp::handle_message(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_shard",
            "arguments": {
                "build_list": fixture("wave1.list"),
                "easyconfigs": [fixture("easyconfigs")],
                "installed": [fixture("installed.txt")],
                "installed_modules_dir": [fixture("modules/all")],
                "shards": 4,
                "generated_at": "2026-11-03T00:00:00Z",
                "out_dir": mcp.to_str().unwrap(),
            }
        }
    }))
    .expect("response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(listing(&cli), listing(&mcp));
    for name in listing(&cli) {
        assert_eq!(read(cli.join(&name)), read(mcp.join(&name)), "{name}");
    }
}

#[test]
fn an_inode_quota_caps_the_concurrent_shards_without_changing_the_lists() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("plain");
    assert!(shard("wave1.list", &plain, &["--shards", "4"])
        .status
        .success());
    let capped = tmp.path().join("capped");
    let footprints = fixture("footprints.json");
    // PETSc is 240,000 in the table; the rest default to 60,000. Shard
    // footprints: the PETSc shard 240,000 + 2 lines * 1000 = 242,000; the
    // ParaView and OpenFOAM shards 61,000 each. The quota 450,000 minus the
    // margin 100,000 fits the two largest (303,000), not all three (364,000).
    let run = shard(
        "wave1.list",
        &capped,
        &[
            "--shards",
            "4",
            "--inode-quota",
            "450000",
            "--inode-footprints",
            &footprints,
        ],
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "shards=3 prelude=3 lines=4 max_concurrent=2\n"
    );
    // The shard and prelude lists are the ones the plain run wrote.
    for name in [
        "prelude.list",
        "shard-01.list",
        "shard-02.list",
        "shard-03.list",
    ] {
        assert_eq!(read(plain.join(name)), read(capped.join(name)), "{name}");
    }
    let plan: Value = serde_json::from_str(&read(capped.join("plan.json"))).unwrap();
    assert_eq!(plan["max_concurrent_shards"], 2);
    assert_eq!(plan["inode_budget"]["quota"], 450_000);
    assert_eq!(plan["inode_budget"]["margin"], 100_000);
    assert_eq!(plan["inode_budget"]["budget"], 350_000);
    assert_eq!(plan["inode_budget"]["default_footprint"], 60_000);
    assert_eq!(plan["inode_budget"]["line_overhead"], 1_000);
    assert_eq!(
        plan["inode_budget"]["footprints_sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    let shards = plan["shards"].as_array().unwrap();
    assert_eq!(shards[0]["inode_footprint"], 242_000);
    assert_eq!(
        shards[0]["inode_peak_module"],
        "PETSc/3.23.0-GCCcore-15.2.0"
    );
    assert_eq!(shards[0]["inode_overhead"], 2_000);
    assert_eq!(shards[1]["inode_footprint"], 61_000);
    assert_eq!(shards[2]["inode_footprint"], 61_000);
    // The prelude builds three modules in one job: 60,000 + 1,000.
    assert_eq!(plan["prelude"]["inode_footprint"], 61_000);
    // The digest changes with the budget; the plain plan has none.
    let plain_plan: Value = serde_json::from_str(&read(plain.join("plan.json"))).unwrap();
    assert_ne!(plain_plan["inputs"]["sha256"], plan["inputs"]["sha256"]);
    assert_eq!(plain_plan["max_concurrent_shards"], 3);
    assert!(plain_plan["inode_budget"]["quota"].is_null());
    assert_eq!(plain_plan["shards"][0]["inode_footprint"], 62_000);

    // A quota the largest shard does not fit under exits 1 and writes nothing.
    let small = tmp.path().join("small");
    let run = shard(
        "wave1.list",
        &small,
        &[
            "--shards",
            "4",
            "--inode-quota",
            "300000",
            "--inode-footprints",
            &footprints,
        ],
    );
    assert_eq!(run.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("shard-01 needs 242000 inodes"), "{stderr}");
    assert!(!small.exists());
    // A margin at or above the quota is a usage error.
    let run = shard(
        "wave1.list",
        &small,
        &["--shards", "4", "--inode-quota", "100000"],
    );
    assert_eq!(run.status.code(), Some(2));
}
