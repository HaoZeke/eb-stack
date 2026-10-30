//! Four open easybuild-easyconfigs pull requests, recorded 2026-10-24.
//!
//! develop `981d2123dddccca9ce0b15f9eb2643e872332f5e`. The robot tree is the
//! modules those closures reach, plus `gfbf-2025a.eb`: wxPython is built with
//! gfbf, and that definition pins GCC 14.2.0, which is how LibSoup at
//! GCC 14.2.0 stays on the reverse-dependency walk. The `.eb` files are a
//! subset of easybuild-easyconfigs and stay under `EASYCONFIGS-GPLv2.txt`.
//!
//! `live_github_open_easyconfigs_pull_request` is ignored. It reads the
//! public GitHub API over HTTPS, with no token.

use eb_stack::pr_overlap::{
    easyconfig_record, fetch_open_easyconfig_prs, fetch_open_software_layer_prs, HttpGet,
    HttpResponse, OpenEasyconfigQuery, OverlapConfig, UreqGet, GITHUB_PR_FILES_TRUNCATED,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_easyconfigs_open")
}

fn relative_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path.strip_prefix(dir).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn recorded_open_pull_requests_match_the_full_develop_run() {
    let dir = fixtures();
    let sha = std::fs::read_to_string(dir.join("develop-sha")).unwrap();
    assert_eq!(sha, "981d2123dddccca9ce0b15f9eb2643e872332f5e\n");
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--kind", "easyconfigs", "--pr"])
        .arg(dir.join("prs.json"))
        .arg("--easyconfigs")
        .arg(dir.join("tree"))
        .args(["--generated-at", "2026-10-24T00:00:00Z"])
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
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        stdout.contains("prs=4") && stdout.contains("shared_modules=1"),
        "{stdout}"
    );
    let expected = dir.join("expected");
    let names = relative_files(&expected);
    assert!(!names.is_empty());
    for name in names {
        let got = std::fs::read_to_string(out.join(&name)).unwrap();
        let want = std::fs::read_to_string(expected.join(&name)).unwrap();
        assert_eq!(got, want, "{}", name.display());
    }
}

struct MapGet {
    pages: BTreeMap<String, (u16, String)>,
    links: BTreeMap<String, String>,
}

impl MapGet {
    fn from_pages(pages: BTreeMap<String, (u16, String)>) -> Self {
        Self {
            pages,
            links: BTreeMap::new(),
        }
    }
}

impl HttpGet for MapGet {
    fn get(&self, url: &str) -> Result<HttpResponse, String> {
        self.pages
            .get(url)
            .map(|(status, body)| HttpResponse {
                status: *status,
                body: body.as_bytes().to_vec(),
                link: self.links.get(url).cloned(),
            })
            .ok_or_else(|| format!("missing {url}"))
    }
}

fn json_page(value: serde_json::Value) -> (u16, String) {
    (200, value.to_string())
}

fn sha(byte: char) -> String {
    byte.to_string().repeat(40)
}

