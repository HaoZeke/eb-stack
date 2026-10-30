//! Known per-architecture failures, seeded from EESSI software-layer bot runs.
//!
//! The easyconfigs are the upstream files at the commits the pull requests
//! pinned: AITW-aiida-wood-permeability, aiida-core, pymatgen and orjson at
//! 94de5a1 (#1829), GROMACS at 4ee416d (#1658), Kaleido before and at
//! be78248 (#1670), waLBerla before ad6355e and at a69e693 (#1623). The
//! easystacks are the hunks those pull requests added, and the `-first`
//! and `-unpinned` ones are the push before the remedy was pinned.

use eb_stack::mcp::handle_message;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/arch_known_failures_fixtures")
        .join(name)
}

fn path(name: &str) -> String {
    fixture(name).to_str().unwrap().to_string()
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

fn known(args: &[&str]) -> String {
    let mut argv = vec!["stack", "known-failures"];
    argv.extend_from_slice(args);
    let output = run(&argv);
    let stdout = text(&output.stdout);
    assert!(
        output.status.success(),
        "warnings exit 0\nstdout:\n{stdout}\nstderr:\n{}",
        text(&output.stderr)
    );
    stdout
}

#[test]
fn pr_1829_warns_about_orjson_on_a64fx_through_the_aitw_closure() {
    let easystack = path("easystacks/1829/eessi-2026.06-eb-5.4.1-2026.1.yml");
    let easyconfigs = path("easyconfigs");
    let stdout = known(&[
        "--arch",
        "aarch64/a64fx",
        "--easystack",
        &easystack,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert!(
        stdout.contains(
            "orjson-3.11.8-GCCcore-15.2.0.eb on aarch64/a64fx: known test failure: test_datetime_zoneinfo_utc fails"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("(required by AITW-aiida-wood-permeability-0.3.0-foss-2026.1.eb)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  remedy: append -k \"not test_datetime_zoneinfo_utc\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("  in EESSI: the pre_single_extension_orjson hook applies it"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  seen in: software-layer #1871, #1829; software-layer-scripts #330"),
        "{stdout}"
    );
    assert!(stdout.ends_with("known=1 remedied=0\n"), "{stdout}");

    let haswell = known(&[
        "--arch",
        "x86_64/intel/haswell",
        "--easystack",
        &easystack,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert_eq!(haswell, "known=0 remedied=0\n");

    let installed = path("installed-a64fx-orjson.txt");
    let already = known(&[
        "--arch",
        "aarch64",
        "--easystack",
        &easystack,
        "--easyconfigs",
        &easyconfigs,
        "--installed",
        &installed,
    ]);
    assert_eq!(
        already, "known=0 remedied=0\n",
        "an installed orjson is not rebuilt"
    );
}

#[test]
fn pr_1670_first_push_warns_about_kaleido_on_aarch64_and_the_pin_remedies_it() {
    let easyconfigs = path("easyconfigs");
    let first = path("easystacks/1670-first/eessi-2025.06-eb-5.4.0-2024a.yml");
    let stdout = known(&[
        "--arch",
        "aarch64/nvidia/grace",
        "--easystack",
        &first,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert!(
        stdout.contains(
            "Kaleido-0.2.1-GCCcore-13.3.0.eb on aarch64/nvidia/grace: known fetch failure: kaleido-0.2.1-py2.py3-none-manylinux1_aarch64.whl does not exist"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("(required by MultiQC-1.28-gfbf-2024a.eb)"),
        "{stdout}"
    );
    assert!(stdout.contains("if ARCH == 'aarch64'"), "{stdout}");
    assert!(stdout.ends_with("known=1 remedied=0\n"), "{stdout}");

    let x86 = known(&[
        "--arch",
        "x86_64/amd/zen4",
        "--easystack",
        &first,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert_eq!(x86, "known=0 remedied=0\n");

    let merged = known(&[
        "--arch",
        "aarch64",
        "--easystack",
        &path("easystacks/1670/eessi-2025.06-eb-5.4.0-2024a.yml"),
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert_eq!(
        merged,
        "Kaleido-0.2.1-GCCcore-13.3.0.eb on aarch64: known fetch failure, remedied: from-commit be78248 carries the remedy\nknown=0 remedied=1\n"
    );

    let fixed_tree = known(&[
        "--arch",
        "aarch64",
        "--easystack",
        &first,
        "--easyconfigs",
        &easyconfigs,
        "--easyconfigs",
        &path("easyconfigs-fixed"),
    ]);
    assert!(
        fixed_tree.contains("remedied: the easyconfig already has manylinux2014")
            || fixed_tree.contains("known fetch failure:"),
        "{fixed_tree}"
    );
}

#[test]
fn pr_1623_walberla_warns_on_the_sve_targets_until_pinned() {
    let easyconfigs = path("easyconfigs");
    let unpinned = path("easystacks/1623-unpinned/eessi-2025.06-eb-5.4.0-2025b.yml");
    let stdout = known(&[
        "--arch",
        "aarch64",
        "--easystack",
        &unpinned,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert!(
        stdout.starts_with(
            "waLBerla-7.2-foss-2025b.eb on aarch64/neoverse_v1, aarch64/a64fx, aarch64/nvidia/grace: known build failure: SIMDAlignment() is not constexpr"
        ),
        "{stdout}"
    );
    assert!(stdout.ends_with("known=1 remedied=0\n"), "{stdout}");

    let neoverse_n1 = known(&[
        "--arch",
        "aarch64/neoverse_n1",
        "--easystack",
        &unpinned,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert_eq!(
        neoverse_n1, "known=0 remedied=0\n",
        "neoverse_n1 has no SVE"
    );

    let pinned = known(&[
        "--easystack",
        &path("easystacks/1623/eessi-2025.06-eb-5.4.0-2025b.yml"),
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert!(
        pinned.contains("remedied: from-commit a69e693 carries the remedy"),
        "{pinned}"
    );
    assert!(pinned.ends_with("known=0 remedied=1\n"), "{pinned}");

    let patched = known(&[
        "--arch",
        "aarch64/a64fx",
        "--easystack",
        &unpinned,
        "--easyconfigs",
        &path("easyconfigs-fixed"),
    ]);
    assert!(
        patched.contains("remedied: the easyconfig already has 0d1d7ff8..."),
        "{patched}"
    );
}

#[test]
fn pr_1683_numba_fails_tests_on_a64fx_and_times_out_on_zen2() {
    let stdout = known(&[
        "--arch",
        "aarch64/a64fx",
        "--arch",
        "x86_64/amd/zen2",
        "--easystack",
        &path("easystacks/1683/eessi-2025.06-eb-5.4.0-2025a.yml"),
        "--easyconfigs",
        &path("easyconfigs"),
    ]);
    assert!(
        stdout.contains("numba-0.62.0-foss-2025a.eb on aarch64/a64fx: known test failure: test_sum_axis_dtype_kws and test_sum_axis_kws1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("numba-0.62.0-foss-2025a.eb on x86_64/amd/zen2: known test timeout:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("sed -i 's/timeout=[3,6]0/timeout=600/g'"),
        "{stdout}"
    );
    assert!(stdout.ends_with("known=2 remedied=0\n"), "{stdout}");
}

#[test]
fn pr_1668_uv_is_matched_by_file_name_and_the_haswell_entry_is_2025_06_only() {
    let easystack = path("easystacks/1668/eessi-2025.06-eb-5.4.0-2025b.yml");
    let stdout = known(&[
        "--arch",
        "aarch64/a64fx",
        "--arch",
        "x86_64/intel/haswell",
        "--easystack",
        &easystack,
    ]);
    assert!(
        stdout.contains("uv-0.10.9-GCCcore-14.3.0.eb on aarch64/a64fx: known build failure: maturin crashes writing the wheel because SOURCE_DATE_EPOCH is not set"),
        "{stdout}"
    );
    assert!(
        stdout.contains("uv-0.10.9-GCCcore-14.3.0.eb on x86_64/intel/haswell: known build failure: rustc segfaults"),
        "{stdout}"
    );
    assert!(stdout.ends_with("known=2 remedied=0\n"), "{stdout}");

    let later = known(&[
        "--arch",
        "x86_64/intel/haswell",
        "--easystack",
        &easystack,
        "--eessi-version",
        "2026.06",
    ]);
    assert_eq!(later, "known=0 remedied=0\n");
}

#[test]
fn pr_1658_gromacs_cuda_demotion_warns_until_the_easyblock_pin() {
    let easyconfigs = path("easyconfigs");
    let first = path("easystacks/1658-first/accel/nvidia/eessi-2026.06-eb-5.4.0-2026.1.yml");
    let stdout = known(&[
        "--arch",
        "x86_64/amd/zen5",
        "--easystack",
        &first,
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert!(
        stdout.contains("GROMACS-2026.3-foss-2026.1-CUDA-13.3.0.eb on x86_64/amd/zen5: known sanity check failure:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("  caused by: the EESSI inject_gpu_property hook"),
        "{stdout}"
    );
    assert!(
        stdout.contains("include-easyblocks-from-commit f6f23d4"),
        "{stdout}"
    );
    assert!(stdout.ends_with("known=1 remedied=0\n"), "{stdout}");

    let pinned = known(&[
        "--arch",
        "x86_64/amd/zen5",
        "--easystack",
        &path("easystacks/1658/accel/nvidia/eessi-2026.06-eb-5.4.0-2026.1.yml"),
        "--easyconfigs",
        &easyconfigs,
    ]);
    assert!(
        pinned.contains("remedied: include-easyblocks-from-commit f6f23d4 carries the remedy"),
        "{pinned}"
    );

    let older = known(&[
        "--arch",
        "x86_64/amd/zen5",
        "--easystack",
        &first,
        "--easyconfigs",
        &easyconfigs,
        "--easybuild-version",
        "5.3.0",
    ]);
    assert_eq!(
        older, "known=0 remedied=0\n",
        "EasyBuild before 5.4 loads build dependencies for the sanity check"
    );
}

#[test]
fn a_missing_easystack_is_a_usage_error() {
    let output = run(&["stack", "known-failures"]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("--easystack"), "{stderr}");
}

#[test]
fn stack_batch_writes_the_warnings_into_the_pull_request_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let tree = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/arch_unsupported_fixtures/easyconfigs");
    let installed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/arch_unsupported_fixtures/installed-empty.txt");
    let output = run(&[
        "stack",
        "batch",
        "--arch",
        "x86_64/amd/zen4",
        "--root",
        "nvtop-3.3.2-GCCcore-15.2.0.eb",
        "--easyconfigs",
        tree.to_str().unwrap(),
        "--installed",
        installed.to_str().unwrap(),
        "--eessi-version",
        "2026.06",
        "--easybuild-version",
        "5.4.0",
        "--generation",
        "2026.1",
        "--generated-at",
        "2026-10-30T00:00:00Z",
        "--known-failures",
        &path("batch-known-failures.toml"),
        "--out-dir",
        dir.path().to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let batch = std::fs::read_dir(dir.path())
        .expect("out dir")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path.is_dir())
        .expect("one batch directory");
    let body = std::fs::read_to_string(batch.join("pr-body.md")).expect("pr-body.md");
    assert!(
        body.contains("Known per-architecture failures, from `eb-stack stack known-failures`:"),
        "{body}"
    );
    assert!(
        body.contains("GDRCopy-2.5.2-GCCcore-15.2.0.eb on x86_64/amd/zen4: known test failure: fixture symptom (required by nvtop-3.3.2-GCCcore-15.2.0.eb)"),
        "{body}"
    );
    let record: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(batch.join("batch.json")).expect("batch.json"),
    )
    .expect("json");
    assert_eq!(record["known_failures"][0]["id"], "gdrcopy-batch-fixture");
    let plan = std::fs::read_to_string(dir.path().join("plan.md")).expect("plan.md");
    assert!(
        plan.contains("1 known per-architecture failure(s); `pr-body.md` has the remedies."),
        "{plan}"
    );
}

#[test]
fn the_mcp_tool_returns_the_same_report_and_warnings_are_not_an_error() {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_known_failures",
            "arguments": {
                "arch": ["aarch64/a64fx"],
                "easystack": [path("easystacks/1829/eessi-2026.06-eb-5.4.1-2026.1.yml")],
                "easyconfigs": [path("easyconfigs")],
            }
        }
    }))
    .expect("response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    let hits = body["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1, "{body}");
    assert_eq!(hits[0]["id"], "orjson-a64fx-zoneinfo-utc");
    assert_eq!(hits[0]["file"], "orjson-3.11.8-GCCcore-15.2.0.eb");
    assert_eq!(hits[0]["phase"], "test");
    assert_eq!(hits[0]["arches"], json!(["aarch64/a64fx"]));
    assert_eq!(
        hits[0]["required_by"],
        json!(["AITW-aiida-wood-permeability-0.3.0-foss-2026.1.eb"])
    );
    assert!(body["text"]
        .as_str()
        .expect("text")
        .ends_with("known=1 remedied=0\n"));
}
