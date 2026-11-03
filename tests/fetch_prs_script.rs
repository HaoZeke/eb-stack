//! Regression checks for the fetch-prs.sh scripts under examples/.
//!
//! The easyconfigs script should rewrite outputs to only the currently open
//! pull requests when rerun in the same output directory. Both scripts should
//! read every page of open pull requests, including a repository with more
//! than 500, and page a changed-file list through GitHub's 3000-file cap.

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
page=$(sed -n 's/.*[?&]page=\([0-9][0-9]*\).*/\1/p' <<<"$url")
[[ -n "$page" ]] || page=1
if [[ "$url" == *"/pulls?state=open&per_page=100"* ]]; then
  if [[ "$page" == 1 ]]; then cat "${MOCK_OPEN_JSONL:?}"; fi
  exit 0
fi
if [[ "$url" == *"/pulls/1/files"* || "$url" == *"/pulls/2/files"* ]]; then
  if [[ "$page" == 1 ]]; then
    printf '%s\n' "easybuild/easyconfigs/l/Lib/Lib-1.0.eb"
  fi
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

    // A stand-in for gh: `api` only, one open-pulls page per request, the
    // files of pull requests 1 and 102, and the easyblocks commit PR 102
    // pins. The pages are GitHub's raw shape and --jq is applied per page,
    // as gh does. A page the fixture does not have is an empty array.
    // Every refusal is also logged.
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
page_n=$(sed -n 's/.*[?&]page=\([0-9][0-9]*\).*/\1/p' <<<"$url")
[[ -n "$page_n" ]] || page_n=1
case "$url" in
  *"/pulls?state=open&per_page=100"*)
    file=""
    case "$page_n" in
      1) file="${MOCK_PAGE1:?}" ;;
      2) file="${MOCK_PAGE2:-}" ;;
    esac
    if [[ -n "$file" && -f "$file" ]]; then
      if [[ -n "$filter" ]]; then jq -c "$filter" "$file"; else cat "$file"; fi
    else
      if [[ -n "$filter" ]]; then echo '[]' | jq -c "$filter"; else echo '[]'; fi
    fi
    ;;
  repos/EESSI/software-layer/pulls/1/files*|repos/EESSI/software-layer/pulls/102/files*)
    if [[ "$page_n" == 1 ]]; then printf '%s\n' "${MOCK_STACK:?}"; fi
    ;;
  repos/easybuilders/easybuild-easyblocks/commits/2222222222222222222222222222222222222222)
    printf '%s\n' easybuild/easyblocks/p/pkg.py
    ;;
  repos/EESSI/software-layer/pulls/*/files*)
    number=${url#repos/EESSI/software-layer/pulls/}
    number=${number%%/*}
    number=${number%%\?*}
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
page_n=$(sed -n 's/.*[?&]page=\([0-9][0-9]*\).*/\1/p' <<<"$url")
[[ -n "$page_n" ]] || page_n=1
case "$url" in
  *"/pulls?state=open&per_page=100"*)
    if [[ "$page_n" == 1 ]]; then
      if [[ -n "$filter" ]]; then jq -c "$filter" "${MOCK_PAGE:?}"; else cat "${MOCK_PAGE:?}"; fi
    else
      if [[ -n "$filter" ]]; then echo '[]' | jq -c "$filter"; else echo '[]'; fi
    fi
    ;;
  repos/easybuilders/easybuild-easyblocks/commits/*)
    printf '%s\n' easybuild/easyblocks/p/pkg.py ;;
  */pulls/*/files*)
    if [[ "$page_n" == 1 ]]; then printf '%s\n' "${MOCK_STACK:?}"; fi ;;
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

fn write_open_pages(root: &Path, heads: &std::collections::BTreeMap<u64, String>, last: u64) {
    let sha_of = |n: u64| {
        heads
            .get(&n)
            .cloned()
            .unwrap_or_else(|| format!("{n:040x}"))
    };
    let pages = last.div_ceil(100);
    for page in 1..=pages {
        let start = (page - 1) * 100 + 1;
        let end = std::cmp::min(page * 100, last);
        write(
            &root.join(format!("page{page}.json")),
            &pulls_page(start..=end, sha_of),
        );
    }
}

/// Open pull requests are served one page at a time from `$MOCK_ROOT/pageN.json`.
/// Changed-file pages for `$MOCK_KEEP` print `$MOCK_PATH` on page 1 only.
/// Every other pull request has an empty file list. A missing page is `[]`.
fn write_paged_open_mock(mock_bin: &Path) {
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
page_n=$(sed -n 's/.*[?&]page=\([0-9][0-9]*\).*/\1/p' <<<"$url")
[[ -n "$page_n" ]] || page_n=1
case "$url" in
  *"/pulls?state=open&per_page=100"*)
    file="${MOCK_ROOT:?}/page${page_n}.json"
    if [[ -f "$file" ]]; then
      if [[ -n "$filter" ]]; then jq -c "$filter" "$file"; else cat "$file"; fi
    else
      if [[ -n "$filter" ]]; then echo '[]' | jq -c "$filter"; else echo '[]'; fi
    fi
    ;;
  */pulls/"${MOCK_KEEP:?}"/files*)
    if [[ "$page_n" == 1 ]]; then printf '%s\n' "${MOCK_PATH:?}"; fi
    ;;
  */pulls/*/files*) ;;
  *) echo "unexpected url: $url" >&2; exit 1 ;;
esac
"#,
    )
    .expect("write paged open mock");
}

/// 501 open pull requests, one per number. Only 501, on page 6, changes an
/// easyconfig. A walk that stopped at 500 would leave `prs/` empty.
#[test]
fn easyconfigs_fetch_reads_open_pull_requests_past_five_hundred() {
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
    let sha = make_pr_head(
        &seed,
        "pr501",
        501,
        "name = 'Lib'\nversion = '1.0'\n# pr501\n",
    );
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

    let mut heads = std::collections::BTreeMap::new();
    heads.insert(501, sha.clone());
    write_open_pages(root, &heads, 501);
    write_paged_open_mock(&mock_bin);
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/easyconfigs-pr-overlap/fetch-prs.sh");
    let ec = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let output = run(
        "bash",
        &[
            script.to_str().expect("script utf-8"),
            "acme/repo",
            out.to_str().expect("out utf-8"),
        ],
        &clone,
        &[
            ("PATH", path.as_str()),
            ("MOCK_ROOT", root.to_str().expect("root utf-8")),
            ("MOCK_KEEP", "501"),
            ("MOCK_PATH", ec),
        ],
    );
    assert_ok(&output, "easyconfigs fetch past 500");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "open pull requests: 501"),
        "stdout:\n{stdout}"
    );
    assert_eq!(json_files(&out.join("prs")), ["501.json"]);
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("prs/501.json")).expect("read 501"))
            .expect("501 json");
    assert_eq!(doc["number"], 501);
    assert_eq!(doc["head_sha"], sha);
    assert_eq!(doc["easyconfigs"][0]["path"], ec);
    assert!(doc["easyconfigs"][0]["head"]
        .as_str()
        .expect("head")
        .contains("# pr501"));
    assert!(doc.get("file_list_truncated").is_none());
}

/// 501 open pull requests. Only 501, on page 6, changes an easystack.
#[test]
fn eessi_fetch_reads_open_pull_requests_past_five_hundred() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");
    let body = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg501-1.0-GCCcore-15.2.0.eb\n";
    let heads = eessi_seed_and_clone(root, &[(501, body.to_string())]);
    write_open_pages(root, &heads, 501);
    write_paged_open_mock(&mock_bin);
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let output = run(
        "bash",
        &[
            eessi_script().to_str().expect("script utf-8"),
            "EESSI/software-layer",
            "2026.06",
            out.to_str().expect("out utf-8"),
        ],
        &clone,
        &[
            ("PATH", path.as_str()),
            ("MOCK_ROOT", root.to_str().expect("root utf-8")),
            ("MOCK_KEEP", "501"),
            ("MOCK_PATH", EESSI_STACK),
        ],
    );
    assert_ok(&output, "eessi fetch past 500");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "open pull requests: 501"),
        "stdout:\n{stdout}"
    );
    assert_eq!(json_files(&out.join("prs")), ["501.json"]);
    let doc: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("prs/501.json")).expect("read 501"))
            .expect("501 json");
    assert_eq!(doc["number"], 501);
    assert_eq!(doc["head_sha"], heads[&501]);
    assert_eq!(doc["easystacks"][0]["path"], EESSI_STACK);
    assert!(doc["easystacks"][0]["head"]
        .as_str()
        .expect("head")
        .contains("Pkg501-1.0-GCCcore-15.2.0.eb"));
    assert!(doc.get("file_list_truncated").is_none());
}

/// Changed files are one page of 100 at a time.
///
/// Pull request 8's easyconfig is the 501st file (page 6). Pull request 9's
/// list answers 422 on the page after 3000 files; the pull request is kept
/// and `file_list_truncated` is set.
fn write_file_page_mock(mock_bin: &Path, interesting: &str) {
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
[[ "${{1:-}}" == api ]] || {{ echo "unexpected: gh $*" >&2; exit 1; }}
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
page_n=$(sed -n 's/.*[?&]page=\([0-9][0-9]*\).*/\1/p' <<<"$url")
[[ -n "$page_n" ]] || page_n=1
interesting={interesting:?}
emit_fill() {{
  local n=$1
  local i
  for ((i=0; i<n; i++)); do
    printf '%s\n' "notes/p${{page_n}}-${{i}}.txt"
  done
}}
case "$url" in
  *"/pulls?state=open&per_page=100"*)
    file="${{MOCK_ROOT:?}}/page${{page_n}}.json"
    if [[ -f "$file" ]]; then
      if [[ -n "$filter" ]]; then jq -c "$filter" "$file"; else cat "$file"; fi
    else
      if [[ -n "$filter" ]]; then echo '[]' | jq -c "$filter"; else echo '[]'; fi
    fi
    ;;
  */pulls/8/files*)
    if (( page_n >= 1 && page_n <= 5 )); then emit_fill 100
    elif (( page_n == 6 )); then emit_fill 99; printf '%s\n' "$interesting"
    fi
    ;;
  */pulls/9/files*)
    if (( page_n == 1 )); then
      printf '%s\n' "$interesting"
      emit_fill 99
    elif (( page_n >= 2 && page_n <= 30 )); then emit_fill 100
    elif (( page_n == 31 )); then
      echo "gh: pagination is limited for this resource. Responses include a maximum of 3000 files. (HTTP 422)" >&2
      exit 1
    fi
    ;;
  */pulls/*/files*) ;;
  *) echo "unexpected url: $url" >&2; exit 1 ;;
esac
"#,
        interesting = interesting,
    );
    write_executable(&mock_bin.join("gh"), &script).expect("write file page mock");
}

#[test]
fn easyconfigs_fetch_pages_files_past_five_hundred_and_keeps_the_cap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");
    let ec = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";

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
    write(&seed.join(ec), "name = 'Lib'\nversion = '1.0'\n");
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
    let sha8 = make_pr_head(&seed, "pr8", 8, "name = 'Lib'\nversion = '1.0'\n# page6\n");
    let sha9 = make_pr_head(&seed, "pr9", 9, "name = 'Lib'\nversion = '1.0'\n# capped\n");
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

    let mut heads = std::collections::BTreeMap::new();
    heads.insert(8, sha8.clone());
    heads.insert(9, sha9.clone());
    write(
        &root.join("page1.json"),
        &pulls_page([8_u64, 9].into_iter(), |n| heads[&n].clone()),
    );
    write_file_page_mock(&mock_bin, ec);
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/easyconfigs-pr-overlap/fetch-prs.sh");
    let output = run(
        "bash",
        &[
            script.to_str().expect("script utf-8"),
            "acme/repo",
            out.to_str().expect("out utf-8"),
        ],
        &clone,
        &[
            ("PATH", path.as_str()),
            ("MOCK_ROOT", root.to_str().expect("root utf-8")),
        ],
    );
    assert_ok(&output, "easyconfigs file pages");
    assert_eq!(json_files(&out.join("prs")), ["8.json", "9.json"]);
    let past: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("prs/8.json")).expect("read 8"))
            .expect("8 json");
    assert_eq!(past["easyconfigs"][0]["path"], ec);
    assert!(past["easyconfigs"][0]["head"]
        .as_str()
        .expect("head")
        .contains("# page6"));
    assert!(past.get("file_list_truncated").is_none());
    let capped: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("prs/9.json")).expect("read 9"))
            .expect("9 json");
    assert_eq!(capped["file_list_truncated"], true);
    assert_eq!(capped["easyconfigs"][0]["path"], ec);
    assert!(capped["easyconfigs"][0]["head"]
        .as_str()
        .expect("head")
        .contains("# capped"));
}

#[test]
fn eessi_fetch_pages_files_past_five_hundred_and_keeps_the_cap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path();
    let clone = root.join("clone");
    let out = root.join("out");
    let mock_bin = root.join("mock-bin");
    fs::create_dir_all(&mock_bin).expect("mock bin");
    let body8 = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg8-1.0-GCCcore-15.2.0.eb\n";
    let body9 = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n  - Pkg9-1.0-GCCcore-15.2.0.eb\n";
    let heads = eessi_seed_and_clone(root, &[(8, body8.to_string()), (9, body9.to_string())]);
    write(
        &root.join("page1.json"),
        &pulls_page([8_u64, 9].into_iter(), |n| heads[&n].clone()),
    );
    write_file_page_mock(&mock_bin, EESSI_STACK);
    let path = format!(
        "{}:{}",
        mock_bin.display(),
        std::env::var("PATH").expect("PATH")
    );
    let output = run(
        "bash",
        &[
            eessi_script().to_str().expect("script utf-8"),
            "EESSI/software-layer",
            "2026.06",
            out.to_str().expect("out utf-8"),
        ],
        &clone,
        &[
            ("PATH", path.as_str()),
            ("MOCK_ROOT", root.to_str().expect("root utf-8")),
        ],
    );
    assert_ok(&output, "eessi file pages");
    assert_eq!(json_files(&out.join("prs")), ["8.json", "9.json"]);
    let past: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("prs/8.json")).expect("read 8"))
            .expect("8 json");
    assert_eq!(past["easystacks"][0]["path"], EESSI_STACK);
    assert!(past["easystacks"][0]["head"]
        .as_str()
        .expect("head")
        .contains("Pkg8-1.0-GCCcore-15.2.0.eb"));
    assert!(past.get("file_list_truncated").is_none());
    let capped: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("prs/9.json")).expect("read 9"))
            .expect("9 json");
    assert_eq!(capped["file_list_truncated"], true);
    assert_eq!(capped["easystacks"][0]["path"], EESSI_STACK);
    assert!(capped["easystacks"][0]["head"]
        .as_str()
        .expect("head")
        .contains("Pkg9-1.0-GCCcore-15.2.0.eb"));
}