#[test]
fn https_reader_pages_the_open_list_and_keeps_easyconfig_text() {
    let repo = "acme/ec";
    let sha12 = sha('a');
    let sha7 = sha('b');
    let lib = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let new = "easybuild/easyconfigs/a/New/New-1.0.eb";
    let old = "easybuild/easyconfigs/l/Old/Old-1.0.eb";
    let plus = "easybuild/easyconfigs/a/A+/A+-1.0.eb";
    let plus_url = "easybuild/easyconfigs/a/A%2B/A%2B-1.0.eb";
    let mut pages = BTreeMap::new();
    let list = |page: u64| {
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=2&page={page}")
    };
    let files = |number: u64, page: u64| {
        format!("https://api.github.com/repos/{repo}/pulls/{number}/files?per_page=2&page={page}")
    };
    let raw =
        |rev: &str, path: &str| format!("https://raw.githubusercontent.com/{repo}/{rev}/{path}");
    pages.insert(
        list(1),
        json_page(serde_json::json!([
            {"number": 12, "title": "edit Lib", "head": {"sha": sha12}},
            {"number": 3, "title": "readme", "head": {"sha": sha('c')}},
        ])),
    );
    pages.insert(
        list(2),
        json_page(serde_json::json!([
            {"number": 7, "title": "add A+", "head": {"sha": sha7}},
        ])),
    );
    pages.insert(
        files(12, 1),
        json_page(serde_json::json!([
            {"filename": lib, "status": "modified"},
            {"filename": "README.md", "status": "modified"},
        ])),
    );
    pages.insert(
        files(12, 2),
        json_page(serde_json::json!([
            {
                "filename": new,
                "status": "renamed",
                "previous_filename": old,
            },
        ])),
    );
    pages.insert(
        files(3, 1),
        json_page(serde_json::json!([
            {"filename": "docs/readme.md", "status": "modified"},
        ])),
    );
    pages.insert(
        files(7, 1),
        json_page(serde_json::json!([
            {"filename": plus, "status": "added"},
        ])),
    );
    pages.insert(raw(&sha12, lib), (200, "name = 'Lib'\n".into()));
    pages.insert(raw("develop", lib), (200, "name = 'Lib'\nbase\n".into()));
    pages.insert(raw(&sha12, new), (200, "name = 'New'\n".into()));
    pages.insert(raw("develop", new), (404, "missing".into()));
    pages.insert(raw("develop", old), (200, "name = 'Old'\n".into()));
    pages.insert(raw(&sha7, plus_url), (200, "name = 'A+'\n".into()));
    pages.insert(raw("develop", plus_url), (404, "missing".into()));

    let mut query = OpenEasyconfigQuery::new(repo);
    query.page_size = Some(2);
    let prs = fetch_open_easyconfig_prs(&MapGet::from_pages(pages), &query).unwrap();
    assert_eq!(
        prs.iter().map(|pr| pr.number).collect::<Vec<_>>(),
        vec![7, 12]
    );
    assert_eq!(prs[0].head_sha, sha7);
    assert_eq!(prs[0].easyconfigs.len(), 1);
    assert_eq!(prs[0].easyconfigs[0].path, plus);
    assert_eq!(prs[0].easyconfigs[0].head.as_deref(), Some("name = 'A+'\n"));
    assert!(prs[0].easyconfigs[0].base.is_none());
    let paths: Vec<_> = prs[1]
        .easyconfigs
        .iter()
        .map(|change| change.path.as_str())
        .collect();
    assert_eq!(paths, vec![new, lib, old]);
    let by_path: BTreeMap<_, _> = prs[1]
        .easyconfigs
        .iter()
        .map(|change| (change.path.as_str(), change))
        .collect();
    assert_eq!(by_path[lib].head.as_deref(), Some("name = 'Lib'\n"));
    assert_eq!(by_path[lib].base.as_deref(), Some("name = 'Lib'\nbase\n"));
    assert!(by_path[new].base.is_none());
    assert_eq!(by_path[old].head, None);
    assert_eq!(by_path[old].base.as_deref(), Some("name = 'Old'\n"));
}

#[test]
fn https_reader_stops_at_the_requested_page_and_file_caps() {
    let repo = "acme/ec";
    let head = sha('d');
    let first = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let second = "easybuild/easyconfigs/n/New/New-1.0.eb";
    let mut pages = BTreeMap::new();
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls?state=open&per_page=10&page=1".into(),
        json_page(serde_json::json!([
            {"number": 1, "title": "two files", "head": {"sha": head}},
            {"number": 2, "title": "later", "head": {"sha": head}},
        ])),
    );
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls/1/files?per_page=10&page=1".into(),
        json_page(serde_json::json!([
            {"filename": first, "status": "added"},
            {"filename": second, "status": "added"},
        ])),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{first}"),
        (200, "name = 'Lib'\n".into()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/develop/{first}"),
        (404, "missing".into()),
    );

    let mut capped = OpenEasyconfigQuery::new(repo);
    capped.page_size = Some(10);
    capped.max_pages = Some(1);
    capped.max_prs = Some(1);
    capped.max_files = Some(1);
    let prs = fetch_open_easyconfig_prs(&MapGet::from_pages(pages), &capped).unwrap();
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].number, 1);
    assert_eq!(prs[0].easyconfigs.len(), 1);
    assert_eq!(prs[0].easyconfigs[0].path, first);
}

