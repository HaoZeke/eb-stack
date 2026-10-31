//! Regression checks for the fetch-prs.sh scripts under examples/.
//!
//! The easyconfigs script should rewrite outputs to only the currently open
//! pull requests when rerun in the same output directory. The EESSI script
//! should read every page of open pull requests, not stop at a fixed limit.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

include!("support/write_executable.rs");

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
    write_executable(
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
    )
    .expect("write mock gh");

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

fn pulls_page(numbers: impl Iterator<Item = u64>, sha_of: impl Fn(u64) -> String) -> String {
    let rows: Vec<serde_json::Value> = numbers
        .map(|n| {
            serde_json::json!({
                "number": n,
                "title": format!("PR {n}"),
                "head": {"sha": sha_of(n)},
            })
        })
        .collect();
    serde_json::to_string(&rows).expect("page json")
}

#[test]
fn eessi_fetch_reads_every_page_of_open_pull_requests() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");
    let stack = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.1-2026.1.yml";
    const EC_PIN: &str = "1111111111111111111111111111111111111111";
    const BLOCKS_PIN: &str = "2222222222222222222222222222222222222222";

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
        &seed.join(stack),
        "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n",
    );
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed main"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &[
            "remote",
            "add",
            "origin",
            origin.to_str().expect("origin utf-8"),
        ],
    );
    git(&seed, &["push", "-u", "origin", "main"]);

    let mut heads = std::collections::BTreeMap::new();
    for pr in [1_u64, 102] {
        git(&seed, &["checkout", "-B", &format!("pr{pr}"), "main"]);
        write(
            &seed.join(stack),
            &format!(
                "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg{pr}-1.0-GCCcore-15.2.0.eb:\n      options:\n        from-commit: {EC_PIN}\n        include-easyblocks-from-commit: {BLOCKS_PIN}\n"
            ),
        );
        git(&seed, &["commit", "-am", &format!("pr-{pr} head")]);
        let sha = git(&seed, &["rev-parse", "HEAD"]).trim().to_string();
        git(
            &seed,
            &["push", "origin", &format!("HEAD:refs/pull/{pr}/head")],
        );
        heads.insert(pr, sha);
    }
    git(&seed, &["checkout", "main"]);
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

    // 102 open pull requests over two pages of 100. Only 1 and 102 change
    // an easystack, and 102 is on the second page.
    let sha_of = |n: u64| {
        heads
            .get(&n)
            .cloned()
            .unwrap_or_else(|| format!("{n:040x}"))
    };
    let page1 = root.join("page1.json");
    let page2 = root.join("page2.json");
    write(&page1, &pulls_page(1..=100, sha_of));
    write(&page2, &pulls_page(101..=102, sha_of));
    let errors = root.join("gh-errors.log");

    // A stand-in for gh: `api` only, one open-pulls URL, the files of pull
    // requests 1 to 102, and the easyblocks commit PR 102 pins. The pages are GitHub's raw shape and --jq is
    // applied per page, as gh does. Without --paginate it serves page 1
    // only. Every refusal is also logged, because a gh call inside a
    // process substitution cannot fail the script.
    let mock_gh = mock_bin.join("gh");
    write_executable(
        &mock_gh,
        r#"#!/usr/bin/env bash
set -euo pipefail
refuse() {
  echo "mock gh: $*" | tee -a "${MOCK_ERRORS:?}" >&2
  exit 1
}
[[ "${1:-}" == api ]] || refuse "unexpected command: gh $*"
shift
url="" filter="" paginate=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method) shift ;;
    --paginate) paginate=1 ;;
    --jq) shift; filter=$1 ;;
    repos/*) url=$1 ;;
    *) refuse "unexpected argument: $1" ;;
  esac
  shift
done
pages=()
case "$url" in
  "repos/EESSI/software-layer/pulls?state=open&per_page=100")
    pages=("${MOCK_PAGE1:?}")
    [[ $paginate -eq 1 ]] && pages+=("${MOCK_PAGE2:?}")
    for page in "${pages[@]}"; do
      if [[ -n "$filter" ]]; then jq -c "$filter" "$page"; else cat "$page"; fi
    done
    ;;
  repos/EESSI/software-layer/pulls/1/files|repos/EESSI/software-layer/pulls/102/files)
    printf '%s\n' "${MOCK_STACK:?}"
    ;;
  repos/easybuilders/easybuild-easyblocks/commits/2222222222222222222222222222222222222222)
    printf '%s\n' easybuild/easyblocks/p/pkg.py
    ;;
  repos/EESSI/software-layer/pulls/*/files)
    number=${url#repos/EESSI/software-layer/pulls/}
    number=${number%/files}
    [[ "$number" =~ ^[0-9]+$ && $number -ge 2 && $number -le 101 ]] \
      || refuse "unexpected pull request: $url"
    ;;
  *) refuse "unexpected URL: $url" ;;
