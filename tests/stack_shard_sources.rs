//! `stack shard` on `--from-commit` lines: EasyBuild's precedence, a commit
//! read from a git repository, and `--materialize`.
//!
//! The robot path has `Root-1.0` and `Dep-1.0`. The commit has its own
//! `Root-1.0` (which also needs `New-2.0`), a different `Dep-1.0` (which
//! needs `Extra-1.0`), `New-2.0`, and `Extra-1.0`. EasyBuild builds the
//! commit's `Root-1.0`, the robot path's `Dep-1.0`, and the commit's
//! `New-2.0`; it never reads `Extra-1.0`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn ec(root: &Path, name: &str, version: &str, deps: &[(&str, &str)], patches: &[&str]) -> String {
    let letter = name.chars().next().unwrap().to_ascii_lowercase();
    let file = format!("{name}-{version}-GCCcore-15.2.0.eb");
    let rel = format!("{letter}/{name}/{file}");
    let deps: Vec<String> = deps
        .iter()
        .map(|(n, v)| format!("('{n}', '{v}')"))
        .collect();
    let patches: Vec<String> = patches.iter().map(|p| format!("'{p}'")).collect();
    let text = format!(
        "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\nhomepage = 'https://example.org/'\ndescription = 'source reuse test'\ntoolchain = {{'name': 'GCCcore', 'version': '15.2.0'}}\nsource_urls = ['https://example.org/']\nsources = [SOURCE_TAR_GZ]\npatches = [{}]\ndependencies = [{}]\nmoduleclass = 'lib'\n",
        patches.join(", "),
        deps.join(", ")
    );
    write(&root.join(&rel), &text);
    rel
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
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
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    robot: PathBuf,
    /// The commit's `easybuild/easyconfigs`, as `--commit-tree` takes it.
    tree: PathBuf,
    /// A bare clone holding the commit, as `--easyconfigs-repo` takes it.
    bare: PathBuf,
    sha: String,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let robot = root.join("robot");
    ec(&robot, "Root", "1.0", &[("Dep", "1.0")], &[]);
    ec(&robot, "Dep", "1.0", &[], &["Dep-1.0_shared.patch"]);
    write(&robot.join("d/Dep/Dep-1.0_shared.patch"), "robot patch\n");
    ec(&robot, "Plain", "1.0", &[], &[]);

    let work = root.join("work");
    let tree = work.join("easybuild/easyconfigs");
    ec(
        &tree,
        "Root",
        "1.0",
        &[("Dep", "1.0"), ("New", "2.0")],
        &["Root-1.0_fix.patch"],
    );
    write(&tree.join("r/Root/Root-1.0_fix.patch"), "root patch\n");
    ec(
        &tree,
        "Dep",
        "1.0",
        &[("Extra", "1.0")],
        &["Dep-1.0_shared.patch"],
    );
    write(&tree.join("d/Dep/Dep-1.0_shared.patch"), "commit patch\n");
    ec(&tree, "New", "2.0", &[], &["New-2.0_only.patch"]);
    write(&tree.join("n/New/New-2.0_only.patch"), "new patch\n");
    write(&tree.join("n/New/New-1.0_unused.patch"), "not read\n");
    ec(&tree, "Extra", "1.0", &[], &[]);
    ec(&tree, "Plain", "1.0", &[], &[]);
    git(&work, &["init", "-q"]);
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "commit"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    let bare = root.join("bare.git");
    git(
        &root,
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    Fixture {
        _tmp: tmp,
        root,
        robot,
        tree,
        bare,
        sha,
    }
}

fn shard(f: &Fixture, list: &str, out: &Path, extra: &[&str]) -> Output {
    let list_path = f.root.join("build.list");
    std::fs::write(&list_path, list).unwrap();
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["stack", "shard", "--build-list"])
        .arg(&list_path)
        .arg("--easyconfigs")
        .arg(&f.robot)
        .args([
            "--shards",
            "4",
            "--generated-at",
            "2026-11-07T00:00:00Z",
            "--out-dir",
        ])
        .arg(out)
        .args(extra)
        .output()
        .unwrap()
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

fn read(path: PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn plan(out: &Path) -> Value {
    serde_json::from_str(&read(out.join("plan.json"))).unwrap()
}

fn listing(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_dir() {
                stack.push(entry.path());
            } else {
                out.push(
                    entry
                        .path()
                        .strip_prefix(dir)
                        .unwrap()
                        .display()
                        .to_string(),
                );
            }
        }
    }
    out.sort();
    out
}