#[test]
fn https_reader_does_not_read_past_max_pages() {
    let mut pages = BTreeMap::new();
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls?state=open&per_page=2&page=1".into(),
        json_page(serde_json::json!([
            {"number": 1, "title": "readme", "head": {"sha": sha('e')}},
            {"number": 2, "title": "notes", "head": {"sha": sha('f')}},
        ])),
    );
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls/1/files?per_page=2&page=1".into(),
        json_page(serde_json::json!([{"filename": "README.md", "status": "modified"}])),
    );
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls/2/files?per_page=2&page=1".into(),
        json_page(serde_json::json!([{"filename": "NOTES.md", "status": "modified"}])),
    );
    let mut query = OpenEasyconfigQuery::new("acme/ec");
    query.page_size = Some(2);
    query.max_pages = Some(1);
    let prs = fetch_open_easyconfig_prs(&MapGet::from_pages(pages), &query).unwrap();
    assert!(prs.is_empty());
}

#[test]
fn https_reader_accepts_base_ref_with_slashes() {
    let repo = "acme/ec";
    let head = sha('g');
    let file = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let mut pages = BTreeMap::new();
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls?state=open&per_page=100&page=1".into(),
        json_page(serde_json::json!([
            {"number": 11, "title": "lib", "head": {"sha": head}},
        ])),
    );
    pages.insert(
        "https://api.github.com/repos/acme/ec/pulls/11/files?per_page=100&page=1".into(),
        json_page(serde_json::json!([
            {"filename": file, "status": "modified"},
        ])),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{file}"),
        (200, "name = 'Lib'\nversion = '1.0'\n".into()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/release/2026.1/{file}"),
        (200, "name = 'Lib'\nversion = '0.9'\n".into()),
    );
    let mut query = OpenEasyconfigQuery::new(repo);
    query.base_ref = "release/2026.1".into();
    let prs = fetch_open_easyconfig_prs(&MapGet::from_pages(pages), &query)
        .expect("base refs with slashes are valid git refs");
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].number, 11);
    assert_eq!(prs[0].easyconfigs.len(), 1);
}

#[test]
fn https_reader_rejects_a_base_ref_that_is_not_a_git_ref() {
    for base_ref in [
        "foo..bar",
        "release/../secret",
        "release//2026.1",
        "/release",
        "foo.lock",
    ] {
        let mut query = OpenEasyconfigQuery::new("acme/ec");
        query.base_ref = base_ref.into();
        let error = fetch_open_easyconfig_prs(&MapGet::from_pages(BTreeMap::new()), &query)
            .expect_err(base_ref)
            .to_string();
        assert!(
            !error.contains("missing https://"),
            "{base_ref} reached the network client: {error}"
        );
        assert!(error.contains("git ref"), "{base_ref}: {error}");
    }
}

#[test]
fn https_reader_rejects_a_repo_that_is_not_owner_name() {
    let query = OpenEasyconfigQuery::new("nope");
    let error = fetch_open_easyconfig_prs(&MapGet::from_pages(BTreeMap::new()), &query)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("owner/name") || error.contains("repo"),
        "{error}"
    );
}

fn easyconfigs_tree() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/pr_overlap_easyconfigs_fixtures")
}

fn open_request(
    out: &Path,
    repo: &str,
    base_ref: Option<&str>,
) -> eb_stack::site_commands::PrOverlapRequest {
    eb_stack::site_commands::PrOverlapRequest {
        prs: Vec::new(),
        kind: eb_stack::site_commands::OverlapKind::Easyconfigs,
        easyconfigs: vec![easyconfigs_tree()],
        commit_trees: Vec::new(),
        installed: None,
        eessi_version: String::new(),
        installed_label: String::new(),
        exclude_paths: Vec::new(),
        include_paths: Vec::new(),
        filter_deps: Vec::new(),
        installed_modulerc: Vec::new(),
        easyblocks: Vec::new(),
        easyblocks_commits: Vec::new(),
        generated_at: Some("2026-10-24T00:00:00Z".into()),
        merge_order: false,
        merged: Vec::new(),
        impact: Vec::new(),
        repo_url: None,
        out_dir: out.to_path_buf(),
        open_repo: Some(repo.into()),
        base_ref: base_ref.map(str::to_string),
        bot_build_set: false,
        arch_installed: Vec::new(),
    }
}

fn dir_file_count(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }
    std::fs::read_dir(path).unwrap().count()
}

