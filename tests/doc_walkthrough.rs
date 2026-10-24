//! The tutorial, how-tos, reference, and explanation pages are commands
//! a checkout can run.
//!
//! Bash blocks must not point at a path this repository does not have.
//! The report commands are executed, and the page must quote their output.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn org_files() -> Vec<PathBuf> {
    let mut files = vec![root().join("docs/orgmode/tutorial.org")];
    for dir in ["howto", "reference", "explanation"] {
        let path = root().join("docs/orgmode").join(dir);
        let mut names: Vec<_> = std::fs::read_dir(&path)
            .unwrap_or_else(|_| panic!("{dir}"))
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "org"))
            .collect();
        names.sort();
        files.append(&mut names);
    }
    files
}

fn bash_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("#+begin_src bash\n") {
        rest = &rest[start + "#+begin_src bash\n".len()..];
        let end = rest.find("#+end_src").expect("bash block closed");
        blocks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    blocks
}

const FORBIDDEN: &[&str] = &[
    "/path/to",
    "/cvmfs/",
    "/workspace/",
    "site.toml",
    "<sha256",
    "SHA256",
    "vendor/easybuild",
    "CAMPAIGN.json",
    "BASE.toml",
    "SITE.toml",
    "App-1.0",
    "eb-stack/scripts",
    "/shared/",
];

#[test]
fn howto_bash_blocks_name_files_in_this_checkout() {
    for path in org_files() {
        let text = std::fs::read_to_string(&path).expect("read");
        let rel = path.strip_prefix(root()).unwrap_or(&path);
        for word in ["ships", "landed", "blocked on", "settle"] {
            assert!(!text.contains(word), "{} contains {word:?}", rel.display());
        }
        for block in bash_blocks(&text) {
            for token in FORBIDDEN {
                assert!(
                    !block.contains(token),
                    "{} bash block contains {token:?}:\n{block}",
                    rel.display()
                );
            }
            for word in block.split_whitespace() {
                let word = word
                    .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | ',' | '\\' | '(' | ')'));
                if word.contains(['*', '$', '~', '{']) {
                    continue;
                }
                let Some(rel_path) = word
                    .strip_prefix("fixtures/")
                    .or_else(|| word.strip_prefix("tests/"))
                    .or_else(|| word.strip_prefix("examples/"))
                    .or_else(|| word.strip_prefix("skills/"))
                    .or_else(|| word.strip_prefix("scripts/"))
                else {
                    continue;
                };
                let _ = rel_path;
                let full = root().join(word);
                assert!(full.exists(), "{} names missing path {word}", rel.display());
            }
        }
    }
}

