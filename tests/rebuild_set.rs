//! `eb-stack rebuild-set`: installed reverse dependents of an in-place patch,
//! from a fixture robot tree and the deployed easystacks.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rebuild_set_fixtures")
}

fn stack() -> PathBuf {
    fixtures().join("eessi-2026.06-eb-5.4.0-2026.1.yml")
}

fn zlib_rebuild() -> PathBuf {
    fixtures().join("20261001-eb-5.4.1-zlib.yml")
}

fn from_commit_stack() -> PathBuf {
    fixtures().join("eessi-2026.06-eb-5.4.0-from-commit.yml")
}

struct Run {
    files: Vec<(String, String)>,
    stdout: String,
}

fn invoke(extra: &[&str], out: &Path) -> Run {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let output = Command::new(binary)
        .env_remove("EASYBUILD_FILTER_DEPS")
        .arg("rebuild-set")
        .args(extra)
        .arg("--out-dir")
        .arg(out)
        .output()
        .expect("run rebuild-set");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut files = Vec::new();
    collect(out, out, &mut files);
    files.sort();
    Run {
        files,
        stdout: String::from_utf8(output.stdout).expect("stdout"),
    }
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("read out")
        .map(|entry| entry.expect("entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            files.push((
                path.strip_prefix(root).unwrap().display().to_string(),
                std::fs::read_to_string(&path).expect("read"),
            ));
        }
    }
}

fn common_args() -> Vec<String> {
    vec![
        "--easyconfigs".into(),
        fixtures().join("release").display().to_string(),
        "--installed".into(),
        fixtures().join("installed.txt").display().to_string(),
        "--installed-label".into(),
        "x86_64/amd/zen4".into(),
        "--generated-at".into(),
        "2026-10-22T12:00:00Z".into(),
        "--changed".into(),
        fixtures().join("changed.txt").display().to_string(),
    ]
}

fn args_with(tail: &[String]) -> Vec<String> {
    let mut args = common_args();
    args.extend(tail.iter().cloned());
    args
}

