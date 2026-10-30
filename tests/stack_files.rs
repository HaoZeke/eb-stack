//! Patch and source presence, seeded from EESSI software-layer bot failures.
//!
//! Each fixture directory is one pull request: 1842 pinned an easyconfigs pull
//! request head that lacked an extension patch, 1398 hit the component patch
//! lookup of easybuild-framework#5133, and 1048 rebuilt from a reprod
//! easyconfig whose new patch was in neither the reprod directory nor the
//! robot path.

use std::path::{Path, PathBuf};
use std::process::Command;

const PR1842_HEAD: &str = "97a4c1a9c45b3bbbea67e085ed187055f473e757";
const PR1842_MERGE: &str = "583754772b7f526dda3a4f1443bbbd5953f7d8ea";
const PR1398_PIN: &str = "7e74f85c32686a6c456d4978430de35751d6cefd";
const PR1048_PREFIX: &str =
    "/cvmfs/software.eessi.io/versions/2023.06/software/linux/x86_64/intel/haswell/software";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/stack_files_fixtures")
        .join(name)
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
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

fn files(args: &[&str]) -> (Option<i32>, String, String) {
    let mut all = vec!["stack", "files"];
    all.extend_from_slice(args);
    let output = run(&all);
    (
        output.status.code(),
        text(&output.stdout),
        text(&output.stderr),
    )
}

fn commit_tree(commit: &str, dir: &str) -> String {
    format!("{commit}={}", arg(&fixture(dir)))
}

const PR1842_MISSING: &str = "Python-3.10.8-GCCcore-12.2.0.eb (from-commit 97a4c1a9c45b): patch pip-20.3.4_remove-emSign-root-cert.patch (extension pip 22.3.1) is not beside the easyconfig in p/Python/ at 97a4c1a9c45b or under p/Python/ on the robot path";

#[test]
fn the_pr_head_pin_of_1842_lacks_the_pip_extension_patch() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture(
            "pr1842/easystacks/20261022-eb-5.4.1-2022b-emSign-cert.yml",
        )),
        "--easyconfigs",
        arg(&fixture("pr1842/robot-5.4.1")),
        "--commit-tree",
        &commit_tree(PR1842_HEAD, "pr1842/commit-97a4c1a"),
    ]);
    assert_eq!(code, Some(1), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains(PR1842_MISSING), "{stdout}");
    assert!(
        stdout.contains("missing=1 no_url=0 unchecked=0 easyconfigs=1 files=4\n"),
        "{stdout}"
    );
    assert!(stderr.contains(PR1842_MISSING), "{stderr}");
}

#[test]
fn the_merge_commit_pin_of_1842_has_every_patch() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture(
            "pr1842/easystacks/20261022-eb-5.4.1-2022b-emSign-cert-merge.yml",
        )),
        "--easyconfigs",
        arg(&fixture("pr1842/robot-5.4.1")),
        "--commit-tree",
        &commit_tree(PR1842_MERGE, "pr1842/commit-5837547"),
    ]);
    assert_eq!(code, Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(
        stdout,
        "missing=0 no_url=0 unchecked=0 easyconfigs=1 files=4\n"
    );
}

#[test]
fn a_pin_without_a_tree_is_unchecked_rather_than_passed() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture(
            "pr1842/easystacks/20261022-eb-5.4.1-2022b-emSign-cert.yml",
        )),
        "--easyconfigs",
        arg(&fixture("pr1842/robot-5.4.1")),
    ]);
    assert_eq!(code, Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains(&format!(
            "Python-3.10.8-GCCcore-12.2.0.eb (from-commit 97a4c1a9c45b): unchecked, no --commit-tree or --easyconfigs-repo has commit {PR1842_HEAD}"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("unchecked=1"), "{stdout}");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.org")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.org")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        text(&output.stderr)
    );
    text(&output.stdout).trim().to_string()
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn commit_fixture(repo: &Path, dir: &str) -> String {
    let easyconfigs = repo.join("easybuild/easyconfigs");
    if easyconfigs.exists() {
        std::fs::remove_dir_all(&easyconfigs).unwrap();
    }
    std::fs::create_dir_all(&easyconfigs).unwrap();
    copy_tree(&fixture(dir), &easyconfigs);
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", dir]);
    git(repo, &["rev-parse", "HEAD"])
}

#[test]
fn easyconfigs_repo_reads_each_pin_from_git_history() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("easybuild-easyconfigs");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    let head = commit_fixture(&repo, "pr1842/commit-97a4c1a");
    let merge = commit_fixture(&repo, "pr1842/commit-5837547");
    let easystack = temp.path().join("easystack.yml");
    let robot = fixture("pr1842/robot-5.4.1");

    for (pin, expected) in [(&head, Some(1)), (&merge, Some(0))] {
        std::fs::write(
            &easystack,
            format!(
                "easyconfigs:\n  - Python-3.10.8-GCCcore-12.2.0.eb:\n      options:\n        from-commit: {pin}\n"
            ),
        )
        .unwrap();
        let (code, stdout, stderr) = files(&[
            "--easystack",
            arg(&easystack),
            "--easyconfigs",
            arg(&robot),
            "--easyconfigs-repo",
            arg(&repo),
        ]);
        assert_eq!(code, expected, "stdout:\n{stdout}\nstderr:\n{stderr}");
        let flagged = stdout.contains(
            "patch pip-20.3.4_remove-emSign-root-cert.patch (extension pip 22.3.1) is not beside the easyconfig in p/Python/",
        );
        assert_eq!(flagged, expected == Some(1), "{stdout}");
    }
}