#[test]
fn report_howtos_quote_the_fixture_commands() {
    let tmp = tempfile::tempdir().expect("temp");
    let overlap = tmp.path().join("overlap");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "pr-overlap",
            "--pr",
            "tests/pr_overlap_layer/prs.json",
            "--easyconfigs",
            "tests/pr_overlap_layer",
            "--installed",
            "tests/pr_overlap_layer/installed.txt",
            "--installed-label",
            "none",
            "--generated-at",
            "2026-10-24T00:00:00Z",
            "--repo-url",
            "https://github.com/EESSI/software-layer",
            "--eessi-version",
            "2026.06",
            "--merge-order",
            "--out-dir",
        ])
        .arg(&overlap)
        .output()
        .expect("pr-overlap");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("prs=2 records=2 shared_modules=91"),
        "{stdout}"
    );
    let order = std::fs::read_to_string(overlap.join("merge-order.toml")).unwrap();
    assert!(order.contains("position = 1\npr = 1585\n"), "{order}");
    assert!(order.contains("pr = 1728\n"), "{order}");
    assert!(order.contains("after = [1585]"), "{order}");
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/pr-overlap.org")).unwrap();
    assert!(page.contains("prs=2 records=2 shared_modules=91"), "{page}");
    assert!(page.contains("tests/pr_overlap_layer/prs.json"));
    assert!(page.contains("prs=4 records=4 shared_modules=1"));
    assert!(page.contains("tests/pr_overlap_easyconfigs_open/prs.json"));

    let easy = tmp.path().join("easy");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "pr-overlap",
            "--kind",
            "easyconfigs",
            "--pr",
            "tests/pr_overlap_easyconfigs_open/prs.json",
            "--easyconfigs",
            "tests/pr_overlap_easyconfigs_open/tree",
            "--generated-at",
            "2026-10-24T00:00:00Z",
            "--repo-url",
            "https://github.com/easybuilders/easybuild-easyconfigs",
            "--out-dir",
        ])
        .arg(&easy)
        .output()
        .expect("easyconfigs overlap");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let got = std::fs::read_to_string(easy.join("overlap.md")).unwrap();
    let want = std::fs::read_to_string(
        root().join("tests/pr_overlap_easyconfigs_open/expected/overlap.md"),
    )
    .unwrap();
    assert_eq!(got, want);

    let reach = tmp.path().join("reach");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "component-reach",
            "--component",
            "curl",
            "--version",
            ">=8,<9",
            "--generation",
            "2026.1",
            "--easyconfigs",
            "tests/component_reach_recorded",
            "--out-dir",
        ])
        .arg(&reach)
        .output()
        .expect("component-reach");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("carriers=2 changed=1 must-rebuild=1 review=0 no-rebuild=1"),
        "{stdout}"
    );
    let page =
        std::fs::read_to_string(root().join("docs/orgmode/howto/component-reach.org")).unwrap();
    assert!(page.contains("carriers=2 changed=1 must-rebuild=1 review=0 no-rebuild=1"));
    assert!(page.contains("tests/component_reach_recorded"));
    assert!(page.contains("981d2123dddccca9ce0b15f9eb2643e872332f5e"));

    let rebuild = tmp.path().join("rebuild");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "rebuild-set",
            "--changed",
            "tests/rebuild_set_fixtures/changed.txt",
            "--easystack",
            "tests/rebuild_set_fixtures/eessi-2026.06-eb-5.4.0-2026.1.yml",
            "--easystack",
            "tests/rebuild_set_fixtures/20261001-eb-5.4.1-zlib.yml",
            "--easyconfigs",
            "tests/rebuild_set_fixtures/release",
            "--easyblocks",
            "tests/rebuild_set_fixtures/easyblocks",
            "--installed",
            "tests/rebuild_set_fixtures/installed.txt",
            "--installed-modulerc",
            "tests/rebuild_set_fixtures/modules/all",
            "--installed-label",
            "x86_64/amd/zen4",
            "--generated-at",
            "2026-10-22T12:00:00Z",
            "--out-dir",
        ])
        .arg(&rebuild)
        .output()
        .expect("rebuild-set");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.contains("changed=2 rows=8 must-rebuild=2 review=2 no-rebuild=4 missing=1"),
        "{stdout}"
    );
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/rebuild-set.org")).unwrap();
    assert!(page.contains("changed=2 rows=8 must-rebuild=2 review=2 no-rebuild=4 missing=1"));

    let list = tmp.path().join("site.list");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args([
            "stack",
            "solve",
            "--easyconfigs",
            "fixtures/gromacs_2025_to_next/easyconfigs",
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--baseline-easystack",
            "fixtures/gromacs_2025_to_next/easystacks/gromacs-2024.4.yml",
            "--build-list-out",
        ])
        .arg(&list)
        .arg("--build-list-excludes-baseline")
        .arg("--stack-diff-out")
        .arg(tmp.path().join("easystack.diff.md"))
        .arg("--lock-out")
        .arg(tmp.path().join("easystack.lock.json"))
        .status()
        .expect("solve");
    assert!(status.success());
    let list = std::fs::read_to_string(&list).unwrap();
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/emit-reports.org")).unwrap();
    assert!(page.contains(list.trim_end()), "{list}");
    assert!(page.contains("Baseline (`foss-2025b`)"));
}

