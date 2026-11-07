//! `stack shard --materialize --build-robot-path` plans over the files
//! EasyBuild's robot reaches on that robot path, and nothing else: a
//! dependency comes from `<root>/<name>/<version>.eb`,
//! `<root>/<name>/<name>-<version>.eb`, `<root>/<letter>/<name>/<name>-<version>.eb`
//! or `<root>/<name>-<version>.eb` of the first root that has one. Generated
//! outputs, archives and other nested copies under a root are never read.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TC: &str = "GCCcore-15.2.0";

fn ec_at(root: &Path, rel: &str, name: &str, version: &str, deps: &[(&str, &str)]) {
    let deps: Vec<String> = deps
        .iter()
        .map(|(n, v)| format!("('{n}', '{v}')"))
        .collect();
    let text = format!(
        "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\nhomepage = 'https://example.org/'\ndescription = 'robot layout test'\ntoolchain = {{'name': 'GCCcore', 'version': '15.2.0'}}\nsource_urls = ['https://example.org/']\nsources = [SOURCE_TAR_GZ]\ndependencies = [{}]\nmoduleclass = 'lib'\n",
        deps.join(", ")
    );
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// `<root>/<letter>/<name>/<name>-<version>-GCCcore-15.2.0.eb`.
fn ec(root: &Path, name: &str, version: &str, deps: &[(&str, &str)]) {
    let letter = name.chars().next().unwrap().to_ascii_lowercase();
    ec_at(
        root,
        &format!("{letter}/{name}/{name}-{version}-{TC}.eb"),
        name,
        version,
        deps,
    );
}

fn file(name: &str, version: &str) -> String {
    format!("{name}-{version}-{TC}.eb")
}

fn shard(dir: &Path, list: &str, easyconfigs: &[&Path], build: &[&Path], extra: &[&str]) -> Output {
    let list_path = dir.join("build.list");
    std::fs::write(&list_path, list).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["stack", "shard", "--build-list"])
        .arg(&list_path)
        .args([
            "--shards",
            "2",
            "--generated-at",
            "2026-11-07T00:00:00Z",
            "--out-dir",
        ])
        .arg(dir.join("out"));
    for root in easyconfigs {
        command.arg("--easyconfigs").arg(root);
    }
    if !build.is_empty() {
        command.arg("--materialize");
        for root in build {
            command.arg("--build-robot-path").arg(root);
        }
    }
    command.args(extra).output().unwrap()
}

fn plan(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("out/plan.json")).unwrap()).unwrap()
}

fn closure(plan: &Value, easyconfig: &str) -> Vec<String> {
    plan["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["easyconfig"] == easyconfig)
        .unwrap_or_else(|| panic!("{easyconfig} not in {plan}"))["closure"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect()
}

fn ok(run: &Output) -> String {
    assert!(
        run.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stderr).into_owned()
}

fn abs(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap()
}

#[test]
fn a_dependency_found_only_in_a_nested_output_tree_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Root", "1.0", &[("Only", "1.0")]);
    ec_at(
        &robot,
        &format!("outputs/x/{}", file("Only", "1.0")),
        "Only",
        "1.0",
        &[],
    );
    let run = shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    );
    assert_eq!(
        run.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("needs Only"), "{stderr}");
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn a_generated_copy_under_the_robot_path_does_not_replace_the_robots_file() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Root", "1.0", &[("Dep", "1.0")]);
    ec(&robot, "Dep", "1.0", &[]);
    ec(&robot, "Extra", "1.0", &[]);
    ec_at(
        &robot,
        &format!(
            "build/plan/sources/v1/0123456789abcdef0123456789abcdef01234567/d/Dep/{}",
            file("Dep", "1.0")
        ),
        "Dep",
        "1.0",
        &[("Extra", "1.0")],
    );
    ok(&shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    ));
    let mut got = closure(&plan(tmp.path()), &file("Root", "1.0"));
    got.sort();
    assert_eq!(got, [format!("Dep/1.0-{TC}"), format!("Root/1.0-{TC}")]);
}

#[test]
fn of_two_layouts_in_one_root_the_earlier_create_paths_layout_wins() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Root", "1.0", &[("Dep", "1.0")]);
    ec(&robot, "Extra", "1.0", &[]);
    // Layout 2, `<root>/Dep/Dep-...`, before layout 3, `<root>/d/Dep/Dep-...`.
    ec_at(
        &robot,
        &format!("Dep/{}", file("Dep", "1.0")),
        "Dep",
        "1.0",
        &[],
    );
    ec(&robot, "Dep", "1.0", &[("Extra", "1.0")]);
    ok(&shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &file("Root", "1.0"));
    assert!(!got.iter().any(|m| m.starts_with("Extra/")), "{got:?}");
}

#[test]
fn the_first_build_robot_path_wins_whatever_the_easyconfigs_order() {
    let tmp = tempfile::tempdir().unwrap();
    let a = abs(&tmp.path().join("a"));
    let b = abs(&tmp.path().join("b"));
    ec(&a, "Root", "1.0", &[("Dep", "1.0")]);
    ec(&a, "Dep", "1.0", &[]);
    ec(&b, "Dep", "1.0", &[("Extra", "1.0")]);
    ec(&b, "Extra", "1.0", &[]);
    // --easyconfigs a b lets b win in the legacy merge; EasyBuild searches a first.
    ok(&shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&a, &b],
        &[&a, &b],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &file("Root", "1.0"));
    assert!(!got.iter().any(|m| m.starts_with("Extra/")), "{got:?}");
    std::fs::remove_dir_all(tmp.path().join("out")).unwrap();
    ok(&shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&a, &b],
        &[&b, &a],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &file("Root", "1.0"));
    assert!(got.contains(&format!("Extra/1.0-{TC}")), "{got:?}");
}

