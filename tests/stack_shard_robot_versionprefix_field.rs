//! CLI regressions for first-class `Candidate.versionprefix`: from-pr
//! create_paths Dir fallback, merge identity without folding bare `version`,
//! EB5.4.2 dummy module keys, with and without `--materialize` /
//! `--build-robot-path`. Additive — does not change shipped assertions.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TC: &str = "GCCcore-15.2.0";

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

fn ok(run: &Output) -> String {
    assert!(
        run.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stderr).into_owned()
}

fn plan(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("out/plan.json")).unwrap()).unwrap()
}

fn abs(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn ec_gcc(name: &str, version: &str, versionprefix: &str, deps: &str, description: &str) -> String {
    let prefix_line = if versionprefix.is_empty() {
        String::new()
    } else {
        format!("versionprefix = '{versionprefix}'\n")
    };
    format!(
        "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\n{prefix_line}homepage = 'https://example.org/'\ndescription = '{description}'\ntoolchain = {{'name': 'GCCcore', 'version': '15.2.0'}}\nsource_urls = ['https://example.org/']\nsources = [SOURCE_TAR_GZ]\ndependencies = [{deps}]\nmoduleclass = 'lib'\n"
    )
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

#[test]
fn from_pr_dir_fallback_matches_create_paths_not_nested_basename() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    let pr = abs(&tmp.path().join("pr"));
    let root_ec = ec_gcc("Root", "1.0", "", "('Nested', '1.0')", "t");
    write(&robot.join(format!("r/Root/Root-1.0-{TC}.eb")), &root_ec);
    write(
        &pr.join(format!("build/x/Nested-1.0-{TC}.eb")),
        &ec_gcc("Nested", "1.0", "", "", "nested only"),
    );
    write(&pr.join(format!("r/Root/Root-1.0-{TC}.eb")), &root_ec);
    let list = format!("Root-1.0-{TC}.eb --from-pr=1\n");
    ok(&shard(
        tmp.path(),
        &list,
        &[&robot],
        &[&robot],
        &["--pr-tree", &format!("1={}", pr.display())],
    ));
    let line = &plan(tmp.path())["lines"][0];
    assert!(line["opaque"].as_str().is_some(), "{line}");
    assert!(
        line["opaque_reason"]
            .as_str()
            .unwrap_or("")
            .contains("Nested"),
        "{line}"
    );
}

#[test]
fn from_commit_dir_fallback_same_create_paths_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    let commit = abs(&tmp.path().join("commit"));
    let root_ec = ec_gcc("Root", "1.0", "", "('Nested', '1.0')", "t");
    write(&robot.join(format!("r/Root/Root-1.0-{TC}.eb")), &root_ec);
    write(
        &commit.join(format!("build/x/Nested-1.0-{TC}.eb")),
        &ec_gcc("Nested", "1.0", "", "", "nested only"),
    );
    write(&commit.join(format!("r/Root/Root-1.0-{TC}.eb")), &root_ec);
    let list = format!("Root-1.0-{TC}.eb --from-commit=deadbeef\n");
    ok(&shard(
        tmp.path(),
        &list,
        &[&robot],
        &[&robot],
        &["--commit-tree", &format!("deadbeef={}", commit.display())],
    ));
    let line = &plan(tmp.path())["lines"][0];
    assert!(line["opaque"].as_str().is_some(), "{line}");
}

