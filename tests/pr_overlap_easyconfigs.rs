//! `eb-stack pr-overlap --kind easyconfigs` against a four-file tree.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_easyconfigs_fixtures")
}

fn head(name: &str, dep: Option<(&str, &str)>, description: &str) -> String {
    let deps = match dep {
        Some((dep_name, version)) => format!("dependencies = [('{dep_name}', '{version}')]\n\n"),
        None => String::new(),
    };
    format!(
        "easyblock = 'ConfigureMake'\n\n\
name = '{name}'\n\
version = '1.0'\n\n\
homepage = 'https://example.org/{name}'\n\
description = \"{description}\"\n\n\
toolchain = SYSTEM\n\n\
source_urls = ['https://example.org/']\n\
sources = ['{name}-1.0.tar.gz']\n\
checksums = ['0000000000000000000000000000000000000000000000000000000000000000']\n\n\
{deps}\
moduleclass = 'lib'\n"
    )
}

fn pr(
    number: u64,
    title: &str,
    path: &str,
    base: Option<&str>,
    head_text: &str,
) -> serde_json::Value {
    let mut change = serde_json::json!({ "path": path, "head": head_text });
    if let Some(base) = base {
        change["base"] = serde_json::json!(base);
    }
    serde_json::json!({
        "number": number,
        "head_sha": number.to_string().repeat(40),
        "title": title,
        "easyconfigs": [change],
    })
}

#[test]
fn the_cli_matches_the_easyconfig_fixture() {
    let dir = fixtures();
    let lib = std::fs::read_to_string(dir.join("l/Lib/Lib-1.0.eb")).unwrap();
    let dep = std::fs::read_to_string(dir.join("d/Dep/Dep-1.0.eb")).unwrap();
    let lib_head = lib.replace("Fixture.", "Head.");
    let dep_head = dep.replace("Fixture.", "Added.");
    let prs = vec![
        pr(1, "edit Lib", "l/Lib/Lib-1.0.eb", Some(&lib), &lib_head),
        pr(2, "edit Lib too", "l/Lib/Lib-1.0.eb", Some(&lib), &lib_head),
        pr(
            3,
            "add New",
            "n/New/New-1.0.eb",
            None,
            &head("New", Some(("Lib", "1.0")), "Head."),
        ),
        pr(
            4,
            "add New too",
            "n/New/New-1.0.eb",
            None,
            &head("New", Some(("Lib", "1.0")), "Head."),
        ),
        pr(5, "add Dep file", "contrib/Dep-1.0.eb", None, &dep_head),
        pr(
            6,
            "add App",
            "a/App/App-1.0.eb",
            None,
            &head("App", Some(("Dep", "1.0")), "Head."),
        ),
        pr(
            7,
            "add Need",
            "n/Need/Need-1.0.eb",
            None,
            &head("Need", None, "Head."),
        ),
        pr(
            8,
            "add User",
            "u/User/User-1.0.eb",
            None,
            &head("User", Some(("Need", "1.0")), "Head.").replace(
                "dependencies = [('Need', '1.0')]\n\n",
                "dependencies = [('Dep', '1.0'), ('Need', '1.0')]\n\n",
            ),
        ),
    ];
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(&prs).unwrap()).unwrap();
    let out = temp.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(&input)
        .arg("--easyconfigs")
        .arg(&dir)
        .args(["--generated-at", "2026-10-22T00:00:00Z"])
        .args([
            "--repo-url",
            "https://github.com/easybuilders/easybuild-easyconfigs",
        ])
        .arg("--out-dir")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let expected = dir.join("expected");
    for name in [
        "overlap.md",
        "overlap.toml",
        "prs/1.toml",
        "prs/2.toml",
        "prs/3.toml",
        "prs/4.toml",
        "prs/5.toml",
        "prs/6.toml",
        "prs/7.toml",
        "prs/8.toml",
    ] {
        let got = std::fs::read_to_string(out.join(name)).unwrap();
        let want = std::fs::read_to_string(expected.join(name)).unwrap();
        assert_eq!(got, want, "{name}");
    }
}

fn run_easy(out: &std::path::Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(fixtures().join("prs.json"))
        .arg("--easyconfigs")
        .arg(fixtures())
        .args(["--generated-at", "2026-10-24T00:00:00Z"])
        .args([
            "--repo-url",
            "https://github.com/easybuilders/easybuild-easyconfigs",
        ])
        .arg("--out-dir")
        .arg(out)
        .args(extra)
        .output()
        .expect("run pr-overlap")
}