fn pr1398_component_line(patch: &str) -> String {
    format!(
        "Xfce-4.20-GCCcore-14.3.0.eb (from-commit 7e74f85c3268): patch {patch} (component xfce4-terminal 1.1.5) is beside the easyconfig in x/Xfce/ at 7e74f85c3268, but for a from-commit easyconfig the component install looks only under x/xfce4-terminal/ on the robot path (easybuild-framework#5133)"
    )
}

#[test]
fn from_commit_component_patches_of_1398_hit_framework_5133() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture(
            "pr1398/easystacks/eessi-2025.06-eb-5.2.0-2025b.yml",
        )),
        "--easyconfigs",
        arg(&fixture("pr1398/robot-5.2.0")),
        "--commit-tree",
        &commit_tree(PR1398_PIN, "pr1398/commit-7e74f85"),
    ]);
    assert_eq!(code, Some(1), "stdout:\n{stdout}\nstderr:\n{stderr}");
    for patch in [
        "xfce-terminal-1.1.5_fix_version.patch",
        "xfce-terminal-1.1.5_fix_doc.patch",
    ] {
        assert!(stdout.contains(&pr1398_component_line(patch)), "{stdout}");
    }
    assert!(stdout.contains("missing=2 "), "{stdout}");
}

#[test]
fn the_1398_workaround_of_a_sourcepath_copy_clears_it() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture(
            "pr1398/easystacks/eessi-2025.06-eb-5.2.0-2025b.yml",
        )),
        "--easyconfigs",
        arg(&fixture("pr1398/robot-5.2.0")),
        "--commit-tree",
        &commit_tree(PR1398_PIN, "pr1398/commit-7e74f85"),
        "--sourcepath",
        arg(&fixture("pr1398/sourcepath")),
    ]);
    assert_eq!(code, Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(
        stdout,
        "missing=0 no_url=0 unchecked=0 easyconfigs=1 files=2\n"
    );
}

#[test]
fn the_same_bundle_on_the_robot_path_needs_no_workaround() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture("pr1398/easystacks/xfce-robot.yml")),
        "--easyconfigs",
        arg(&fixture("pr1398/robot-develop")),
    ]);
    assert_eq!(code, Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(
        stdout,
        "missing=0 no_url=0 unchecked=0 easyconfigs=1 files=2\n"
    );
}

fn pr1048_map() -> String {
    format!("/cvmfs={}", arg(&fixture("pr1048/cvmfs")))
}

#[test]
fn the_1048_reprod_easyconfig_needs_a_patch_eb_4_9_4_lacks() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture("pr1048/easystacks/015-eb-4.9.4.yml")),
        "--easyconfigs",
        arg(&fixture("pr1048/robot-4.9.4")),
        "--path-map",
        &pr1048_map(),
    ]);
    assert_eq!(code, Some(1), "stdout:\n{stdout}\nstderr:\n{stderr}");
    let expected = format!(
        "SciPy-bundle-2023.07-gfbf-2023a.eb: patch scipy-1.11.1_vectorization_error.patch (extension scipy 1.11.1) is not beside the easyconfig in {PR1048_PREFIX}/SciPy-bundle/2023.07-gfbf-2023a/easybuild/ or under s/SciPy-bundle/ on the robot path"
    );
    assert!(stdout.contains(&expected), "{stdout}");
    assert!(stdout.contains("missing=1 "), "{stdout}");
}

#[test]
fn the_1048_reprod_easyconfig_passes_on_a_robot_path_with_the_patch() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture("pr1048/easystacks/015-eb-4.9.4.yml")),
        "--easyconfigs",
        arg(&fixture("pr1048/robot-develop")),
        "--path-map",
        &pr1048_map(),
    ]);
    assert_eq!(code, Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(
        stdout,
        "missing=0 no_url=0 unchecked=0 easyconfigs=1 files=3\n"
    );
}

#[test]
fn an_unreadable_absolute_path_is_unchecked() {
    let (code, stdout, stderr) = files(&[
        "--easystack",
        arg(&fixture("pr1048/easystacks/015-eb-4.9.4.yml")),
        "--easyconfigs",
        arg(&fixture("pr1048/robot-4.9.4")),
    ]);
    assert_eq!(code, Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains("is not readable here; --path-map maps its prefix to a local copy"),
        "{stdout}"
    );
    assert!(stdout.contains("unchecked=1"), "{stdout}");
}

#[test]
fn the_mcp_tool_reports_the_same_missing_patch() {
    let call = |easystack: &str, tree: String| {
        eb_stack::mcp::handle_message(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "eb_stack_files",
                "arguments": {
                    "easystack": [arg(&fixture(easystack))],
                    "easyconfigs": [arg(&fixture("pr1842/robot-5.4.1"))],
                    "commit_tree": [tree],
                }
            }
        }))
        .expect("tool response")
    };
    let failed = call(
        "pr1842/easystacks/20261022-eb-5.4.1-2022b-emSign-cert.yml",
        commit_tree(PR1842_HEAD, "pr1842/commit-97a4c1a"),
    );
    assert!(failed.to_string().contains(PR1842_MISSING), "{failed}");
    assert_eq!(failed["result"]["isError"], true, "{failed}");

    let clean = call(
        "pr1842/easystacks/20261022-eb-5.4.1-2022b-emSign-cert-merge.yml",
        commit_tree(PR1842_MERGE, "pr1842/commit-5837547"),
    );
    let body = &clean["result"]["structuredContent"];
    assert_eq!(body["missing"], serde_json::json!([]), "{clean}");
    assert_eq!(body["easyconfigs"], 1, "{clean}");
    assert_eq!(body["files"], 4, "{clean}");
}