#[test]
fn versionprefix_variant_does_not_steal_plain_with_effective_robot() {
    let tmp = tempfile::tempdir().unwrap();
    let a = abs(&tmp.path().join("a"));
    let b = abs(&tmp.path().join("b"));
    write(
        &a.join(format!("m/Mol/Mol-mpp-1.0-{TC}.eb")),
        &ec_gcc("Mol", "1.0", "mpp-", "('Extra', '1.0')", "prefixed"),
    );
    write(
        &a.join(format!("e/Extra/Extra-1.0-{TC}.eb")),
        &ec_gcc("Extra", "1.0", "", "", "extra"),
    );
    write(
        &a.join(format!("r/Root/Root-1.0-{TC}.eb")),
        &ec_gcc("Root", "1.0", "", "('Mol', '1.0')", "root"),
    );
    write(
        &b.join(format!("m/Mol/Mol-1.0-{TC}.eb")),
        &ec_gcc("Mol", "1.0", "", "", "plain"),
    );
    write(
        &b.join(format!("r/Root/Root-1.0-{TC}.eb")),
        &ec_gcc("Root", "1.0", "", "('Mol', '1.0')", "root"),
    );
    ok(&shard(
        tmp.path(),
        &format!("Root-1.0-{TC}.eb\n"),
        &[&a, &b],
        &[&a, &b],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &format!("Root-1.0-{TC}.eb"));
    assert!(
        !got.iter().any(|m| m.starts_with("Extra/")),
        "Extra must not enter the closure; EB finds B's plain Mol: {got:?}"
    );
    assert!(
        got.iter().any(|m| m == &format!("Mol/1.0-{TC}")),
        "plain Mol module expected: {got:?}"
    );
}

#[test]
fn versionprefix_variants_remain_distinct_without_effective_robot() {
    // Without --materialize, merge identity still keeps mpp- and plain apart
    // (bare version stays 1.0; versionprefix differs).
    let tmp = tempfile::tempdir().unwrap();
    let a = abs(&tmp.path().join("a"));
    let b = abs(&tmp.path().join("b"));
    write(
        &a.join(format!("m/Mol/Mol-mpp-1.0-{TC}.eb")),
        &ec_gcc("Mol", "1.0", "mpp-", "", "prefixed"),
    );
    write(
        &b.join(format!("m/Mol/Mol-1.0-{TC}.eb")),
        &ec_gcc("Mol", "1.0", "", "", "plain"),
    );
    write(
        &b.join(format!("r/Root/Root-1.0-{TC}.eb")),
        &ec_gcc("Root", "1.0", "", "('Mol', '1.0')", "root"),
    );
    // Named explicit file: the plain Mol on B.
    ok(&shard(
        tmp.path(),
        &format!("Mol-1.0-{TC}.eb\n"),
        &[&a, &b],
        &[],
        &[],
    ));
    let line = &plan(tmp.path())["lines"][0];
    assert_eq!(
        line["module"].as_str().unwrap(),
        format!("Mol/1.0-{TC}"),
        "{line}"
    );
}

#[test]
fn coincident_versionprefix_module_uses_eb_install_stem() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    write(
        &robot.join(format!("m/Mol/Mol-mpp-mpp-1.0-{TC}.eb")),
        &ec_gcc("Mol", "mpp-1.0", "mpp-", "", "coincident"),
    );
    let list = format!("Mol-mpp-mpp-1.0-{TC}.eb\n");
    ok(&shard(tmp.path(), &list, &[&robot], &[&robot], &[]));
    let line = &plan(tmp.path())["lines"][0];
    assert_eq!(
        line["module"].as_str().unwrap(),
        format!("Mol/mpp-mpp-1.0-{TC}"),
        "{line}"
    );
}

#[test]
fn dummy_toolchain_plan_module_matches_eb5_det_full_ec_version() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    write(
        &robot.join("d/Daisy/Daisy-1-dummy-dummy.eb"),
        "easyblock = 'ConfigureMake'\nname = 'Daisy'\nversion = '1'\ntoolchain = {'name': 'dummy', 'version': 'dummy'}\nhomepage = 'https://example.org/'\ndescription = 'dummy'\nsource_urls = ['https://example.org/']\nsources = [SOURCE_TAR_GZ]\ndependencies = []\nmoduleclass = 'lib'\n",
    );
    ok(&shard(
        tmp.path(),
        "Daisy-1-dummy-dummy.eb\n",
        &[&robot],
        &[&robot],
        &[],
    ));
    assert_eq!(
        plan(tmp.path())["lines"][0]["module"].as_str().unwrap(),
        "Daisy/1-dummy-dummy"
    );
}

#[test]
fn explicit_named_file_works_without_build_robot_path() {
    let tmp = tempfile::tempdir().unwrap();
    let robot = abs(&tmp.path().join("robot"));
    write(
        &robot.join(format!("r/Root/Root-1.0-{TC}.eb")),
        &ec_gcc("Root", "1.0", "", "", "root"),
    );
    ok(&shard(
        tmp.path(),
        &format!("Root-1.0-{TC}.eb\n"),
        &[&robot],
        &[],
        &[],
    ));
    assert_eq!(
        plan(tmp.path())["lines"][0]["module"].as_str().unwrap(),
        format!("Root/1.0-{TC}")
    );
}