#[test]
fn open_flag_reads_easyconfigs_through_the_fetch_client() {
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "pr-overlap",
            "--kind",
            "easyconfigs",
            "--open",
            "nope",
            "--easyconfigs",
        ])
        .arg(easyconfigs_tree())
        .args(["--generated-at", "2026-10-24T00:00:00Z", "--out-dir"])
        .arg(&out)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&status.stderr);
    assert_eq!(status.status.code(), Some(2), "{err}");
    assert!(
        err.contains("owner/name"),
        "expected the open fetch to reject the repo, got {err}"
    );
    assert!(!err.contains("unexpected argument"), "{err}");
    assert_eq!(
        dir_file_count(&out),
        0,
        "a rejected --open wrote under {out:?}"
    );
}

#[test]
fn open_flag_rejects_a_software_layer_run() {
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["pr-overlap", "--open", "acme/ec", "--easyconfigs"])
        .arg(easyconfigs_tree())
        .args(["--out-dir"])
        .arg(&out)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&status.stderr);
    assert_eq!(status.status.code(), Some(2), "{err}");
    assert!(err.contains("--open needs --kind easyconfigs"), "{err}");
    assert_eq!(dir_file_count(&out), 0, "{out:?}");
}

#[test]
fn open_runner_writes_the_pull_request_the_client_returns() {
    let repo = "acme/ec";
    let head = sha('h');
    let file = "easybuild/easyconfigs/f/Fetched/Fetched-1.0.eb";
    let body = "\
easyblock = 'ConfigureMake'\n\
\n\
name = 'Fetched'\n\
version = '1.0'\n\
\n\
homepage = 'https://example.org/Fetched'\n\
description = \"Fetched.\"\n\
\n\
toolchain = SYSTEM\n\
\n\
source_urls = ['https://example.org/']\n\
sources = ['Fetched-1.0.tar.gz']\n\
checksums = ['0000000000000000000000000000000000000000000000000000000000000000']\n\
\n\
moduleclass = 'lib'\n";
    let mut pages = BTreeMap::new();
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=100&page=1"),
        json_page(serde_json::json!([
            {"number": 11, "title": "fetch", "head": {"sha": head}},
        ])),
    );
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls/11/files?per_page=100&page=1"),
        json_page(serde_json::json!([
            {"filename": file, "status": "added"},
        ])),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{file}"),
        (200, body.to_string()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/develop/{file}"),
        (404, "missing".into()),
    );
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out");
    let report = eb_stack::site_commands::run_pr_overlap_with(
        &open_request(&out, repo, None),
        &MapGet::from_pages(pages),
    )
    .expect("open fetch");
    assert_eq!(report.prs, 1);
    assert_eq!(report.records, 1);
    let record = std::fs::read_to_string(out.join("prs/11.toml")).unwrap();
    assert!(record.contains("Fetched/1.0"), "{record}");
}

#[test]
fn open_runner_rejects_a_base_ref_before_any_request() {
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out");
    let error = match eb_stack::site_commands::run_pr_overlap_with(
        &open_request(&out, "acme/ec", Some("foo..bar")),
        &MapGet::from_pages(BTreeMap::new()),
    ) {
        Ok(_) => panic!("bad ref was accepted"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("git ref"), "{error}");
    assert!(!error.contains("missing https://"), "{error}");
    assert_eq!(dir_file_count(&out), 0);
}

#[test]
fn mcp_open_rejects_a_repo_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    let out = temp.path().join("out");
    let response = eb_stack::mcp::handle_message(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_pr_overlap",
            "arguments": {
                "kind": "easyconfigs",
                "open": "nope",
                "easyconfigs": [easyconfigs_tree()],
                "generated_at": "2026-10-24T00:00:00Z",
                "out_dir": out,
            }
        }
    }))
    .expect("rpc");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.contains("owner/name"), "{text}");
    assert_eq!(dir_file_count(&out), 0, "{out:?}");
}

fn files_url(repo: &str, number: u64, page_size: usize, page: usize) -> String {
    format!(
        "https://api.github.com/repos/{repo}/pulls/{number}/files?per_page={page_size}&page={page}"
    )
}

