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

/// easybuilders/easybuild-easyconfigs commit that adds
/// `petsc4py-3.25.0-foss-2026.1.eb`.
/// <https://github.com/easybuilders/easybuild-easyconfigs/commit/0d6b80c34f3553774e93162cf89088e2423ecf89>
const PETSC4PY_COMMIT: &str = "0d6b80c34f3553774e93162cf89088e2423ecf89";

/// `.eb` files under `easybuild/easyconfigs/` in [`PETSC4PY_COMMIT`].
/// `find easybuild/easyconfigs -name '*.eb' | wc -l` on the codeload
/// archive, 2026-11-04.
const EASYCONFIGS_IN_PETSC4PY_COMMIT: usize = 11897;

/// The fetched fixture is at least this many lines. The lists that built
/// one dependency in two shards had about 30 lines EasyBuild fetches.
const MIN_FETCHED_LIST_LINES: usize = 30;

/// Non-comment lines in `tests/stack_shard_fixtures/fetched.list`.
const FETCHED_LIST_LINES: usize = 34;

/// `--shards` from the how-to. This list has at least that many groups.
const SHARDS_REQUESTED: &str = "4";

/// Prelude size of `fetched.list` over [`PETSC4PY_COMMIT`] with petsc4py
/// removed from the robot path. Counted 2026-11-04.
const FETCHED_LIST_PRELUDE: &str = "173";

/// Stamp written into `plan.json` so two runs match byte for byte.
const FETCHED_GENERATED_AT: &str = "2026-11-04T00:00:00Z";

/// `EB_STACK_COMMIT_MIRROR`, when set, is a local copy of the codeload
/// `.tar.gz` for [`PETSC4PY_COMMIT`]. The shard command still downloads
/// that URL into `--commit-cache`.
const COMMIT_MIRROR_ENV: &str = "EB_STACK_COMMIT_MIRROR";

/// Path of petsc4py inside the commit. The commit message adds this file.
const PETSC4PY_RELATIVE: &str = "p/petsc4py/petsc4py-3.25.0-foss-2026.1.eb";

/// Module name `stack shard` records for `PETSc-3.25.0-foss-2026.1.eb`.
const PETSC_MODULE: &str = "PETSc/3.25.0-foss-2026.1";

fn commit_archive() -> Vec<u8> {
    if let Ok(path) = std::env::var(COMMIT_MIRROR_ENV) {
        return std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    }
    let url = format!(
        "https://codeload.github.com/easybuilders/easybuild-easyconfigs/tar.gz/{PETSC4PY_COMMIT}"
    );
    use eb_stack::pr_overlap::HttpGet;
    let response = eb_stack::pr_overlap::UreqGet
        .get(&url)
        .unwrap_or_else(|error| panic!("{url}: {error}"));
    assert_eq!(response.status, 200, "{url}");
    response.body
}