/// `--impact` has to use the same installed set as `merge-order.toml`.
#[test]
fn impact_honours_merged_the_way_merge_order_does() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("out");
    let status = run_easy(&out, &["--merged", "7", "--impact", "8"]);
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{err}");
    let order: eb_stack::merge_order::MergeOrder =
        toml::from_str(&std::fs::read_to_string(out.join("merge-order.toml")).unwrap()).unwrap();
    let step = order
        .steps
        .iter()
        .find(|step| step.pr == 8)
        .expect("PR 8 step");
    assert_eq!(step.builds, vec!["User/1.0".to_string()]);
    let report: eb_stack::merge_order::MergeImpactReport =
        toml::from_str(&std::fs::read_to_string(out.join("merge-impact.toml")).unwrap()).unwrap();
    let impact = report
        .impacts
        .iter()
        .find(|impact| impact.pr == 8)
        .expect("impact 8");
    assert!(
        impact.affected.iter().all(|effect| effect.pr != 7),
        "merged PR 7 is still affected: {:?}",
        impact.affected
    );
    for effect in &impact.affected {
        assert!(
            !effect.shrinks.iter().any(|module| module == "Need/1.0"),
            "Need/1.0 is already installed by merged PR 7: {:?}",
            impact.affected
        );
    }
    let both = run_easy(
        &temp.path().join("both"),
        &["--merged", "7", "--impact", "7"],
    );
    assert_eq!(both.status.code(), Some(2));
    let err = String::from_utf8_lossy(&both.stderr);
    assert!(
        err.contains("--impact 7: already listed in --merged"),
        "{err}"
    );
}

/// Patching Lib schedules the bundle that ships it, and orders that patch
/// before a pull request that builds a module depending on Lib.
#[test]
fn patching_a_dependency_schedules_its_must_rebuild_dependents() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("out");
    let status = run_easy(&out, &["--merge-order", "--impact", "1"]);
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{err}");
    let record: eb_stack::pr_overlap::PrRecord =
        toml::from_str(&std::fs::read_to_string(out.join("prs/1.toml")).unwrap()).unwrap();
    assert!(
        record.to_build.iter().any(|module| module == "Bundle/1.0"),
        "must-rebuild dependent missing from to_build: {:?}",
        record.to_build
    );
    assert!(
        record.to_build.iter().any(|module| module == "Lib/1.0"),
        "patched module missing from to_build: {:?}",
        record.to_build
    );
    let bundle = record
        .reverse_dependents
        .iter()
        .find(|dep| dep.module == "Bundle/1.0")
        .expect("bundle listed");
    assert_eq!(bundle.class, "must-rebuild");
    let order: eb_stack::merge_order::MergeOrder =
        toml::from_str(&std::fs::read_to_string(out.join("merge-order.toml")).unwrap()).unwrap();
    let step = order
        .steps
        .iter()
        .find(|step| step.pr == 1)
        .expect("PR 1 step");
    assert!(
        step.builds.iter().any(|module| module == "Bundle/1.0"),
        "merge order did not schedule Bundle: {:?}",
        step.builds
    );
    let dependent = order
        .steps
        .iter()
        .find(|step| step.pr == 3)
        .expect("PR 3 step");
    assert_eq!(
        dependent.after,
        vec![1, 2],
        "the Lib patches should order before New"
    );
    let report: eb_stack::merge_order::MergeImpactReport =
        toml::from_str(&std::fs::read_to_string(out.join("merge-impact.toml")).unwrap()).unwrap();
    let impact = report
        .impacts
        .iter()
        .find(|impact| impact.pr == 1)
        .expect("impact 1");
    let on_new = impact
        .affected
        .iter()
        .find(|effect| effect.pr == 3)
        .expect("PR 3 affected by the Lib patch");
    assert_eq!(on_new.resolves, vec!["Lib/1.0".to_string()]);
    let markdown = std::fs::read_to_string(out.join("overlap.md")).unwrap();
    assert!(markdown.contains("`Bundle/1.0` (1, must-rebuild)"));
}