#[test]
fn foreign_plan_howtos_use_the_fixture_robots() {
    let tmp = tempfile::tempdir().expect("temp");
    let bs4 = tmp.path().join("bs4");
    let status = run_to(
        &[
            "package",
            "plan",
            "--source",
            "fixtures/foreign_ingest/pypi_bs4/pypi.json",
            "--format",
            "pypi",
            "--toolchain",
            "foss/2026.1",
            "--easyconfigs",
            "fixtures/foreign_ingest/pypi_bs4/robot",
            "--stack-policy",
            "examples/stacks/eessi-python-extras.toml",
            "--out-dir",
        ],
        &bs4,
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let recipe = std::fs::read_to_string(
        bs4.join("easyconfigs/b/beautifulsoup4/beautifulsoup4-4.12.3-foss-2026.1.eb"),
    )
    .unwrap();
    assert!(recipe.contains("easyblock = 'PythonPackage'"), "{recipe}");
    assert!(!recipe.contains("exts_list"), "{recipe}");
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/pypi-extend.org")).unwrap();
    assert!(page.contains("fixtures/foreign_ingest/pypi_bs4/robot"));
    assert!(page.contains("easyblock = 'PythonPackage'") || page.contains("~PythonPackage~"));

    let cran = tmp.path().join("cran");
    let status = run_to(
        &[
            "package",
            "plan",
            "--source",
            "fixtures/foreign_ingest/cran_jsonlite/cran.json",
            "--format",
            "cran",
            "--toolchain",
            "foss/2026.1",
            "--easyconfigs",
            "fixtures/foreign_ingest/cran_jsonlite/robot",
            "--stack-policy",
            "examples/stacks/eessi-r-extras.toml",
            "--source-checksum",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--out-dir",
        ],
        &cran,
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let recipe =
        std::fs::read_to_string(cran.join("easyconfigs/j/jsonlite/jsonlite-1.8.8-foss-2026.1.eb"))
            .unwrap();
    assert!(recipe.contains("easyblock = 'Bundle'"), "{recipe}");
    assert!(
        recipe.contains("exts_defaultclass = 'RPackage'"),
        "{recipe}"
    );

    let missing = run_to(
        &[
            "package",
            "plan",
            "--source",
            "fixtures/foreign_ingest/cran_unpinned/cran.json",
            "--format",
            "cran",
            "--toolchain",
            "foss/2026.1",
            "--easyconfigs",
            "fixtures/foreign_ingest/cran_bundle/robot",
            "--stack-policy",
            "examples/stacks/eessi-r-extras.toml",
            "--source-checksum",
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "--out-dir",
        ],
        &tmp.path().join("missing"),
    );
    assert!(!missing.status.success());
    let err = String::from_utf8_lossy(&missing.stderr);
    assert!(err.contains("processx is not in the robot"), "{err}");
    let page = std::fs::read_to_string(root().join("docs/orgmode/howto/cran-extend.org")).unwrap();
    assert!(page.contains("processx is not in the robot"));
    assert!(
        page.contains("easyblock = RPackage")
            || page.contains("~easyblock = RPackage~")
            || page.contains("~RPackage~")
    );
}

#[test]
fn reference_pages_do_not_embed_a_checkout_path() {
    for dir in ["docs/orgmode/reference", "docs/orgmode/explanation"] {
        let mut names: Vec<_> = std::fs::read_dir(root().join(dir))
            .unwrap_or_else(|_| panic!("{dir}"))
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "org"))
            .collect();
        names.sort();
        for path in names {
            let text = std::fs::read_to_string(&path).expect("read");
            let rel = path.strip_prefix(root()).unwrap_or(&path);
            for token in ["/workspace", "/tmp/ec-fixture-prs.json"] {
                assert!(!text.contains(token), "{} contains {token}", rel.display());
            }
        }
    }
}

#[test]
fn easyblock_reach_howto_uses_the_pinned_trees() {
    let page =
        std::fs::read_to_string(root().join("docs/orgmode/howto/easyblock-reach.org")).unwrap();
    assert!(page.contains("scripts/fetch-upstream-trees.sh"));
    assert!(page.contains("EB_EASYBLOCKS"));
    assert!(page.contains("EB_EASYBUILD_FRAMEWORK"));
    assert!(!page.contains("~/src"));
    assert!(!page.contains("-b develop"));
    assert!(!page.contains("git clone"));
    assert!(page.contains("easyconfigs=2352 installed=0 classes=39 reverse=5058"));
    assert!(page.contains("easyconfigs=0 installed=0 classes=0 reverse=0"));
}

/// `sphinx-rustdocgen` joins a `#[path = "..."]` value onto the parent file,
/// then opens that path. A module file has to be `name.rs` or `name/mod.rs`
/// next to its parent.
#[test]
fn rust_modules_use_files_the_api_generator_can_open() {
    fn walk(dir: &Path) {
        for entry in std::fs::read_dir(dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path);
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("read");
                assert!(
                    !text.contains("#[path"),
                    "{} uses #[path]; sphinx-rustdocgen cannot open that module",
                    path.display()
                );
            }
        }
    }
    walk(&root().join("src"));
}

fn run_to(args: &[&str], out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args(args)
        .arg(out)
        .output()
        .expect("spawn")
}
