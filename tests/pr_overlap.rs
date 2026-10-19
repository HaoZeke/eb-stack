//! `eb-stack pr-overlap`: per-PR build sets and the overlap between them,
//! from easystack changes read as data against a fixture robot tree.

use eb_stack::pr_overlap::{
    overlap_index, parse_installed, pr_record, render_markdown, to_toml, EasystackChange,
    OverlapConfig, PrInput, PrRecord, RobotTrees,
};
use eb_stack::{parse_easyconfig_tree, parse_easyconfig_trees};
use std::path::{Path, PathBuf};
use std::process::Command;

const STACK: &str = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml";
const REBUILDS: &str =
    "easystacks/software.eessi.io/2026.06/rebuilds/20261019-eb-5.4.0-zlib-rebuild.yml";
const OLD_STACK: &str = "easystacks/software.eessi.io/2025.06/eessi-2025.06-eb-5.1.1-2025a.yml";
const BASE: &str = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_fixtures")
}

fn change(path: &str, base: Option<&str>, added: &str) -> EasystackChange {
    let head = format!("{}{added}", base.unwrap_or("easyconfigs:\n"));
    EasystackChange {
        path: path.into(),
        base: base.map(str::to_string),
        head: Some(head),
    }
}

fn pr(number: u64, easystacks: Vec<EasystackChange>) -> PrInput {
    PrInput {
        number,
        head_sha: format!("{number:0>40}"),
        title: Some(format!("PR {number}")),
        easystacks,
    }
}

fn prs() -> Vec<PrInput> {
    vec![
        pr(
            101,
            vec![change(
                STACK,
                Some(BASE),
                "  - Graphviz-14.0.2-GCCcore-15.2.0.eb\n",
            )],
        ),
        pr(
            102,
            vec![change(
                STACK,
                Some(BASE),
                "  - gnuplot-6.0.3-GCCcore-15.2.0.eb\n",
            )],
        ),
        pr(
            103,
            vec![change(
                STACK,
                Some(BASE),
                "  - NewThing-1.0-GCCcore-15.2.0.eb:\n      options:\n        from-commit: abc\n",
            )],
        ),
        pr(
            104,
            vec![change(REBUILDS, None, "  - zlib-1.3.1-GCCcore-15.2.0.eb\n")],
        ),
        pr(
            105,
            vec![change(
                OLD_STACK,
                None,
                "  - Graphviz-14.0.2-GCCcore-15.2.0.eb\n",
            )],
        ),
        pr(
            106,
            vec![change(
                STACK,
                Some(BASE),
                "  - Absent-2.0-GCCcore-15.2.0.eb:\n      options:\n        from-commit: def\n",
            )],
        ),
    ]
}

fn config() -> OverlapConfig {
    OverlapConfig {
        eessi_version: "2026.06".into(),
        installed_source: "x86_64/amd/zen4".into(),
        exclude_paths: vec!["/accel/".into()],
        include_paths: Vec::new(),
        generated_at: "2026-10-19T12:00:00Z".into(),
    }
}

fn records() -> Vec<PrRecord> {
    let base =
        parse_easyconfig_trees(&[fixtures().join("release").as_path()]).expect("release tree");
    assert!(base.skipped.is_empty(), "fixture parse: {:?}", base.skipped);
    let commit = parse_easyconfig_tree(&fixtures().join("commit-abc")).expect("commit tree");
    let mut trees = RobotTrees {
        base: base.candidates,
        commits: Default::default(),
    };
    trees.commits.insert("abc".into(), commit.candidates);
    let installed = parse_installed(
        &std::fs::read_to_string(fixtures().join("installed.txt")).expect("installed"),
    );
    prs()
        .iter()
        .filter_map(|p| pr_record(p, &trees, &installed, &config()))
        .collect()
}

fn record(records: &[PrRecord], n: u64) -> &PrRecord {
    records.iter().find(|r| r.pr == n).expect("record")
}

#[test]
fn a_build_set_stops_at_installed_modules() {
    let records = records();
    let r = record(&records, 101);
    assert_eq!(
        r.to_build,
        [
            "Graphviz/14.0.2-GCCcore-15.2.0",
            "freetype/2.14.1-GCCcore-15.2.0",
            "libpng/1.6.50-GCCcore-15.2.0",
        ]
    );
    // CMake is a build dependency of libpng and installed, so it is a leaf.
    assert_eq!(
        r.already_installed,
        [
            "CMake/4.1.2-GCCcore-15.2.0",
            "GCCcore/15.2.0",
            "zlib/1.3.1-GCCcore-15.2.0",
        ]
    );
    assert!(r.unresolved.is_empty(), "{:?}", r.unresolved);
    assert_eq!(r.entries.len(), 1, "the base entry is not counted");
    assert_eq!(r.entries[0].eb_version.as_deref(), Some("5.4.0"));
    assert_eq!(r.easystack_files, [STACK]);
}