#[test]
fn https_reader_reads_a_file_list_past_twenty_pages() {
    let repo = "acme/ec";
    let head = sha('p');
    let file = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let mut pages = BTreeMap::new();
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=1&page=1"),
        json_page(serde_json::json!([
            {"number": 21, "title": "many files", "head": {"sha": head}},
        ])),
    );
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=1&page=2"),
        json_page(serde_json::json!([])),
    );
    for page in 1..=21 {
        let filename = if page == 21 {
            file.to_string()
        } else {
            format!("notes/{page}.md")
        };
        pages.insert(
            files_url(repo, 21, 1, page),
            json_page(serde_json::json!([{"filename": filename, "status": "added"}])),
        );
    }
    pages.insert(files_url(repo, 21, 1, 22), json_page(serde_json::json!([])));
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{file}"),
        (200, "name = 'Lib'\n".into()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/develop/{file}"),
        (404, "missing".into()),
    );
    let mut query = OpenEasyconfigQuery::new(repo);
    query.page_size = Some(1);
    let prs = fetch_open_easyconfig_prs(&MapGet::from_pages(pages), &query).unwrap();
    assert_eq!(prs.len(), 1, "{prs:?}");
    assert!(!prs[0].file_list_truncated);
    assert_eq!(prs[0].easyconfigs.len(), 1);
    assert_eq!(prs[0].easyconfigs[0].path, file);
}

#[test]
fn https_reader_stops_on_a_full_page_when_the_link_header_has_no_next() {
    let repo = "acme/ec";
    let head = sha('r');
    let file = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let mut pages = BTreeMap::new();
    let mut links = BTreeMap::new();
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=2&page=1"),
        json_page(serde_json::json!([
            {"number": 22, "title": "full last page", "head": {"sha": head}},
        ])),
    );
    let first = files_url(repo, 22, 2, 1);
    let second = files_url(repo, 22, 2, 2);
    pages.insert(
        first.clone(),
        json_page(serde_json::json!([
            {"filename": "README.md", "status": "modified"},
            {"filename": "NOTES.md", "status": "modified"},
        ])),
    );
    links.insert(
        first,
        format!("<{second}>; rel=\"next\", <{second}>; rel=\"last\""),
    );
    links.insert(second.clone(), format!("<{second}>; rel=\"last\""));
    pages.insert(
        second,
        json_page(serde_json::json!([
            {"filename": file, "status": "added"},
            {"filename": "docs/x.md", "status": "added"},
        ])),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{file}"),
        (200, "name = 'Lib'\n".into()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/develop/{file}"),
        (404, "missing".into()),
    );
    let mut query = OpenEasyconfigQuery::new(repo);
    query.page_size = Some(2);
    let prs = fetch_open_easyconfig_prs(&MapGet { pages, links }, &query)
        .expect("a full last page with no rel=next ends the list");
    assert_eq!(prs.len(), 1);
    assert!(!prs[0].file_list_truncated);
    assert_eq!(prs[0].easyconfigs[0].path, file);
}

#[test]
fn https_reader_reports_a_file_list_truncated_at_githubs_limit() {
    let repo = "acme/ec";
    let head = sha('q');
    let file = "easybuild/easyconfigs/l/Lib/Lib-1.0.eb";
    let mut pages = BTreeMap::new();
    let mut links = BTreeMap::new();
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=2&page=1"),
        json_page(serde_json::json!([
            {"number": 30, "title": "capped", "head": {"sha": head}},
        ])),
    );
    let page1 = files_url(repo, 30, 2, 1);
    let page2 = files_url(repo, 30, 2, 2);
    let page3 = files_url(repo, 30, 2, 3);
    pages.insert(
        page1.clone(),
        json_page(serde_json::json!([
            {"filename": file, "status": "added"},
            {"filename": "README.md", "status": "modified"},
        ])),
    );
    links.insert(page1, format!("<{page2}>; rel=\"next\""));
    pages.insert(
        page2.clone(),
        json_page(serde_json::json!([
            {"filename": "a.md", "status": "added"},
            {"filename": "b.md", "status": "added"},
        ])),
    );
    links.insert(page2, format!("<{page3}>; rel=\"next\""));
    pages.insert(
        page3,
        (
            422,
            "{\"message\":\"In order to keep the API fast for everyone, pagination is limited for this resource. Responses include a maximum of 3000 files.\",\"documentation_url\":\"https://docs.github.com/rest/pulls/pulls#list-pull-requests-files\"}".into(),
        ),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{file}"),
        (200, "name = 'Lib'\n".into()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/develop/{file}"),
        (404, "missing".into()),
    );
    let mut query = OpenEasyconfigQuery::new(repo);
    query.page_size = Some(2);
    let prs = fetch_open_easyconfig_prs(&MapGet { pages, links }, &query)
        .expect("GitHub's file cap is a truncated result");
    assert_eq!(prs.len(), 1, "{prs:?}");
    assert!(prs[0].file_list_truncated);
    assert_eq!(prs[0].number, 30);
    assert_eq!(prs[0].easyconfigs.len(), 1);
    assert_eq!(prs[0].easyconfigs[0].path, file);
    let record = easyconfig_record(
        &prs[0],
        &[],
        None,
        &OverlapConfig {
            eessi_version: String::new(),
            installed_source: String::new(),
            exclude_paths: Vec::new(),
            include_paths: Vec::new(),
            generated_at: "2026-10-24T00:00:00Z".into(),
            filter_deps: eb_stack::dep_filter::FilterDeps::empty(),
            easyblocks: None,
            build_scope: eb_stack::pr_overlap::BuildScope::Added,
        },
    )
    .expect("record");
    assert_eq!(record.notes, [GITHUB_PR_FILES_TRUNCATED]);
}

