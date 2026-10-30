//! Regression checks for examples/easyconfigs-pr-overlap/fetch-prs.sh.
//!
//! The script should rewrite outputs to only the currently open pull requests
//! when rerun in the same output directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(program: &str, args: &[&str], cwd: &Path, envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().unwrap_or_else(|e| {
        panic!(
            "failed to run {} {} in {}: {e}",
            program,
            args.join(" "),
            cwd.display()
        )
    })
}

fn assert_ok(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{}\nstatus={:?}\nstdout:\n{}\nstderr:\n{}",
        context,
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = run("git", args, cwd, &[]);
    assert_ok(&output, &format!("git {}", args.join(" ")));
    String::from_utf8(output.stdout).expect("git stdout utf-8")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, text).expect("write file");
}

fn make_pr_head(seed: &Path, branch: &str, pr: u64, body: &str) -> String {
    git(seed, &["checkout", "-B", branch, "develop"]);
    write(&seed.join("easybuild/easyconfigs/l/Lib/Lib-1.0.eb"), body);
    git(seed, &["add", "."]);
    git(seed, &["commit", "-m", &format!("pr-{pr} head")]);
    let sha = git(seed, &["rev-parse", "HEAD"]).trim().to_string();
    git(
        seed,
        &["push", "origin", &format!("HEAD:refs/pull/{pr}/head")],
    );
    sha
}

fn json_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .map(|path| {
            path.file_name()
                .expect("filename")
                .to_string_lossy()
                .to_string()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn rerun_rewrites_outputs_to_current_open_pull_requests_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");

    assert_ok(
        &run(
            "git",
            &["init", "--bare", origin.to_str().expect("origin utf-8")],
            root,
            &[],
        ),
        "init bare origin",
    );

    fs::create_dir_all(&seed).expect("seed dir");
    assert_ok(&run("git", &["init"], &seed, &[]), "init seed");
    git(&seed, &["config", "user.name", "Fixture"]);
    git(&seed, &["config", "user.email", "fixture@example.invalid"]);
    write(
        &seed.join("easybuild/easyconfigs/l/Lib/Lib-1.0.eb"),
        "name = 'Lib'\nversion = '1.0'\n",
    );
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed develop"]);
    git(&seed, &["branch", "-M", "develop"]);
    git(
        &seed,
        &[
            "remote",
            "add",
            "origin",
            origin.to_str().expect("origin utf-8"),
        ],
    );
    git(&seed, &["push", "-u", "origin", "develop"]);

    let sha1 = make_pr_head(&seed, "pr1", 1, "name = 'Lib'\nversion = '1.0'\n# pr1\n");
    let sha2 = make_pr_head(&seed, "pr2", 2, "name = 'Lib'\nversion = '1.0'\n# pr2\n");
    git(&seed, &["checkout", "develop"]);

    assert_ok(
        &run(
            "git",
            &[
                "clone",
                origin.to_str().expect("origin utf-8"),
                clone.to_str().expect("clone utf-8"),
            ],
            root,
            &[],
        ),
        "clone fixture",
    );
    git(&clone, &["checkout", "develop"]);

    let open_first = root.join("open-first.jsonl");
    let open_second = root.join("open-second.jsonl");
    write(
        &open_first,
        &format!(
            "{{\"number\":1,\"headRefOid\":\"{sha1}\",\"title\":\"PR 1\"}}\n{{\"number\":2,\"headRefOid\":\"{sha2}\",\"title\":\"PR 2\"}}\n"
        ),
    );
    write(
        &open_second,
        &format!("{{\"number\":1,\"headRefOid\":\"{sha1}\",\"title\":\"PR 1\"}}\n"),
    );

    let mock_gh = mock_bin.join("gh");
    write(
        &mock_gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
url=""
for arg in "$@"; do
  case "$arg" in
    repos/*) url="$arg" ;;
  esac
done
if [[ "$url" == *"/pulls?state=open&per_page=100"* ]]; then
  cat "${MOCK_OPEN_JSONL:?}"
  exit 0
fi
if [[ "$url" == *"/pulls/1/files"* ]]; then
  printf '%s\n' "easybuild/easyconfigs/l/Lib/Lib-1.0.eb"
  exit 0
fi
if [[ "$url" == *"/pulls/2/files"* ]]; then
  printf '%s\n' "easybuild/easyconfigs/l/Lib/Lib-1.0.eb"
  exit 0
fi
echo "unexpected gh api request: $url" >&2
exit 1
"#,
    );
    assert_ok(
        &run(
            "chmod",
            &["+x", mock_gh.to_str().expect("mock gh utf-8")],
            root,
            &[],
        ),
        "chmod mock gh",
    );

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/easyconfigs-pr-overlap/fetch-prs.sh");
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let out_dir = out.to_str().expect("out utf-8");
    let script_path = script.to_str().expect("script utf-8");

    let first = run(
        "bash",
        &[script_path, "acme/repo", out_dir],
        &clone,
        &[
            ("PATH", &path),
            (
                "MOCK_OPEN_JSONL",
                open_first.to_str().expect("open_first utf-8"),
            ),
        ],
    );
    assert_ok(&first, "first script run");
    assert_eq!(json_files(&out.join("prs")), ["1.json", "2.json"]);

    let second = run(
        "bash",
        &[script_path, "acme/repo", out_dir],
        &clone,
        &[
            ("PATH", &path),
            (
                "MOCK_OPEN_JSONL",
                open_second.to_str().expect("open_second utf-8"),
            ),
        ],
    );
    assert_ok(&second, "second script run");
    assert_eq!(json_files(&out.join("prs")), ["1.json"]);
}