/// A runtime dependent of a patched recipe is listed and is not built.
#[test]
fn a_no_rebuild_dependent_is_not_scheduled() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("out");
    let status = run_easy(&out, &["--merge-order", "--impact", "1"]);
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{err}");
    let record: eb_stack::pr_overlap::PrRecord =
        toml::from_str(&std::fs::read_to_string(out.join("prs/1.toml")).unwrap()).unwrap();
    let tool = record
        .reverse_dependents
        .iter()
        .find(|dep| dep.module == "Tool/1.0")
        .expect("Tool listed");
    assert_eq!(tool.class, "no-rebuild");
    let wrap = record
        .reverse_dependents
        .iter()
        .find(|dep| dep.module == "Wrap/1.0")
        .expect("Wrap listed");
    assert_eq!(wrap.class, "no-rebuild");
    assert!(
        !record
            .to_build
            .iter()
            .any(|module| module == "Tool/1.0" || module == "Wrap/1.0"),
        "no-rebuild dependents were scheduled: {:?}",
        record.to_build
    );
    let compile = record
        .reverse_dependents
        .iter()
        .find(|dep| dep.module == "Compile/1.0")
        .expect("Compile listed");
    assert_eq!(compile.class, "review");
    assert!(
        !record.to_build.iter().any(|module| module == "Compile/1.0"),
        "review dependent was scheduled: {:?}",
        record.to_build
    );
    let order: eb_stack::merge_order::MergeOrder =
        toml::from_str(&std::fs::read_to_string(out.join("merge-order.toml")).unwrap()).unwrap();
    for step in &order.steps {
        assert!(
            step.builds.iter().all(|module| module != "Tool/1.0"
                && module != "Wrap/1.0"
                && module != "Compile/1.0"),
            "PR {} scheduled a dependent that is not must-rebuild: {:?}",
            step.pr,
            step.builds
        );
    }
    let markdown = std::fs::read_to_string(out.join("overlap.md")).unwrap();
    assert!(markdown.contains("`Tool/1.0` (1, no-rebuild)"));
    assert!(markdown.contains("`Wrap/1.0` (2, no-rebuild)"));
    assert!(markdown.contains("`Compile/1.0` (1, review)"));
}

/// Easyconfigs edges come from overlap `depends-on` rows.
#[test]
fn easyconfigs_merge_order_and_impact_follow_depends_on_rows() {
    let temp = tempfile::tempdir().expect("temp");
    let out = temp.path().join("out");
    let status = run_easy(&out, &["--merge-order", "--impact", "7"]);
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{err}");
    let order: eb_stack::merge_order::MergeOrder =
        toml::from_str(&std::fs::read_to_string(out.join("merge-order.toml")).unwrap()).unwrap();
    let step = order
        .steps
        .iter()
        .find(|step| step.pr == 8)
        .expect("PR 8 step");
    assert_eq!(step.after, vec![7], "PR 8 should follow the depends-on row");
    let report: eb_stack::merge_order::MergeImpactReport =
        toml::from_str(&std::fs::read_to_string(out.join("merge-impact.toml")).unwrap()).unwrap();
    let impact = report
        .impacts
        .iter()
        .find(|impact| impact.pr == 7)
        .expect("impact 7");
    let on_8 = impact
        .affected
        .iter()
        .find(|effect| effect.pr == 8)
        .expect("PR 8 affected");
    assert_eq!(on_8.resolves, vec!["Need/1.0".to_string()]);

    // `--impact 8` must not say that merging 8 shrinks 7 by Need/1.0.
    // Need exists only in PR 7. PR 8's closure contains it because the
    // record is closed over PR 7's head.
    let alone = run_easy(&temp.path().join("impact-8"), &["--impact", "8"]);
    let err = String::from_utf8_lossy(&alone.stderr);
    assert!(alone.status.success(), "{err}");
    let report: eb_stack::merge_order::MergeImpactReport = toml::from_str(
        &std::fs::read_to_string(temp.path().join("impact-8").join("merge-impact.toml")).unwrap(),
    )
    .unwrap();
    let impact = report
        .impacts
        .iter()
        .find(|impact| impact.pr == 8)
        .expect("impact 8");
    assert!(
        impact.affected.iter().all(|effect| {
            effect.pr != 7 && effect.shrinks.iter().all(|module| module != "Need/1.0")
        }),
        "PR 8 does not build Need/1.0: {:?}",
        impact.affected
    );
}

/// A pull request with no easyconfigs is in the input and has no record.
/// `--merged` and `--impact` both reject it before writing.
#[test]
fn merged_and_impact_reject_the_same_recordless_pull_request() {
    let tmp = tempfile::tempdir().expect("temp");
    let mut prs: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join("prs.json")).unwrap())
            .unwrap();
    prs.push(serde_json::json!({
        "number": 9,
        "head_sha": "9".repeat(40),
        "title": "empty",
        "easyconfigs": []
    }));
    let input = tmp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(&prs).unwrap()).unwrap();
    for flag in ["--merged", "--impact"] {
        let out_dir = tmp.path().join(flag.trim_start_matches('-'));
        let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
            .env_remove("EASYBUILD_FILTER_DEPS")
            .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
            .arg(&input)
            .arg("--easyconfigs")
            .arg(fixtures())
            .args([
                "--generated-at",
                "2026-10-24T00:00:00Z",
                flag,
                "9",
                "--out-dir",
            ])
            .arg(&out_dir)
            .output()
            .expect("run pr-overlap");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{flag} {stderr}");
        assert!(
            stderr.contains(&format!(
                "{flag} 9: pull request 9 has no record (no easyconfigs or easystack change)"
            )),
            "{stderr}"
        );
        assert!(!out_dir.exists(), "{flag} left {}", out_dir.display());
    }
}