#[test]
fn a_missing_dependency_is_reported_and_the_rest_still_counts() {
    let records = records();
    let r = record(&records, 102);
    assert!(r
        .to_build
        .contains(&"gnuplot/6.0.3-GCCcore-15.2.0".to_string()));
    assert!(r
        .to_build
        .contains(&"freetype/2.14.1-GCCcore-15.2.0".to_string()));
    assert_eq!(r.unresolved.len(), 1, "{:?}", r.unresolved);
    assert!(r.unresolved[0].contains("Missing"), "{:?}", r.unresolved);
}

#[test]
fn from_commit_easyconfigs_sit_ahead_of_the_release_tree() {
    let records = records();
    let r = record(&records, 103);
    assert_eq!(
        r.to_build,
        [
            "NewThing/1.0-GCCcore-15.2.0",
            "libpng/1.6.50-GCCcore-15.2.0"
        ]
    );
    assert_eq!(r.entries[0].from_commit.as_deref(), Some("abc"));

    let r = record(&records, 106);
    assert!(r.to_build.is_empty());
    assert_eq!(r.unresolved.len(), 2, "{:?}", r.unresolved);
    assert!(r.unresolved.iter().any(|u| u.contains("from-commit def")));
}

#[test]
fn a_rebuild_easystack_builds_its_installed_entry_again() {
    let records = records();
    let r = record(&records, 104);
    assert_eq!(r.to_build, ["zlib/1.3.1-GCCcore-15.2.0"]);
    assert_eq!(r.already_installed, ["GCCcore/15.2.0"]);
}

#[test]
fn include_path_keeps_only_matching_easystacks() {
    let riscv = "easystacks/software.eessi.io/2026.06/additional_arch/riscv64/generic/eessi-2026.06-eb-5.4.0-2026.1.yml";
    let inputs = [
        pr(
            201,
            vec![change(
                STACK,
                Some(BASE),
                "  - Graphviz-14.0.2-GCCcore-15.2.0.eb\n",
            )],
        ),
        pr(
            202,
            vec![change(
                riscv,
                Some(BASE),
                "  - Graphviz-14.0.2-GCCcore-15.2.0.eb\n",
            )],
        ),
    ];
    let base =
        parse_easyconfig_trees(&[fixtures().join("release").as_path()]).expect("release tree");
    let trees = RobotTrees {
        base: base.candidates,
        commits: Default::default(),
    };
    let installed = parse_installed(
        &std::fs::read_to_string(fixtures().join("installed.txt")).expect("installed"),
    );
    let mut cfg = config();
    cfg.include_paths = vec!["/additional_arch/riscv64/".into()];
    let kept: Vec<_> = inputs
        .iter()
        .filter_map(|p| pr_record(p, &trees, &installed, &cfg))
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].pr, 202);
    assert_eq!(kept[0].easystack_files, [riscv]);
}

#[test]
fn other_stack_versions_get_no_record() {
    let records = records();
    assert!(records.iter().all(|r| r.pr != 105));
    assert_eq!(records.len(), 5);
}

#[test]
fn the_index_names_every_shared_module_and_pair() {
    let records = records();
    let index = overlap_index(&records, &config());
    assert_eq!(index.prs, [101, 102, 103, 104, 106]);
    let shared: Vec<(&str, &[u64])> = index
        .shared
        .iter()
        .map(|(m, p)| (m.as_str(), p.as_slice()))
        .collect();
    assert_eq!(
        shared,
        [
            ("freetype/2.14.1-GCCcore-15.2.0", &[101, 102][..]),
            ("libpng/1.6.50-GCCcore-15.2.0", &[101, 102, 103][..]),
        ]
    );
    assert_eq!(index.pairs["103"]["101"], ["libpng/1.6.50-GCCcore-15.2.0"]);
    assert!(!index.pairs.contains_key("104"));
}