#[test]
fn the_cli_writes_the_rebuild_set_and_drops_a_stale_easystack() {
    let temp = tempfile::tempdir().expect("tempdir");
    let out = temp.path().join("out");
    std::fs::create_dir_all(out.join("rebuilds")).expect("rebuilds");
    std::fs::write(out.join("rebuilds/stale.yml"), "stale").expect("stale");

    let args = args_with(&[
        "--easystack".into(),
        stack().display().to_string(),
        "--easystack".into(),
        zlib_rebuild().display().to_string(),
        "--installed-modulerc".into(),
        fixtures().join("modules").display().to_string(),
        "--easyblocks".into(),
        fixtures().join("easyblocks").display().to_string(),
    ]);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let first = invoke(&borrowed, &out);
    let second = invoke(&borrowed, &out);
    assert_eq!(first.files, second.files, "deterministic output");

    let names: Vec<&str> = first.files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "rebuild-set.json",
            "rebuild-set.md",
            "rebuilds/20261022-eb-5.4.0-cve-patches.yml",
            "rebuilds/20261022-eb-5.4.1-cve-patches.yml",
        ]
    );
    assert!(
        first.stdout.contains("must-rebuild=2")
            && first.stdout.contains("review=2")
            && first.stdout.contains("no-rebuild=4")
            && first.stdout.contains("missing=1"),
        "{}",
        first.stdout
    );

    let json = first
        .files
        .iter()
        .find(|(name, _)| name == "rebuild-set.json")
        .expect("json")
        .1
        .as_str();
    let value: serde_json::Value = serde_json::from_str(json).expect("parse json");
    assert_eq!(value["schema_version"], 4);
    assert_eq!(value["symbols_checked"], false);
    assert_eq!(value["generated_at"], "2026-10-22T12:00:00Z");
    assert_eq!(value["installed_source"], "x86_64/amd/zen4");
    assert_eq!(value["missing"], serde_json::json!(["Absent-1.0.eb"]));
    assert_eq!(value["counts"]["must-rebuild"], 2);
    assert_eq!(value["counts"]["review"], 2);
    assert_eq!(value["counts"]["no-rebuild"], 4);
    assert_eq!(
        value["notes"],
        serde_json::json!([
            "easyblock Bundle missing",
            "easyblock ConfigureMake missing"
        ])
    );

    let class_of = |dependent: &str| -> (String, String, String) {
        let row = value["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["dependent"] == dependent)
            .unwrap_or_else(|| panic!("no row for {dependent}: {}", value["rows"]));
        (
            row["class"].as_str().unwrap().to_string(),
            row["edge_kind"].as_str().unwrap().to_string(),
            row["reason"].as_str().unwrap().to_string(),
        )
    };
    assert_eq!(
        class_of("zlib/1.3.1-GCCcore-15.2.0"),
        (
            "must-rebuild".into(),
            "changed".into(),
            "patched easyconfig".into()
        )
    );
    assert_eq!(
        value["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["dependent"] == "zlib/1.3.1-GCCcore-15.2.0")
            .unwrap()["eb_version"],
        "5.4.1"
    );
    assert_eq!(class_of("libpng/1.6.50-GCCcore-15.2.0").0, "no-rebuild");
    assert_eq!(
        class_of("Graphviz/14.0.2-GCCcore-15.2.0").2,
        "runtime dependency through libpng/1.6.50-GCCcore-15.2.0; links the patched package as a shared library"
    );
    assert_eq!(class_of("CMake/4.1.2-GCCcore-15.2.0").0, "review");
    assert_eq!(
        class_of("AppStatic/1.0-GCCcore-15.2.0").2,
        "builddependency only, through libpng/1.6.50-GCCcore-15.2.0; it may have linked the patched package statically or embedded it"
    );
    assert_eq!(
        class_of("Bundle/1.0-GCCcore-15.2.0"),
        (
            "must-rebuild".into(),
            "extension".into(),
            "the patched package is an extension in this bundle".into()
        )
    );
    assert_eq!(class_of("PyApp/1.0-GCCcore-15.2.0").0, "no-rebuild");
    assert_eq!(class_of("Both/1.0-GCCcore-15.2.0").1, "runtime");
    let dependents: Vec<&str> = value["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["dependent"].as_str().unwrap())
        .collect();
    assert!(!dependents
        .iter()
        .any(|name| name.starts_with("NotInstalled")));
    assert!(!dependents.iter().any(|name| name.starts_with("Hidden")));
    assert!(!dependents.contains(&"zlib/1.3.1"));
    assert!(!dependents.contains(&"GCCcore/15.2.0"));

    let bundle = file_body(&first, "rebuilds/20261022-eb-5.4.0-cve-patches.yml");
    assert!(bundle
        .contains("  - Bundle-1.0-GCCcore-15.2.0.eb:\n      options:\n        rebuild: true\n"));
    assert!(!bundle.contains("zlib"));
    let zlib = file_body(&first, "rebuilds/20261022-eb-5.4.1-cve-patches.yml");
    assert!(
        zlib.contains("  - zlib-1.3.1-GCCcore-15.2.0.eb:\n      options:\n        rebuild: true\n")
    );
    assert!(!zlib.contains("libpng"));

    let markdown = file_body(&first, "rebuild-set.md");
    assert!(markdown.contains("| Changed easyconfig | Dependent | Edge kind | Class | Reason |"));
    assert!(markdown.contains("2 must-rebuild, 2 review, 4 no-rebuild."));
    assert!(markdown.contains("- `Absent-1.0.eb`"));
    assert!(markdown.contains("- note: easyblock Bundle missing"));
    assert!(markdown.contains("--check-static"));
}

#[test]
fn a_from_commit_tree_adds_the_overlay_dependent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let out = temp.path().join("out");
    let base = args_with(&[
        "--easystack".into(),
        stack().display().to_string(),
        "--easystack".into(),
        from_commit_stack().display().to_string(),
    ]);

    let missing: Vec<String> = base.clone();
    let borrowed: Vec<&str> = missing.iter().map(String::as_str).collect();
    let without = invoke(&borrowed, &out);
    let json = json_of(&without);
    assert!(json["notes"].as_array().unwrap().iter().any(|note| note
        == "from-commit abcdef: no easyconfigs given for this commit, release tree used"));
    assert!(json["unresolved"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item
            .as_str()
            .unwrap()
            .contains("NewLib-2.0-GCCcore-15.2.0.eb")));
    assert!(json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["dependent"] != "NewLib/2.0-GCCcore-15.2.0"));

    let mut with_tree = base;
    with_tree.push("--commit-tree".into());
    with_tree.push(format!(
        "abcdef={}",
        fixtures().join("commit-abc").display()
    ));
    let borrowed: Vec<&str> = with_tree.iter().map(String::as_str).collect();
    let with = invoke(&borrowed, &out);
    let json = json_of(&with);
    assert!(
        json["notes"].as_array().unwrap().is_empty(),
        "{}",
        json["notes"]
    );
    let newlib = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["dependent"] == "NewLib/2.0-GCCcore-15.2.0")
        .expect("NewLib row");
    assert_eq!(newlib["class"], "no-rebuild");
    assert_eq!(newlib["edge_kind"], "runtime");
}