fn fetched_lines() -> Vec<String> {
    let text = std::fs::read_to_string(fixture("fetched.list")).expect("read fetched.list");
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Plan `fetched.list` twice against the public commit archive.
///
/// The robot path is that commit with petsc4py removed, so the file is
/// only in the tree `--commit-cache` downloads. petsc4py depends on PETSc,
/// and the PETSc line is fetched from the same commit.
#[test]
fn a_fetched_petsc_line_stays_with_petsc4py_and_the_plan_is_stable() {
    let lines = fetched_lines();
    assert!(
        lines.len() >= MIN_FETCHED_LIST_LINES,
        "{} lines, want at least {MIN_FETCHED_LIST_LINES}",
        lines.len()
    );
    assert_eq!(lines.len(), FETCHED_LIST_LINES);
    assert!(lines.iter().any(|line| !line.contains("--")));
    assert!(lines.iter().any(|line| line.contains("--from-commit=")));
    assert!(lines.iter().any(|line| {
        line.contains("--include-easyblocks-from-commit=") && !line.contains("--from-commit=")
    }));
    assert!(lines.iter().any(|line| {
        line.starts_with("PETSc-3.25.0-foss-2026.1.eb ") && line.contains(PETSC4PY_COMMIT)
    }));
    assert!(lines
        .iter()
        .any(|line| line.starts_with("petsc4py-3.25.0-foss-2026.1.eb ")));

    let tmp = tempfile::tempdir().unwrap();
    let archive = commit_archive();
    let tree = tmp.path().join("tree");
    let extracted = eb_stack::shard_trees::extract_easyconfigs(&archive, &tree)
        .unwrap_or_else(|error| panic!("extract: {error}"));
    assert_eq!(extracted, EASYCONFIGS_IN_PETSC4PY_COMMIT);
    let robot = tree.join("easybuild/easyconfigs");
    std::fs::remove_file(robot.join(PETSC4PY_RELATIVE)).expect("petsc4py is in the commit");
    let cache = tmp.path().join("cache");
    let summary = format!(
        "shards={SHARDS_REQUESTED} prelude={FETCHED_LIST_PRELUDE} lines={FETCHED_LIST_LINES}\n"
    );

    let mut plans = Vec::new();
    for name in ["first", "second"] {
        let out = tmp.path().join(name);
        let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
            .env_remove("EASYBUILD_FILTER_DEPS")
            .args([
                "stack",
                "shard",
                "--build-list",
                &fixture("fetched.list"),
                "--easyconfigs",
                robot.to_str().unwrap(),
                "--commit-cache",
                cache.to_str().unwrap(),
                "--shards",
                SHARDS_REQUESTED,
                "--generated-at",
                FETCHED_GENERATED_AT,
                "--out-dir",
                out.to_str().unwrap(),
            ])
            .output()
            .expect("run eb-stack");
        assert!(
            run.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&run.stdout), summary);
        assert!(
            run.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        plans.push(out);
    }

    let petsc4py = cache
        .join(PETSC4PY_COMMIT)
        .join("easybuild/easyconfigs")
        .join(PETSC4PY_RELATIVE);
    assert!(
        petsc4py.is_file(),
        "commit cache did not download {PETSC4PY_COMMIT}"
    );

    for name in listing(&plans[0]) {
        assert_eq!(
            std::fs::read(plans[0].join(&name)).unwrap(),
            std::fs::read(plans[1].join(&name)).unwrap(),
            "{name} differs across runs"
        );
    }

    let plan: Value = serde_json::from_str(&read(plans[0].join("plan.json"))).unwrap();
    let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for shard in plan["shards"].as_array().unwrap() {
        for module in shard["modules"].as_array().unwrap() {
            *seen
                .entry(module.as_str().unwrap().to_string())
                .or_default() += 1;
        }
    }
    let mut prelude_modules = Vec::new();
    for module in plan["prelude"]["modules"].as_array().unwrap() {
        let name = module["module"].as_str().unwrap().to_string();
        prelude_modules.push(name.clone());
        *seen.entry(name).or_default() += 1;
    }
    let twice: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
    assert!(twice.is_empty(), "built by two jobs: {twice:?}");

    let line = |file: &str| {
        plan["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|line| line["easyconfig"] == file)
            .unwrap_or_else(|| panic!("{file} not in the plan"))
            .clone()
    };
    let petsc = line("PETSc-3.25.0-foss-2026.1.eb");
    let py = line("petsc4py-3.25.0-foss-2026.1.eb");
    assert!(py["opaque"].is_null(), "{py}");
    assert!(py["closure"]
        .as_array()
        .unwrap()
        .iter()
        .any(|module| module == PETSC_MODULE));
    let together =
        petsc["shard"] == py["shard"] || prelude_modules.iter().any(|m| m == PETSC_MODULE);
    assert!(together, "PETSc {petsc} petsc4py {py}");
    let shard_file = plans[0].join(format!("{}.list", py["shard"].as_str().unwrap()));
    let shard_text = read(shard_file);
    let petsc_at = shard_text
        .find("PETSc-3.25.0-foss-2026.1.eb ")
        .expect("PETSc line in the petsc4py shard");
    let py_at = shard_text
        .find("petsc4py-3.25.0-foss-2026.1.eb ")
        .expect("petsc4py line");
    assert!(petsc_at < py_at, "{shard_text}");
}

#[test]
fn line_dependency_filters_are_refused_before_writing_a_plan() {
    for option in [
        "--filter-deps=zlib",
        "--filter-deps zlib",
        "--filter-dep=zlib",
        "--filter-d zlib",
    ] {
        for source in ["", " --from-commit=0123abcd"] {
            let tmp = tempfile::tempdir().unwrap();
            let list = tmp.path().join("build.list");
            let out = tmp.path().join("out");
            std::fs::write(&list, format!("# list\nAlpha.eb {option}{source}\n")).unwrap();
            let run = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
                .env_remove("EASYBUILD_FILTER_DEPS")
                .args(["stack", "shard", "--build-list"])
                .arg(&list)
                .arg("--easyconfigs")
                .arg(fixture("easyconfigs"))
                .arg("--out-dir")
                .arg(&out)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&run.stderr);
            assert!(!run.status.success(), "{option}{source}: {stderr}");
            assert!(stderr.contains("line 2: Alpha.eb"), "{stderr}");
            assert!(stderr.contains("dependency-filter override"), "{stderr}");
            assert!(!out.exists(), "a refused list wrote {}", out.display());
        }
    }
}

#[test]
fn other_line_filters_do_not_count_as_dependency_overrides() {
    for option in ["--filter-ecs=example", "--filter-env-vars=TMP"] {
        let line = eb_stack::shard::parse_lines(&format!("Alpha.eb {option}"));
        assert_eq!(line[0].filter_deps_option(), None, "{option}");
    }
}