#[test]
fn records_round_trip_through_toml_and_carry_the_never_merge_header() {
    let records = records();
    let text = to_toml(record(&records, 103)).expect("toml");
    assert!(text.starts_with("# Generated by `eb-stack pr-overlap`"));
    assert!(text.contains("must never be merged"));
    let back: PrRecord = toml::from_str(&text).expect("parse back");
    assert_eq!(&back, record(&records, 103));
    let index = overlap_index(&records, &config());
    let text = to_toml(&index).expect("index toml");
    let back: eb_stack::pr_overlap::OverlapIndex = toml::from_str(&text).expect("index back");
    assert_eq!(back, index);
}

#[test]
fn the_markdown_report_says_who_shares_what() {
    let records = records();
    let index = overlap_index(&records, &config());
    let md = render_markdown(&records, &index, "https://github.com/EESSI/software-layer");
    assert!(md.contains("| `libpng/1.6.50-GCCcore-15.2.0` | [#101](https://github.com/EESSI/software-layer/pull/101), [#102]"));
    assert!(md.contains("- shares 2 with [#102](https://github.com/EESSI/software-layer/pull/102): `freetype/2.14.1-GCCcore-15.2.0`, `libpng/1.6.50-GCCcore-15.2.0`"));
    assert!(md.contains("- shares nothing with another open pull request"));
    assert!(!md.contains('@'), "no mentions in a generated report");
}

#[test]
fn the_cli_writes_the_same_bytes_twice_and_drops_closed_prs() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let input = temp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(&prs()).expect("json")).expect("write prs");
    let out = temp.path().join("out");
    std::fs::create_dir_all(out.join("prs")).expect("prs dir");
    std::fs::write(out.join("prs/999.toml"), "stale").expect("stale record");

    let run = || {
        let status = Command::new(binary)
            .arg("pr-overlap")
            .arg("--pr")
            .arg(&input)
            .arg("--easyconfigs")
            .arg(fixtures().join("release"))
            .arg("--commit-tree")
            .arg(format!("abc={}", fixtures().join("commit-abc").display()))
            .arg("--installed")
            .arg(fixtures().join("installed.txt"))
            .args(["--installed-label", "x86_64/amd/zen4"])
            .args(["--exclude-path", "/accel/"])
            .args(["--generated-at", "2026-10-19T12:00:00Z"])
            .arg("--out-dir")
            .arg(&out)
            .output()
            .expect("run pr-overlap");
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let mut files: Vec<(String, String)> = Vec::new();
        for dir in [out.clone(), out.join("prs")] {
            for entry in std::fs::read_dir(&dir).expect("read out") {
                let path = entry.expect("entry").path();
                if path.is_file() {
                    files.push((
                        path.strip_prefix(&out).unwrap().display().to_string(),
                        std::fs::read_to_string(&path).expect("read"),
                    ));
                }
            }
        }
        files.sort();
        files
    };
    let first = run();
    let second = run();
    assert_eq!(first, second, "deterministic output");
    let names: Vec<&str> = first.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "overlap.md",
            "overlap.toml",
            "prs/101.toml",
            "prs/102.toml",
            "prs/103.toml",
            "prs/104.toml",
            "prs/106.toml",
        ]
    );
}

fn run_overlap(dir: &Path, prs: &[PrInput], installed: &Path, out: &Path) {
    let input = dir.join("prs.json");
    std::fs::write(&input, serde_json::to_string(prs).expect("json")).expect("write prs");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .arg("pr-overlap")
        .arg("--pr")
        .arg(&input)
        .arg("--easyconfigs")
        .arg(fixtures().join("release"))
        .arg("--installed")
        .arg(installed)
        .args(["--installed-label", "x86_64/amd/zen4"])
        .args(["--exclude-path", "/accel/"])
        .args(["--generated-at", "2026-10-19T12:00:00Z"])
        .args(["--repo-url", "https://github.com/EESSI/software-layer"])
        .arg("--out-dir")
        .arg(out)
        .output()
        .expect("run pr-overlap");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
}

/// Lines inside every `to_build` array, passed through `sort | uniq -d`.
fn duplicate_to_build_lines(out: &Path) -> String {
    let status = Command::new("bash")
        .arg("-c")
        .arg(
            "awk '/^to_build = \\[/{f=1; next} f && /^\\]/{f=0; next} f' prs/*.toml | sort | uniq -d",
        )
        .current_dir(out)
        .output()
        .expect("uniq");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    String::from_utf8(status.stdout).expect("utf8")
}

fn git_diff_no_index(before: &str, after: &str) -> String {
    let dir = tempfile::tempdir().expect("diffdir");
    let a = dir.path().join("before.toml");
    let b = dir.path().join("after.toml");
    std::fs::write(&a, before).expect("before");
    std::fs::write(&b, after).expect("after");
    let status = Command::new("git")
        .args(["--no-pager", "diff", "--no-index", "--"])
        .arg(&a)
        .arg(&b)
        .output()
        .expect("git diff");
    String::from_utf8(status.stdout).expect("utf8")
}