fn mol_prefix_graph(root: &Path) -> (PathBuf, PathBuf) {
    let tree = abs(&root.join("tree"));
    write(
        &tree.join(format!("m/Mol/Mol-mpp-1.0-{TC}.eb")),
        &ec_gcc("Mol", "1.0", "mpp-", "", "prefixed"),
    );
    write(
        &tree.join(format!("m/Mol/Mol-1.0-{TC}.eb")),
        &ec_gcc("Mol", "1.0", "", "", "plain"),
    );
    write(
        &tree.join(format!("r/RootPlain/RootPlain-1.0-{TC}.eb")),
        &ec_gcc("RootPlain", "1.0", "", "('Mol', '1.0')", "wants plain"),
    );
    write(
        &tree.join(format!("r/RootPref/RootPref-1.0-{TC}.eb")),
        &ec_gcc(
            "RootPref",
            "1.0",
            "",
            "('Mol', 'mpp-1.0')",
            "wants prefixed",
        ),
    );
    (tree.clone(), tree)
}

#[test]
fn dep_1_0_selects_plain_mol_with_effective_robot() {
    let tmp = tempfile::tempdir().unwrap();
    let (tree, _) = mol_prefix_graph(tmp.path());
    ok(&shard(
        tmp.path(),
        &format!("RootPlain-1.0-{TC}.eb\n"),
        &[&tree],
        &[&tree],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &format!("RootPlain-1.0-{TC}.eb"));
    assert!(
        got.iter().any(|m| m == &format!("Mol/1.0-{TC}")),
        "plain Mol expected: {got:?}"
    );
    assert!(
        !got.iter().any(|m| m.contains("mpp-")),
        "prefixed Mol must not be selected for dep 1.0: {got:?}"
    );
}

#[test]
fn dep_1_0_selects_plain_mol_without_effective_robot() {
    let tmp = tempfile::tempdir().unwrap();
    let (tree, _) = mol_prefix_graph(tmp.path());
    ok(&shard(
        tmp.path(),
        &format!("RootPlain-1.0-{TC}.eb\n"),
        &[&tree],
        &[],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &format!("RootPlain-1.0-{TC}.eb"));
    assert!(
        got.iter().any(|m| m == &format!("Mol/1.0-{TC}")),
        "plain Mol expected: {got:?}"
    );
    assert!(
        !got.iter().any(|m| m.contains("mpp-")),
        "prefixed Mol must not be selected for dep 1.0: {got:?}"
    );
}

#[test]
fn dep_mpp_1_0_selects_prefixed_mol_with_effective_robot() {
    let tmp = tempfile::tempdir().unwrap();
    let (tree, _) = mol_prefix_graph(tmp.path());
    ok(&shard(
        tmp.path(),
        &format!("RootPref-1.0-{TC}.eb\n"),
        &[&tree],
        &[&tree],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &format!("RootPref-1.0-{TC}.eb"));
    assert!(
        got.iter().any(|m| m == &format!("Mol/mpp-1.0-{TC}")),
        "prefixed Mol expected: {got:?}"
    );
    assert!(
        !got.iter().any(|m| m == &format!("Mol/1.0-{TC}")),
        "plain Mol must not be selected for dep mpp-1.0: {got:?}"
    );
}

#[test]
fn dep_mpp_1_0_selects_prefixed_mol_without_effective_robot() {
    let tmp = tempfile::tempdir().unwrap();
    let (tree, _) = mol_prefix_graph(tmp.path());
    ok(&shard(
        tmp.path(),
        &format!("RootPref-1.0-{TC}.eb\n"),
        &[&tree],
        &[],
        &[],
    ));
    let got = closure(&plan(tmp.path()), &format!("RootPref-1.0-{TC}.eb"));
    assert!(
        got.iter().any(|m| m == &format!("Mol/mpp-1.0-{TC}")),
        "prefixed Mol expected: {got:?}"
    );
    assert!(
        !got.iter().any(|m| m == &format!("Mol/1.0-{TC}")),
        "plain Mol must not be selected for dep mpp-1.0: {got:?}"
    );
}