esac
"#,
    )
    .expect("write mock gh");
    // The pins are not fetched: every curl fails, and the script records
    // the pin as unresolved and goes on.
    write_executable(
        &mock_bin.join("curl"),
        "#!/usr/bin/env bash\necho \"mock curl: no network: $*\" >&2\nexit 22\n",
    )
    .expect("write mock curl");

    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/eessi-pr-overlap/fetch-prs.sh");
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let output = run(
        "bash",
        &[
            script.to_str().expect("script utf-8"),
            "EESSI/software-layer",
            "2026.06",
            out.to_str().expect("out utf-8"),
        ],
        &clone,
        &[
            ("PATH", &path),
            ("MOCK_PAGE1", page1.to_str().expect("page1 utf-8")),
            ("MOCK_PAGE2", page2.to_str().expect("page2 utf-8")),
            ("MOCK_STACK", stack),
            ("MOCK_ERRORS", errors.to_str().expect("errors utf-8")),
        ],
    );
    assert_ok(&output, "eessi fetch-prs.sh");
    let refused = fs::read_to_string(&errors).unwrap_or_default();
    assert!(refused.is_empty(), "mock gh refused calls:\n{refused}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "open pull requests: 102"),
        "stdout:\n{stdout}"
    );
    assert_eq!(json_files(&out.join("prs")), ["1.json", "102.json"]);
    let doc: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(out.join("prs/102.json")).expect("read prs/102.json"),
    )
    .expect("prs/102.json is JSON");
    assert_eq!(doc["number"], 102);
    assert_eq!(doc["title"], "PR 102");
    assert_eq!(doc["head_sha"], heads[&102].as_str());
    assert_eq!(doc["easystacks"][0]["path"], stack);
    assert!(doc["easystacks"][0]["head"]
        .as_str()
        .expect("head text")
        .contains("Pkg102-1.0-GCCcore-15.2.0.eb"));
}

const EESSI_STACK: &str = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.1-2026.1.yml";

/// Seed a bare origin with `main` carrying `EESSI_STACK`, push one head per
/// `(number, body)` to `refs/pull/<N>/head`, and clone it into `root/clone`.
/// Returns each pull request's head sha. Mirrors the setup the existing EESSI
/// test does inline.
fn eessi_seed_and_clone(
    root: &Path,
    prs: &[(u64, String)],
) -> std::collections::BTreeMap<u64, String> {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");

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
        &seed.join(EESSI_STACK),
        "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n",
    );
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "seed main"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &[
            "remote",
            "add",
            "origin",
            origin.to_str().expect("origin utf-8"),
        ],
    );
    git(&seed, &["push", "-u", "origin", "main"]);

    let mut heads = std::collections::BTreeMap::new();
    for (pr, body) in prs {
        git(&seed, &["checkout", "-B", &format!("pr{pr}"), "main"]);
        write(&seed.join(EESSI_STACK), body);
        git(&seed, &["commit", "-am", &format!("pr-{pr} head")]);
        let sha = git(&seed, &["rev-parse", "HEAD"]).trim().to_string();
        git(
            &seed,
            &["push", "origin", &format!("HEAD:refs/pull/{pr}/head")],
        );
        heads.insert(*pr, sha);
    }
    git(&seed, &["checkout", "main"]);

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
    heads
}