#[test]
fn filter_deps_drops_dependency_tuples_and_keeps_the_extension() {
    let temp = tempfile::tempdir().expect("tempdir");
    let out = temp.path().join("out");
    let args = args_with(&[
        "--easystack".into(),
        stack().display().to_string(),
        "--filter-deps".into(),
        "zlib".into(),
    ]);
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let run = invoke(&borrowed, &out);
    let json = json_of(&run);
    let dependents: Vec<&str> = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["dependent"].as_str().unwrap())
        .collect();
    assert_eq!(
        dependents,
        [
            "Bundle/1.0-GCCcore-15.2.0",
            "PyApp/1.0-GCCcore-15.2.0",
            "zlib/1.3.1-GCCcore-15.2.0",
        ]
    );
    let pyapp = json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["dependent"] == "PyApp/1.0-GCCcore-15.2.0")
        .unwrap();
    assert_eq!(pyapp["class"], "no-rebuild");
}

#[test]
fn an_easyblock_commit_catalog_counts_only_for_the_group_that_named_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let out = temp.path().join("out");
    let commit = "abcdef";
    let classes = temp.path().join("easyblocks/c");
    std::fs::create_dir_all(&classes).expect("classes");
    std::fs::write(
        classes.join("configuremake.py"),
        "class ConfigureMake(object):\n    pass\n",
    )
    .expect("class");

    let plain = temp.path().join("eessi-2026.06-eb-5.4.0-plain.yml");
    std::fs::write(&plain, "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n").expect("plain");
    let other = temp.path().join("side-eb-9.9.9.yml");
    std::fs::write(
        &other,
        format!(
            "easyconfigs:\n  - GCCcore-15.2.0.eb:\n      options:\n        include-easyblocks-from-commit: {commit}\n"
        ),
    )
    .expect("other");

    let shared = vec![
        "--changed".to_string(),
        "zlib-1.3.1-GCCcore-15.2.0.eb".to_string(),
        "--easyconfigs".to_string(),
        fixtures().join("release").display().to_string(),
        "--installed".to_string(),
        fixtures().join("installed.txt").display().to_string(),
        "--easyblocks".to_string(),
        fixtures().join("easyblocks").display().to_string(),
        "--easyblocks-commit".to_string(),
        format!("{commit}={}", temp.path().join("easyblocks").display()),
        "--generated-at".to_string(),
        "2026-10-22T12:00:00Z".to_string(),
    ];

    let mut unnamed = shared.clone();
    unnamed.extend([
        "--easystack".to_string(),
        plain.display().to_string(),
        "--easystack".to_string(),
        other.display().to_string(),
    ]);
    let borrowed: Vec<&str> = unnamed.iter().map(String::as_str).collect();
    let json = json_of(&invoke(&borrowed, &out));
    let notes = notes_of(&json);
    assert!(
        notes
            .iter()
            .any(|note| note == "easyblock ConfigureMake missing"),
        "a commit catalog another group named must not resolve zlib: {notes:?}"
    );
    assert!(
        json["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .any(|row| row["dependent"] == "zlib/1.3.1-GCCcore-15.2.0"),
        "zlib stays in the set"
    );

    let named = temp.path().join("eessi-2026.06-eb-5.4.0-named.yml");
    std::fs::write(
        &named,
        format!(
            "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb:\n      options:\n        include-easyblocks-from-commit: {commit}\n"
        ),
    )
    .expect("named");
    let mut named_args = shared;
    named_args.extend(["--easystack".to_string(), named.display().to_string()]);
    let borrowed: Vec<&str> = named_args.iter().map(String::as_str).collect();
    let json = json_of(&invoke(&borrowed, &out));
    let notes = notes_of(&json);
    assert!(
        notes
            .iter()
            .all(|note| note != "easyblock ConfigureMake missing"),
        "the group that named the commit keeps the class: {notes:?}"
    );
}