fn run_tree(tree: &Path, prs: &[serde_json::Value], extra: &[&str]) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("temp");
    let input = temp.path().join("prs.json");
    std::fs::write(&input, serde_json::to_string(prs).unwrap()).unwrap();
    let out = temp.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(&input)
        .arg("--easyconfigs")
        .arg(tree)
        .args(["--generated-at", "2026-10-29T00:00:00Z", "--repo-url", ""])
        .arg("--out-dir")
        .arg(&out)
        .args(extra)
        .output()
        .expect("run pr-overlap");
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(status.status.success(), "{err}");
    temp
}

fn read_record(out: &Path, number: u64) -> eb_stack::pr_overlap::PrRecord {
    toml::from_str(&std::fs::read_to_string(out.join(format!("prs/{number}.toml"))).unwrap())
        .unwrap()
}

fn read_index(out: &Path) -> eb_stack::pr_overlap::OverlapIndex {
    toml::from_str(&std::fs::read_to_string(out.join("overlap.toml")).unwrap()).unwrap()
}

/// `('Java', '21', '', SYSTEM)` is a reverse dependent until `--filter-deps Java`.
#[test]
fn filter_deps_drops_a_system_java_reverse_dependent() {
    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_easyconfigs_filter");
    let java = std::fs::read_to_string(tree.join("j/Java/Java-21.eb")).unwrap();
    let app = std::fs::read_to_string(tree.join("a/App/App-1.0-GCCcore-15.2.0.eb")).unwrap();
    let prs = vec![
        pr(
            1,
            "patch Java",
            "j/Java/Java-21.eb",
            Some(&java),
            &java.replace("Fixture.", "Head."),
        ),
        pr(
            2,
            "patch App",
            "a/App/App-1.0-GCCcore-15.2.0.eb",
            Some(&app),
            &app.replace("Fixture.", "Head."),
        ),
    ];
    let open = run_tree(&tree, &prs, &["--merge-order"]);
    let out = open.path().join("out");
    let record = read_record(&out, 1);
    let app_dep = record
        .reverse_dependents
        .iter()
        .find(|dep| dep.module.starts_with("App/"))
        .expect("App is classified without the filter");
    assert!(!app_dep.class.is_empty(), "{app_dep:?}");
    let index = read_index(&out);
    assert!(
        index.rows.iter().any(|row| {
            row.kind == "depends-on"
                && row.from == Some(1)
                && row.to == Some(2)
                && row.modules.iter().any(|module| module.starts_with("Java/"))
        }),
        "missing depends-on row: {:?}",
        index.rows
    );

    let filtered = run_tree(&tree, &prs, &["--filter-deps", "Java", "--merge-order"]);
    let out = filtered.path().join("out");
    let record = read_record(&out, 1);
    assert!(
        record
            .reverse_dependents
            .iter()
            .all(|dep| !dep.module.starts_with("App/")),
        "filtered Java still classified App: {:?}",
        record.reverse_dependents
    );
    let index = read_index(&out);
    assert!(
        index.rows.iter().all(|row| row.kind != "depends-on"),
        "filtered Java still created a depends-on row: {:?}",
        index.rows
    );
}

/// Two patches that each build a dependent of the other are one cycle.
#[test]
fn two_patches_that_depend_on_each_other_are_a_cycle() {
    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_easyconfigs_cycle");
    let alpha = std::fs::read_to_string(tree.join("a/Alpha/Alpha-1.0.eb")).unwrap();
    let beta = std::fs::read_to_string(tree.join("b/Beta/Beta-1.0.eb")).unwrap();
    let prs = vec![
        pr(
            1,
            "patch Alpha",
            "a/Alpha/Alpha-1.0.eb",
            Some(&alpha),
            &alpha.replace("Fixture.", "Head."),
        ),
        pr(
            2,
            "patch Beta",
            "b/Beta/Beta-1.0.eb",
            Some(&beta),
            &beta.replace("Fixture.", "Head."),
        ),
    ];
    let ran = run_tree(&tree, &prs, &["--merge-order"]);
    let out = ran.path().join("out");
    let index = read_index(&out);
    let edges: Vec<(u64, u64)> = index
        .rows
        .iter()
        .filter(|row| row.kind == "depends-on")
        .map(|row| (row.from.unwrap(), row.to.unwrap()))
        .collect();
    assert!(edges.contains(&(1, 2)), "{edges:?}");
    assert!(edges.contains(&(2, 1)), "{edges:?}");
    let order: eb_stack::merge_order::MergeOrder =
        toml::from_str(&std::fs::read_to_string(out.join("merge-order.toml")).unwrap()).unwrap();
    assert_eq!(order.cycles, vec![vec![1, 2]]);
    assert_eq!(order.steps.len(), 2);
    assert!(
        order
            .steps
            .iter()
            .any(|step| step.reason.contains("inside a cycle")),
        "{:?}",
        order.steps
    );
}