#[test]
#[ignore]
fn live_github_open_easyconfigs_pull_request() {
    let mut query = OpenEasyconfigQuery::new("easybuilders/easybuild-easyconfigs");
    query.max_pages = Some(1);
    query.max_prs = Some(1);
    query.max_files = Some(1);
    query.page_size = Some(20);
    let prs = fetch_open_easyconfig_prs(&UreqGet, &query).expect("public GitHub API");
    assert!(
        !prs.is_empty(),
        "no easyconfig in the first 20 open pull requests"
    );
    let pr = &prs[0];
    let change = pr
        .easyconfigs
        .iter()
        .find(|change| change.path.ends_with(".eb"))
        .unwrap();
    eprintln!(
        "live pr {} {} {} head_bytes {} base_bytes {}",
        pr.number,
        &pr.head_sha[..12],
        change.path,
        change.head.as_ref().map(String::len).unwrap_or(0),
        change.base.as_ref().map(String::len).unwrap_or(0),
    );
    assert_eq!(pr.head_sha.len(), 40);
    assert!(pr.easyconfigs.iter().any(|change| {
        change.path.ends_with(".eb")
            && change
                .head
                .as_ref()
                .or(change.base.as_ref())
                .is_some_and(|text| text.contains("name"))
    }));
}

#[test]
fn software_layer_reader_keeps_easystack_text_and_skips_other_prs() {
    let repo = "acme/sl";
    let head = sha('c');
    let docs = sha('d');
    let stack = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.1-2026.1.yml";
    let base_text = "easyconfigs:\n  - A-1.eb\n";
    let head_text = "easyconfigs:\n  - A-1.eb\n  - B-2.eb\n";
    let mut pages = BTreeMap::new();
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls?state=open&per_page=100&page=1"),
        json_page(serde_json::json!([
            {"number": 9, "title": "B 2", "head": {"sha": head}},
            {"number": 10, "title": "docs", "head": {"sha": docs}},
        ])),
    );
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls/9/files?per_page=100&page=1"),
        json_page(serde_json::json!([
            {"filename": stack, "status": "modified"},
            {"filename": "README.md", "status": "modified"},
        ])),
    );
    pages.insert(
        format!("https://api.github.com/repos/{repo}/pulls/10/files?per_page=100&page=1"),
        json_page(serde_json::json!([{"filename": "docs/x.md", "status": "added"}])),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/main/{stack}"),
        (200, base_text.into()),
    );
    pages.insert(
        format!("https://raw.githubusercontent.com/{repo}/{head}/{stack}"),
        (200, head_text.into()),
    );
    let mut query = OpenEasyconfigQuery::new(repo);
    query.base_ref = "main".into();
    let prs = fetch_open_software_layer_prs(&MapGet::from_pages(pages), &query).unwrap();
    assert_eq!(prs.len(), 1, "{prs:?}");
    assert_eq!(prs[0].number, 9);
    assert_eq!(prs[0].easystacks.len(), 1);
    assert_eq!(prs[0].easystacks[0].path, stack);
    assert_eq!(prs[0].easystacks[0].base.as_deref(), Some(base_text));
    assert_eq!(prs[0].easystacks[0].head.as_deref(), Some(head_text));
    assert!(prs[0].easyconfigs.is_empty());
}