/// A stand-in for `gh api`: one open-pulls URL served from `MOCK_PAGE` (raw
/// GitHub shape, with `--jq` applied as gh does), the changed files of any
/// pull request from `MOCK_STACK`, and any easyblocks commit's one touched
/// file. Also a `curl` that always fails, so a named pin stays unresolved
/// without any network, as it does in the field.
fn write_eessi_mocks(mock_bin: &Path) {
    write_executable(
        &mock_bin.join("gh"),
        r#"#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == api ]] || { echo "unexpected: gh $*" >&2; exit 1; }
shift
url="" filter=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --method) shift ;;
    --paginate) ;;
    --jq) shift; filter=$1 ;;
    repos/*) url=$1 ;;
    *) echo "unexpected arg: $1" >&2; exit 1 ;;
  esac
  shift
done
case "$url" in
  *"/pulls?state=open&per_page=100")
    if [[ -n "$filter" ]]; then jq -c "$filter" "${MOCK_PAGE:?}"; else cat "${MOCK_PAGE:?}"; fi ;;
  repos/easybuilders/easybuild-easyblocks/commits/*)
    printf '%s\n' easybuild/easyblocks/p/pkg.py ;;
  */pulls/*/files)
    printf '%s\n' "${MOCK_STACK:?}" ;;
  *) echo "unexpected url: $url" >&2; exit 1 ;;
esac
"#,
    )
    .expect("write mock gh");
    write_executable(
        &mock_bin.join("curl"),
        "#!/usr/bin/env bash\necho \"mock curl: no network: $*\" >&2\nexit 22\n",
    )
    .expect("write mock curl");
}

fn eessi_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/eessi-pr-overlap/fetch-prs.sh")
}

fn run_eessi(
    clone: &Path,
    out: &Path,
    mock_bin: &Path,
    page: &Path,
    extra: &[(&str, &str)],
) -> Output {
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let mut envs: Vec<(&str, &str)> = vec![
        ("PATH", path.as_str()),
        ("MOCK_PAGE", page.to_str().expect("page utf-8")),
        ("MOCK_STACK", EESSI_STACK),
    ];
    envs.extend_from_slice(extra);
    // `path` must outlive the borrow; bind the run before it drops.
    run(
        "bash",
        &[
            eessi_script().to_str().expect("script utf-8"),
            "EESSI/software-layer",
            "2026.06",
            out.to_str().expect("out utf-8"),
        ],
        clone,
        &envs,
    )
}

/// A rerun into the same out-dir must drop a pull request that is no longer
/// open: the first run sees {1, 2}, the second only {2}, so `prs/` holds just
/// `2.json` and the count line says 1. Before the fix `raw/` and `prs/` were
/// never cleared, so `1.json` survived and the count stayed 2.
#[test]
fn eessi_rerun_rewrites_outputs_to_current_open_pull_requests_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");

    // Both heads name a from-commit and an include-easyblocks-from-commit pin,
    // so the b7c933b script gets past bug (c) and this test isolates the
    // stale-output bug. The pins never resolve (curl fails), as in the field.
    let body = |pr: u64| {
        format!(
            "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg{pr}-1.0-GCCcore-15.2.0.eb:\n      options:\n        from-commit: 1111111111111111111111111111111111111111\n        include-easyblocks-from-commit: 2222222222222222222222222222222222222222\n"
        )
    };
    let heads = eessi_seed_and_clone(root, &[(1, body(1)), (2, body(2))]);
    write_eessi_mocks(&mock_bin);

    let sha_of = |n: u64| {
        heads
            .get(&n)
            .cloned()
            .unwrap_or_else(|| format!("{n:040x}"))
    };
    let page_first = root.join("page-first.json");
    let page_second = root.join("page-second.json");
    write(&page_first, &pulls_page([1, 2].into_iter(), sha_of));
    write(&page_second, &pulls_page([2].into_iter(), sha_of));

    let first = run_eessi(&clone, &out, &mock_bin, &page_first, &[]);
    assert_ok(&first, "first eessi run");
    assert_eq!(json_files(&out.join("prs")), ["1.json", "2.json"]);

    let second = run_eessi(&clone, &out, &mock_bin, &page_second, &[]);
    assert_ok(&second, "second eessi run");
    assert_eq!(json_files(&out.join("prs")), ["2.json"]);
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout
            .lines()
            .any(|line| line == "pull requests with 2026.06 easystack changes: 1"),
        "stdout:\n{stdout}"
    );
}

/// A run where no head easystack names a pin is a normal, successful run. The
/// script must reach the count line and exit 0. Before the fix the empty
/// `grep` in the from-commit pipeline exited 1 under `pipefail` and aborted
/// the script after `prs/` was written but before the count line.
#[test]
fn eessi_no_pin_run_reaches_count_line_and_exits_zero() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");

    let body = |pr: u64| {
        format!(
            "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg{pr}-1.0-GCCcore-15.2.0.eb\n"
        )
    };
    let heads = eessi_seed_and_clone(root, &[(1, body(1)), (2, body(2))]);
    write_eessi_mocks(&mock_bin);

    let sha_of = |n: u64| {
        heads
            .get(&n)
            .cloned()
            .unwrap_or_else(|| format!("{n:040x}"))
    };
    let page = root.join("page.json");
    write(&page, &pulls_page([1, 2].into_iter(), sha_of));

    let output = run_eessi(&clone, &out, &mock_bin, &page, &[]);
    assert_ok(&output, "no-pin eessi run");
    assert_eq!(json_files(&out.join("prs")), ["1.json", "2.json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .lines()
            .any(|line| line == "pull requests with 2026.06 easystack changes: 2"),
        "stdout:\n{stdout}"
    );
    // No pin was named, so nothing was fetched into these caches.
    assert!(!out
        .join("commit-trees")
        .join("1111111111111111111111111111111111111111")
        .exists());
    assert!(!out
        .join("easyblocks")
        .join("2222222222222222222222222222222222222222")
        .exists());
}

/// Every pull request head is fetched in one `git fetch --stdin`, not one
/// shallow fetch per pull request. A `git` wrapper on PATH logs each call and
/// delegates to the real git; exactly one logged call carries `--stdin`, and
/// no per-PR `refs/pull/<N>/head` fetch is logged. Before the fix the loop ran
/// a shallow fetch per pull request and never used `--stdin`.
#[test]
fn eessi_fetches_all_pull_request_heads_in_one_stdin_fetch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");

    let body = |pr: u64| {
        format!(
            "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg{pr}-1.0-GCCcore-15.2.0.eb\n"
        )
    };
    let heads = eessi_seed_and_clone(root, &[(1, body(1)), (2, body(2))]);
    write_eessi_mocks(&mock_bin);

    let real_git = String::from_utf8(run("bash", &["-c", "command -v git"], root, &[]).stdout)
        .expect("git path utf-8")
        .trim()
        .to_string();
    let git_log = root.join("git-calls.log");
    write_executable(
        &mock_bin.join("git"),
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >>\"${GIT_LOG:?}\"\nexec \"${REAL_GIT:?}\" \"$@\"\n",
    )
    .expect("write mock git");

    let sha_of = |n: u64| {
        heads
            .get(&n)
            .cloned()
            .unwrap_or_else(|| format!("{n:040x}"))
    };
    let page = root.join("page.json");
    write(&page, &pulls_page([1, 2].into_iter(), sha_of));

    let output = run_eessi(
        &clone,
        &out,
        &mock_bin,
        &page,
        &[
            ("GIT_LOG", git_log.to_str().expect("log utf-8")),
            ("REAL_GIT", real_git.as_str()),
        ],
    );
    assert_ok(&output, "single-fetch eessi run");
    assert_eq!(json_files(&out.join("prs")), ["1.json", "2.json"]);

    let log = fs::read_to_string(&git_log).expect("read git log");
    let stdin_fetches: Vec<&str> = log
        .lines()
        .filter(|line| line.starts_with("fetch ") || line.contains(" fetch "))
        .filter(|line| line.contains("--stdin"))
        .collect();
    assert_eq!(
        stdin_fetches.len(),
        1,
        "expected one `fetch --stdin`, saw {stdin_fetches:?} in:\n{log}"
    );
    assert!(
        !log.lines().any(|line| line.contains("refs/pull/")),
        "no per-PR refs/pull fetch should be logged:\n{log}"
    );
}

#[test]
fn easyconfigs_fetch_refuses_an_empty_out_dir_before_deleting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");

    // Every command the script could run to touch the filesystem or the
    // network is stubbed out: each stand-in logs its name and arguments to
    // the shared log and exits 1. If the empty-argument guard were missing,
    // the script would reach `rm -rf "$out/raw" "$out/prs"` on `/raw` and
    // `/prs`; the stand-in records that call instead of deleting anything,
    // so this test is safe even when CI runs as root.
    let log = root.join("commands.log");
    for name in ["rm", "mkdir", "git", "gh", "python3"] {
        write_executable(
            &mock_bin.join(name),
            &format!(
                "#!/usr/bin/env bash\nprintf '{name} %s\\n' \"$*\" >> \"${{MOCK_LOG:?}}\"\nexit 1\n"
            ),
        )
        .expect("write mock command");
    }

    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/easyconfigs-pr-overlap/fetch-prs.sh");
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );

    let output = run(
        "bash",
        &[script.to_str().expect("script utf-8"), "acme/repo", ""],
        root,
        &[
            ("PATH", &path),
            ("MOCK_LOG", log.to_str().expect("log utf-8")),
        ],
    );

    assert!(
        !output.status.success(),
        "script exited successfully on an empty out-dir\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let logged = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        logged.is_empty(),
        "script ran a mocked command before the guard:\n{logged}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("usage: fetch-prs.sh <owner/repo> <out-dir>"),
        "stderr missing usage guard:\n{stderr}"
    );
}