/// Two pull requests add different easystack lines that both need `shared`,
/// which is not installed. After the first pull request is deployed and
/// closed, `shared` leaves the second record and the overlap goes with it.
fn shared_line_drops_out_after_deploy(
    a: u64,
    b: u64,
    added_a: &str,
    added_b: &str,
    shared: &str,
    deployed: &[&str],
    absent_from_easystack: &[&str],
) {
    for needle in absent_from_easystack {
        assert!(
            !added_a.contains(needle) && !added_b.contains(needle),
            "{needle} is named by an easystack line"
        );
    }
    let temp = tempfile::tempdir().expect("tempdir");
    let installed = temp.path().join("installed.txt");
    std::fs::copy(fixtures().join("installed.txt"), &installed).expect("installed");
    let out = temp.path().join("out");
    let open = vec![
        pr(a, vec![change(STACK, Some(BASE), added_a)]),
        pr(b, vec![change(STACK, Some(BASE), added_b)]),
    ];
    run_overlap(temp.path(), &open, &installed, &out);

    let md = std::fs::read_to_string(out.join("overlap.md")).expect("md");
    assert!(
        md.contains(&format!("`{shared}`")),
        "overlap.md should name {shared}: {md}"
    );
    assert!(
        md.contains(&format!("/pull/{a}")) && md.contains(&format!("/pull/{b}")),
        "{md}"
    );

    let dups = duplicate_to_build_lines(&out);
    assert_eq!(dups, format!("  \"{shared}\",\n"), "{dups}");

    let before_b = std::fs::read_to_string(out.join(format!("prs/{b}.toml"))).expect("b");
    assert!(before_b.contains(&format!("  \"{shared}\",\n")));
    let rec_b: PrRecord = toml::from_str(&before_b).expect("parse b");
    assert!(rec_b.to_build.iter().any(|m| m == shared));

    let mut text = std::fs::read_to_string(&installed).expect("read installed");
    for module in deployed {
        text.push_str(module);
        text.push('\n');
    }
    std::fs::write(&installed, text).expect("write installed");
    // A is closed: only B is still open. The previous prs/<A>.toml must go.
    run_overlap(
        temp.path(),
        &[pr(b, vec![change(STACK, Some(BASE), added_b)])],
        &installed,
        &out,
    );
    assert!(!out.join(format!("prs/{a}.toml")).exists());
    let after_b = std::fs::read_to_string(out.join(format!("prs/{b}.toml"))).expect("b after");
    let rec_b: PrRecord = toml::from_str(&after_b).expect("parse b after");
    assert!(
        !rec_b.to_build.iter().any(|m| m == shared),
        "{:?}",
        rec_b.to_build
    );
    assert!(
        rec_b.already_installed.iter().any(|m| m == shared),
        "{:?}",
        rec_b.already_installed
    );
    let md = std::fs::read_to_string(out.join("overlap.md")).expect("md after");
    assert!(md.contains("None."), "{md}");
    assert!(!md.contains(shared), "{md}");

    let diff = git_diff_no_index(&before_b, &after_b);
    assert!(
        diff.contains(&format!("-  \"{shared}\",")),
        "git diff should drop the shared to_build line:\n{diff}"
    );
}

#[test]
fn two_pull_requests_share_a_to_build_line_until_one_is_deployed() {
    shared_line_drops_out_after_deploy(
        301,
        302,
        "  - Alpha-1.0-GCCcore-15.2.0.eb\n",
        "  - Beta-2.0-GCCcore-15.2.0.eb\n",
        "SharedLib/1.2-GCCcore-15.2.0",
        &["Alpha/1.0-GCCcore-15.2.0", "SharedLib/1.2-GCCcore-15.2.0"],
        &["SharedLib"],
    );
}

#[test]
fn a_dependency_two_levels_down_is_the_same_to_build_line() {
    shared_line_drops_out_after_deploy(
        401,
        402,
        "  - Gamma-1.0-GCCcore-15.2.0.eb\n",
        "  - Delta-1.0-GCCcore-15.2.0.eb\n",
        "DeepLib/3.0-GCCcore-15.2.0",
        &["Gamma/1.0-GCCcore-15.2.0", "DeepLib/3.0-GCCcore-15.2.0"],
        &["DeepLib", "MidA", "MidB"],
    );
}