fn notes_of(json: &serde_json::Value) -> Vec<String> {
    json["notes"]
        .as_array()
        .expect("notes")
        .iter()
        .map(|note| note.as_str().expect("note").to_string())
        .collect()
}

#[test]
fn a_changed_list_of_only_comments_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let list = temp.path().join("changed.txt");
    std::fs::write(&list, "# nothing\n\n").expect("list");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .arg("rebuild-set")
        .arg("--changed")
        .arg(&list)
        .arg("--easystack")
        .arg(stack())
        .arg("--easyconfigs")
        .arg(fixtures().join("release"))
        .arg("--installed")
        .arg(fixtures().join("installed.txt"))
        .arg("--out-dir")
        .arg(temp.path().join("out"))
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("named no easyconfig"), "{stderr}");
}

fn file_body<'a>(run: &'a Run, name: &str) -> &'a str {
    run.files
        .iter()
        .find(|(found, _)| found == name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .1
        .as_str()
}

fn json_of(run: &Run) -> serde_json::Value {
    serde_json::from_str(file_body(run, "rebuild-set.json")).expect("json")
}

#[test]
fn without_an_easystack_the_installed_modules_are_the_roots() {
    let temp = tempfile::tempdir().expect("tempdir");
    let with_stack = args_with(&["--easystack".into(), stack().display().to_string()]);
    let borrowed: Vec<&str> = with_stack.iter().map(String::as_str).collect();
    let from_stack = json_of(&invoke(&borrowed, &temp.path().join("stack")));

    let plain = common_args();
    let borrowed: Vec<&str> = plain.iter().map(String::as_str).collect();
    let from_installed = json_of(&invoke(&borrowed, &temp.path().join("installed")));

    assert_eq!(
        from_installed["easystacks"],
        serde_json::json!(["installed modules"])
    );
    assert_eq!(from_installed["counts"], from_stack["counts"]);
    let rows = |value: &serde_json::Value| -> Vec<(String, String, String)> {
        value["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .map(|row| {
                (
                    row["changed"].as_str().unwrap().to_string(),
                    row["dependent"].as_str().unwrap().to_string(),
                    row["class"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    assert_eq!(rows(&from_installed), rows(&from_stack));
    assert_eq!(from_installed["counts"]["must-rebuild"], 2);
    assert_eq!(from_installed["counts"]["review"], 2);
    assert_eq!(from_installed["counts"]["no-rebuild"], 4);
}

#[test]
fn a_findings_export_is_ranked_by_score_per_in_place_rebuild() {
    let temp = tempfile::tempdir().expect("tempdir");
    let out = temp.path().join("out");
    let args = [
        "--easyconfigs".to_string(),
        fixtures().join("release").display().to_string(),
        "--installed".into(),
        fixtures().join("installed.txt").display().to_string(),
        "--installed-label".into(),
        "x86_64/amd/zen4".into(),
        "--generated-at".into(),
        "2026-10-22T12:00:00Z".into(),
        "--findings".into(),
        fixtures().join("findings.csv").display().to_string(),
    ];
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let run = invoke(&borrowed, &out);
    assert!(
        run.stdout.contains("changed=2") && run.stdout.contains("findings_modules=2"),
        "{}",
        run.stdout
    );
    let names: Vec<&str> = run.files.iter().map(|(name, _)| name.as_str()).collect();
    for expected in [
        "findings.csv",
        "findings.json",
        "findings.md",
        "rebuild-set.json",
    ] {
        assert!(names.contains(&expected), "{names:?}");
    }

    let value: serde_json::Value =
        serde_json::from_str(file_body(&run, "findings.json")).expect("findings json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["rows"], 3);
    assert_eq!(value["ids"], 3);
    assert_eq!(value["modules"], 2);
    assert_eq!(
        value["not_installed"],
        serde_json::json!(["NotInstalled/1.0-GCCcore-15.2.0"])
    );
    let ranked = value["ranked"].as_array().expect("ranked");
    assert_eq!(ranked[0]["module"], "zlib/1.3.1-GCCcore-15.2.0");
    assert_eq!(ranked[0]["score"], 11);
    assert_eq!(ranked[0]["closure"]["must_rebuild"], 2);
    assert_eq!(ranked[0]["closure"]["review"], 2);
    assert_eq!(ranked[0]["closure"]["runtime"], 4);
    assert_eq!(ranked[0]["closure"]["in_place"], 4);
    assert_eq!(ranked[0]["closure"]["bump"], 8);
    assert_eq!(ranked[0]["value_per_rebuild"], 2.75);
    assert_eq!(ranked[1]["module"], "libpng/1.6.50-GCCcore-15.2.0");
    assert_eq!(ranked[1]["closure"]["in_place"], 2);
    assert_eq!(ranked[1]["closure"]["bump"], 3);
    assert_eq!(
        ranked[1]["closure"]["in_place_dependents"],
        serde_json::json!(["AppStatic/1.0-GCCcore-15.2.0"])
    );

    let totals = value["totals"].as_array().expect("totals");
    assert_eq!(totals[0]["severity"], "Critical");
    assert_eq!(totals[0]["in_place_at_or_above"], 4);
    assert_eq!(totals[0]["bump_at_or_above"], 8);
    assert_eq!(totals[1]["severity"], "High");
    assert_eq!(totals[1]["modules_at_or_above"], 2);
    assert_eq!(totals[1]["in_place_at_or_above"], 5);
    assert_eq!(totals[1]["bump_at_or_above"], 8);

    let csv = file_body(&run, "findings.csv");
    assert!(csv.lines().nth(1).unwrap().starts_with(
        "1,zlib/1.3.1-GCCcore-15.2.0,zlib-1.3.1-GCCcore-15.2.0.eb,Critical,1,0,0,1,0,2,1,0,1,11,2,2,4,4,8,2.75,"
    ));
    assert!(file_body(&run, "findings.md").contains("| Critical | 1 | 1 | 1 | 1 | 4 | 8 |"));
}

#[test]
fn a_findings_export_that_names_no_installed_module_is_a_usage_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let csv = temp.path().join("findings.csv");
    std::fs::write(
        &csv,
        "severity,vuln_id,package,used_in\nHigh,CVE-1,x,Gone/1.0\n",
    )
    .expect("write");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["rebuild-set", "--easyconfigs"])
        .arg(fixtures().join("release"))
        .arg("--installed")
        .arg(fixtures().join("installed.txt"))
        .arg("--findings")
        .arg(&csv)
        .arg("--out-dir")
        .arg(temp.path().join("out"))
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--findings names no installed module")
    );
}