fn root_line(f: &Fixture) -> String {
    format!(
        "Root-1.0-GCCcore-15.2.0.eb --from-commit={} --include-systems=snel-zen2\nPlain-1.0-GCCcore-15.2.0.eb\n",
        f.sha
    )
}

fn closure(plan: &Value, file: &str) -> Vec<String> {
    plan["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["easyconfig"] == file)
        .unwrap_or_else(|| panic!("{file} not in {plan}"))["closure"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_named_root_comes_from_the_commit_and_dependencies_from_the_robot_path() {
    let f = fixture();
    let out = f.root.join("out");
    let tree = format!("{}={}", f.sha, f.tree.display());
    let stderr = ok(&shard(&f, &root_line(&f), &out, &["--commit-tree", &tree]));
    let plan = plan(&out);
    let mut got = closure(&plan, "Root-1.0-GCCcore-15.2.0.eb");
    got.sort();
    // The commit's Root needs New; the robot path's Dep needs no Extra.
    assert_eq!(
        got,
        [
            "Dep/1.0-GCCcore-15.2.0",
            "New/2.0-GCCcore-15.2.0",
            "Root/1.0-GCCcore-15.2.0"
        ]
    );
    let shadowed = plan["shadowed"].as_array().expect("shadowed list");
    assert_eq!(shadowed.len(), 1, "{plan}");
    assert_eq!(shadowed[0]["module"], "Dep/1.0-GCCcore-15.2.0");
    assert_eq!(shadowed[0]["robot_easyconfig"], "Dep-1.0-GCCcore-15.2.0.eb");
    assert_eq!(shadowed[0]["source"], format!("--from-commit={}", f.sha));
    assert!(
        stderr.contains(
            "Dep/1.0-GCCcore-15.2.0 builds from the robot path's Dep-1.0-GCCcore-15.2.0.eb"
        ),
        "{stderr}"
    );
    // The prelude-free plan keeps the verbatim lines.
    assert!(!out.join("prelude.local.list").exists());
    assert!(!out.join("sources").exists());
}

#[test]
fn a_root_the_commit_lacks_comes_from_the_robot_path() {
    let f = fixture();
    let out = f.root.join("out");
    let tree = format!("{}={}", f.sha, f.tree.display());
    std::fs::remove_file(f.tree.join("r/Root/Root-1.0-GCCcore-15.2.0.eb")).unwrap();
    ok(&shard(&f, &root_line(&f), &out, &["--commit-tree", &tree]));
    let mut got = closure(&plan(&out), "Root-1.0-GCCcore-15.2.0.eb");
    got.sort();
    assert_eq!(got, ["Dep/1.0-GCCcore-15.2.0", "Root/1.0-GCCcore-15.2.0"]);
}

#[test]
fn a_git_repository_gives_the_same_plan_as_the_commit_directory() {
    let f = fixture();
    let by_dir = f.root.join("by-dir");
    let by_git = f.root.join("by-git");
    let tree = format!("{}={}", f.sha, f.tree.display());
    let dir_err = ok(&shard(
        &f,
        &root_line(&f),
        &by_dir,
        &["--commit-tree", &tree],
    ));
    let git_err = ok(&shard(
        &f,
        &root_line(&f),
        &by_git,
        &["--easyconfigs-repo", f.bare.to_str().unwrap()],
    ));
    assert_eq!(dir_err, git_err);
    assert_eq!(listing(&by_dir), listing(&by_git));
    for name in listing(&by_dir) {
        assert_eq!(read(by_dir.join(&name)), read(by_git.join(&name)), "{name}");
    }
    // The repository was read in place: nothing but the plan was written.
    assert_eq!(
        listing(&f.root.join("by-git")),
        [
            "plan.json",
            "prelude.list",
            "shard-01.list",
            "shard-02.list"
        ]
    );
}

#[test]
fn an_abbreviated_commit_is_resolved_in_the_repository() {
    let f = fixture();
    let out = f.root.join("out");
    let list = format!(
        "Root-1.0-GCCcore-15.2.0.eb --from-commit={}\n",
        &f.sha[..10]
    );
    ok(&shard(
        &f,
        &list,
        &out,
        &["--easyconfigs-repo", f.bare.to_str().unwrap()],
    ));
    let plan = plan(&out);
    assert!(plan["inputs"]["trees"][format!("--from-commit={}", &f.sha[..10])].is_null());
    assert!(closure(&plan, "Root-1.0-GCCcore-15.2.0.eb")
        .contains(&"New/2.0-GCCcore-15.2.0".to_string()));
}

#[test]
fn a_commit_the_repository_lacks_is_an_opaque_line_without_a_cache() {
    let f = fixture();
    let out = f.root.join("out");
    let missing = "1111111111111111111111111111111111111111";
    let list = format!("Root-1.0-GCCcore-15.2.0.eb --from-commit={missing}\n");
    let stderr = ok(&shard(
        &f,
        &list,
        &out,
        &["--easyconfigs-repo", f.bare.to_str().unwrap()],
    ));
    let plan = plan(&out);
    let why = plan["inputs"]["trees"][format!("--from-commit={missing}")]
        .as_str()
        .unwrap();
    assert!(why.contains("is not in --easyconfigs-repo"), "{why}");
    assert!(
        stderr.contains("closure over the robot path only"),
        "{stderr}"
    );
}

#[test]
fn a_path_that_is_not_a_repository_is_a_usage_error() {
    let f = fixture();
    let out = f.root.join("out");
    let run = shard(
        &f,
        &root_line(&f),
        &out,
        &["--easyconfigs-repo", f.robot.to_str().unwrap()],
    );
    assert_eq!(
        run.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!out.exists());
}

fn materialized(f: &Fixture, list: &str, out: &Path) -> (String, Value) {
    let robot = f.robot.to_str().unwrap();
    let stderr = ok(&shard(
        f,
        list,
        out,
        &[
            "--easyconfigs-repo",
            f.bare.to_str().unwrap(),
            "--materialize",
            "--build-robot-path",
            robot,
        ],
    ));
    (stderr, plan(out))
}

#[test]
fn materialize_writes_the_selected_commit_files_and_executable_lists() {
    let f = fixture();
    let plain = f.root.join("plain");
    let out = f.root.join("out");
    ok(&shard(
        &f,
        &root_line(&f),
        &plain,
        &["--easyconfigs-repo", f.bare.to_str().unwrap()],
    ));
    let (_, plan) = materialized(&f, &root_line(&f), &out);

    // The verbatim lists are the same files with and without --materialize.
    for name in listing(&plain) {
        if name != "plan.json" {
            assert_eq!(read(plain.join(&name)), read(out.join(&name)), "{name}");
        }
    }
    let dir = format!("sources/v1/{}", f.sha);
    let sources: Vec<String> = listing(&out.join(&dir));
    // The named root at the top, as EasyBuild copies it, and at its commit
    // path; the commit's New and the patches whose first match is in the
    // commit at their commit paths. The robot path's Dep and its patch,
    // Extra, and unused files are not there.
    assert_eq!(
        sources,
        [
            "Root-1.0-GCCcore-15.2.0.eb",
            "n/New/New-2.0-GCCcore-15.2.0.eb",
            "n/New/New-2.0_only.patch",
            "r/Root/Root-1.0-GCCcore-15.2.0.eb",
            "r/Root/Root-1.0_fix.patch",
        ]
    );
    assert_eq!(
        read(out.join(&dir).join("Root-1.0-GCCcore-15.2.0.eb")),
        read(f.tree.join("r/Root/Root-1.0-GCCcore-15.2.0.eb"))
    );

    let manifest: Value =
        serde_json::from_str(&read(out.join(format!("sources/v1/{}.json", f.sha)))).unwrap();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["provider"], "git");
    assert_eq!(manifest["resolved_commit"], f.sha.as_str());
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files.len(), 5);
    for file in files {
        use sha2::{Digest, Sha256};
        let bytes = std::fs::read(out.join(&dir).join(file["path"].as_str().unwrap())).unwrap();
        let sha: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(file["sha256"], sha.as_str());
        assert_eq!(file["bytes"], bytes.len());
        let from = file["from"].as_str().unwrap();
        assert_eq!(
            bytes,
            std::fs::read(
                f.tree
                    .join(from.trim_start_matches("easybuild/easyconfigs/"))
            )
            .unwrap()
        );
    }

    let abs = std::path::absolute(&out).unwrap().join(&dir);
    let shard_of = |file: &str| {
        plan["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["easyconfig"] == file)
            .unwrap()["shard"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let local = read(out.join(format!(
        "{}.local.list",
        shard_of("Root-1.0-GCCcore-15.2.0.eb")
    )));
    assert_eq!(
        local,
        format!(
            "{}/Root-1.0-GCCcore-15.2.0.eb --include-systems=snel-zen2 --robot-paths={}:{}\n",
            abs.display(),
            f.robot.display(),
            abs.display()
        )
    );
    assert!(!local.contains("--from-commit"));
    let plain_local = read(out.join(format!(
        "{}.local.list",
        shard_of("Plain-1.0-GCCcore-15.2.0.eb")
    )));
    assert_eq!(plain_local, "Plain-1.0-GCCcore-15.2.0.eb\n");

    let m = &plan["materialized"];
    assert_eq!(m["schema_version"], 1);
    assert_eq!(
        m["robot_paths"],
        serde_json::json!([f.robot.to_str().unwrap()])
    );
    assert_eq!(m["lines"][0]["root"], "commit");
    assert_eq!(m["sources"][&f.sha]["files"], 5);
    assert_eq!(m["sources"][&f.sha]["sha256"], manifest["sha256"]);
    assert_eq!(m["unchanged"], serde_json::json!([]));
}

#[test]
fn a_line_that_cannot_run_from_its_sources_is_copied_unchanged() {
    let f = fixture();
    let out = f.root.join("out");
    let list = format!(
        "Root-1.0-GCCcore-15.2.0.eb --from-commit={sha} --robot-paths=/elsewhere\nNew-2.0-GCCcore-15.2.0.eb --from-commit={sha}\n",
        sha = f.sha
    );
    let (_, plan) = materialized(&f, &list, &out);
    let unchanged = plan["materialized"]["unchanged"].as_array().unwrap();
    assert_eq!(unchanged.len(), 1, "{plan}");
    assert_eq!(unchanged[0]["easyconfig"], "Root-1.0-GCCcore-15.2.0.eb");
    assert!(unchanged[0]["reason"]
        .as_str()
        .unwrap()
        .contains("--robot-paths"));
    let all: String = listing(&out)
        .iter()
        .filter(|n| n.ends_with(".local.list"))
        .map(|n| read(out.join(n)))
        .collect();
    assert!(all.contains(&format!(
        "Root-1.0-GCCcore-15.2.0.eb --from-commit={} --robot-paths=/elsewhere\n",
        f.sha
    )));
    assert!(
        !all.contains("New-2.0-GCCcore-15.2.0.eb --from-commit"),
        "{all}"
    );
}

#[test]
fn a_patch_the_commit_and_robot_path_both_lack_keeps_the_line_unchanged() {
    let f = fixture();
    let out = f.root.join("out");
    // Rewrite the commit without New's patch.
    let work = f.root.join("work");
    std::fs::remove_file(work.join("easybuild/easyconfigs/n/New/New-2.0_only.patch")).unwrap();
    git(&work, &["commit", "-q", "-am", "drop patch"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    git(
        &f.bare,
        &[
            "fetch",
            "-q",
            work.to_str().unwrap(),
            "HEAD:refs/heads/drop",
        ],
    );
    let list = format!("New-2.0-GCCcore-15.2.0.eb --from-commit={sha}\n");
    let (_, plan) = materialized(&f, &list, &out);
    let unchanged = plan["materialized"]["unchanged"].as_array().unwrap();
    assert_eq!(unchanged.len(), 1, "{plan}");
    assert!(unchanged[0]["reason"]
        .as_str()
        .unwrap()
        .contains("New-2.0_only.patch"));
    assert!(!out.join(format!("sources/v1/{sha}")).exists());
}

#[test]
fn a_rerun_without_materialize_removes_the_written_sources_and_lists() {
    let f = fixture();
    let out = f.root.join("out");
    materialized(&f, &root_line(&f), &out);
    assert!(out.join("prelude.local.list").exists());
    ok(&shard(
        &f,
        &root_line(&f),
        &out,
        &["--easyconfigs-repo", f.bare.to_str().unwrap()],
    ));
    assert!(listing(&out)
        .iter()
        .all(|n| !n.contains(".local.") && !n.starts_with("sources")));
}

#[test]
fn materialize_options_are_checked_before_anything_is_written() {
    let f = fixture();
    for extra in [
        vec!["--materialize"],
        vec!["--materialize", "--build-robot-path", "relative/path"],
        vec!["--materialize", "--build-robot-path", "/a:/b"],
        vec!["--build-robot-path", "/a"],
    ] {
        let out = f.root.join("out");
        let run = shard(&f, &root_line(&f), &out, &extra);
        assert_eq!(
            run.status.code(),
            Some(2),
            "{extra:?}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(!out.exists(), "{extra:?}");
    }
}

#[test]
fn the_mcp_tool_takes_the_same_sources_options() {
    let f = fixture();
    let cli = f.root.join("cli");
    materialized(&f, &root_line(&f), &cli);
    let mcp = f.root.join("mcp");
    let installed = f.root.join("installed.txt");
    std::fs::write(&installed, "").unwrap();
    let modules = f.root.join("modules/all");
    std::fs::create_dir_all(&modules).unwrap();
    let response = eb_stack::mcp::handle_message(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_shard",
            "arguments": {
                "build_list": f.root.join("build.list").to_str().unwrap(),
                "easyconfigs": [f.robot.to_str().unwrap()],
                "easyconfigs_repo": f.bare.to_str().unwrap(),
                "materialize": true,
                "build_robot_path": [f.robot.to_str().unwrap()],
                "installed": [installed.to_str().unwrap()],
                "installed_modules_dir": [modules.to_str().unwrap()],
                "shards": 4,
                "generated_at": "2026-11-07T00:00:00Z",
                "out_dir": mcp.to_str().unwrap(),
            }
        }
    }))
    .expect("response");
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(listing(&cli), listing(&mcp));
    let cli_abs = std::path::absolute(&cli).unwrap().display().to_string();
    let mcp_abs = std::path::absolute(&mcp).unwrap().display().to_string();
    for name in listing(&cli) {
        assert_eq!(
            read(cli.join(&name)).replace(&cli_abs, "OUT"),
            read(mcp.join(&name)).replace(&mcp_abs, "OUT"),
            "{name}"
        );
    }
}

#[test]
fn a_prelude_module_only_the_commit_has_runs_from_its_sources() {
    let f = fixture();
    let work = f.root.join("work");
    ec(
        &work.join("easybuild/easyconfigs"),
        "Other",
        "1.0",
        &[("New", "2.0")],
        &[],
    );
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "other"]);
    let sha = git(&work, &["rev-parse", "HEAD"]);
    git(
        &f.bare,
        &[
            "fetch",
            "-q",
            work.to_str().unwrap(),
            "HEAD:refs/heads/other",
        ],
    );
    let list = format!(
        "Root-1.0-GCCcore-15.2.0.eb --from-commit={sha}\nOther-1.0-GCCcore-15.2.0.eb --from-commit={sha}\n"
    );
    let out = f.root.join("out");
    let (_, plan) = materialized(&f, &list, &out);
    assert_eq!(
        read(out.join("prelude.list")),
        format!("New-2.0-GCCcore-15.2.0.eb --from-commit={sha}\n")
    );
    let abs = std::path::absolute(&out)
        .unwrap()
        .join(format!("sources/v1/{sha}"));
    assert_eq!(
        read(out.join("prelude.local.list")),
        format!(
            "{}/New-2.0-GCCcore-15.2.0.eb --robot-paths={}:{}\n",
            abs.display(),
            f.robot.display(),
            abs.display()
        )
    );
    assert!(abs.join("New-2.0-GCCcore-15.2.0.eb").is_file());
    assert!(abs.join("n/New/New-2.0_only.patch").is_file());
    assert_eq!(
        plan["materialized"]["prelude"],
        serde_json::json!(["New/2.0-GCCcore-15.2.0"])
    );
}

#[test]
fn a_named_root_with_the_robot_paths_bytes_still_runs_from_the_commit_copy() {
    let f = fixture();
    let out = f.root.join("out");
    let list = format!("Plain-1.0-GCCcore-15.2.0.eb --from-commit={}\n", f.sha);
    assert_eq!(
        read(f.robot.join("p/Plain/Plain-1.0-GCCcore-15.2.0.eb")),
        read(f.tree.join("p/Plain/Plain-1.0-GCCcore-15.2.0.eb"))
    );
    let (_, plan) = materialized(&f, &list, &out);
    let abs = std::path::absolute(&out)
        .unwrap()
        .join(format!("sources/v1/{}", f.sha));
    assert_eq!(plan["materialized"]["lines"][0]["root"], "commit");
    assert_eq!(
        read(out.join("shard-01.local.list")),
        format!(
            "{}/Plain-1.0-GCCcore-15.2.0.eb --robot-paths={}:{}\n",
            abs.display(),
            f.robot.display(),
            abs.display()
        )
    );
    assert!(abs.join("Plain-1.0-GCCcore-15.2.0.eb").is_file());
}