#[test]
fn an_archived_easyconfig_is_not_read_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Root", "1.0", &[("Old", "0.1")]);
    ec(&robot.join("__archive__"), "Old", "0.1", &[]);
    let run = shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    );
    assert_eq!(
        run.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stderr).contains("needs Old"));
    let refused = shard(
        tmp.path(),
        &format!("{} --consider-archived-easyconfigs\n", file("Root", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    );
    assert_eq!(
        refused.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--consider-archived-easyconfigs"));
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.org")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.org")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_commit_supplies_only_what_its_own_layout_reaches_after_the_robot_path() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Dep", "1.0", &[]);
    let work = tmp.path().join("work");
    let tree = work.join("easybuild/easyconfigs");
    ec(
        &tree,
        "Root",
        "1.0",
        &[("Dep", "1.0"), ("NewDep", "1.0"), ("Nested", "1.0")],
    );
    ec(&tree, "Dep", "1.0", &[("Extra", "1.0")]);
    ec(&tree, "Extra", "1.0", &[]);
    ec(&tree, "NewDep", "1.0", &[]);
    ec_at(
        &tree,
        &format!("build/x/{}", file("Nested", "1.0")),
        "Nested",
        "1.0",
        &[],
    );
    git(&work, &["init", "-q"]);
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "c"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    let repo = work.to_str().unwrap().to_string();
    let list = format!("{} --from-commit={sha}\n", file("Root", "1.0"));
    let stderr = ok(&shard(
        tmp.path(),
        &list,
        &[&robot],
        &[&robot],
        &["--easyconfigs-repo", &repo],
    ));
    let first = plan(tmp.path());
    let line = &first["lines"][0];
    // Nested-1.0 is in the commit only below build/: EasyBuild cannot find it.
    assert!(
        line["opaque_reason"]
            .as_str()
            .unwrap_or("")
            .contains("Nested"),
        "{line} {stderr}"
    );

    // With Nested at its letter path the commit supplies it and NewDep; Dep
    // still comes from the robot path, so Extra is not built.
    ec(&tree, "Nested", "1.0", &[]);
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "d"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    std::fs::remove_dir_all(tmp.path().join("out")).unwrap();
    let list = format!("{} --from-commit={sha}\n", file("Root", "1.0"));
    ok(&shard(
        tmp.path(),
        &list,
        &[&robot],
        &[&robot],
        &["--easyconfigs-repo", &repo],
    ));
    let mut got = closure(&plan(tmp.path()), &file("Root", "1.0"));
    got.sort();
    assert_eq!(
        got,
        [
            format!("Dep/1.0-{TC}"),
            format!("Nested/1.0-{TC}"),
            format!("NewDep/1.0-{TC}"),
            format!("Root/1.0-{TC}")
        ]
    );
}

#[test]
fn a_line_naming_a_file_two_places_under_one_root_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Dup", "1.0", &[]);
    ec_at(
        &robot,
        &format!("Dup/{}", file("Dup", "1.0")),
        "Dup",
        "1.0",
        &[],
    );
    let run = shard(
        tmp.path(),
        &format!("{}\n", file("Dup", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    );
    assert_ne!(run.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("not defined"), "{stderr}");
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn a_build_robot_path_the_plan_cannot_read_is_a_usage_error() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    let other = abs(&tmp.path().join("other"));
    ec(&robot, "Root", "1.0", &[]);
    ec(&other, "Root", "1.0", &[]);
    let list = format!("{}\n", file("Root", "1.0"));
    for build in [
        vec![other.as_path()],
        vec![robot.as_path(), other.as_path()],
    ] {
        let run = shard(tmp.path(), &list, &[&robot], &build, &[]);
        assert_eq!(
            run.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(String::from_utf8_lossy(&run.stderr).contains("same directories as --easyconfigs"));
    }
    let absent = abs(&tmp.path().join("absent"));
    let run = shard(tmp.path(), &list, &[&robot], &[&absent], &[]);
    assert_eq!(run.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&run.stderr).contains("not a directory"));
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn a_version_named_file_hides_the_robots_file_and_the_dependency_is_missing() {
    // EasyBuild finds `<root>/Dep/1.0-...eb` first and then refuses it, since
    // its name is not `Dep-1.0-...eb`; it does not go on to `<root>/d/Dep/`.
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    ec(&robot, "Root", "1.0", &[("Dep", "1.0")]);
    ec(&robot, "Dep", "1.0", &[]);
    ec_at(&robot, &format!("Dep/1.0-{TC}.eb"), "Dep", "1.0", &[]);
    let run = shard(
        tmp.path(),
        &format!("{}\n", file("Root", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    );
    assert_eq!(
        run.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("needs Dep"), "{stderr}");
    // A plan that does not need Dep still names the refused file.
    ec(&robot, "Other", "1.0", &[]);
    let stderr = ok(&shard(
        tmp.path(),
        &format!("{}\n", file("Other", "1.0")),
        &[&robot],
        &[&robot],
        &[],
    ));
    assert!(stderr.contains("refuses it"), "{stderr}");
    assert!(stderr.contains(&format!("Dep/1.0-{TC}.eb")), "{stderr}");
}
