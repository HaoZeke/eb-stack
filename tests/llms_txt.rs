//! The committed llms.txt index is generated from the Org manual.

use std::path::PathBuf;
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn llms_txt_matches_the_org_manual() {
    let root = repo();
    let output = Command::new("python3")
        .arg(root.join("scripts/gen-llms-txt.py"))
        .arg("--check")
        .current_dir(&root)
        .output()
        .expect("python3 scripts/gen-llms-txt.py --check");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let index = std::fs::read_to_string(root.join("llms.txt")).expect("read llms.txt");
    assert!(index.starts_with("# eb-stack\n"), "{index}");
    assert!(index.contains("\n> eb-stack parses conda-forge,"));
    for section in [
        "## Getting started",
        "## How-to guides",
        "## Reference",
        "## Explanation",
        "## Skills",
        "## Project",
        "## Optional",
    ] {
        assert!(index.contains(section), "missing {section}");
    }
    assert!(index.contains("https://eb-stack.rgoswami.me/tutorial.html.md"));
    assert!(index.contains("https://eb-stack.rgoswami.me/reference/cli.html.md"));
    assert!(index.contains("https://eb-stack.rgoswami.me/reference/mcp.html.md"));
    assert!(index.contains("server.json"));
    assert!(index.contains("skills/new-package/SKILL.md"));
    assert!(index.contains("CONTRIBUTING.md"));

    // A page summary is already markdown, so summarize() must not re-read it
    // as Org. A regression ran inline_plain() over the markdown and paired
    // the equals inside one code span with the next equals in the line,
    // dropping the equals that belongs in the cran-extend summary.
    assert!(
        index.contains("exts_defaultclass = 'RPackage'"),
        "llms.txt dropped the equals in the cran-extend summary"
    );
    assert!(
        !index.contains("exts_defaultclass 'RPackage'"),
        "llms.txt still has the equals-dropping cran-extend summary"
    );

    let full = std::fs::read_to_string(root.join("llms-full.txt")).expect("read llms-full.txt");
    assert!(full.contains("# Command-line reference"));
    assert!(full.contains("eb-stack package plan"));

    // Org code (~...~) and verbatim (=...=) markers each render as one
    // backtick span, even when they contain or sit next to an equals sign.
    // A regression in scripts/gen-llms-txt.py once paired the equals inside
    // one span with the equals of a following span, splicing a backtick over
    // the equals that belonged in the text.
    for span in [
        "`recipes_read=58`",
        "`==0.3.24`",
        "`${workspaceFolder}`",
        "`-DQMC_COMPLEX=ON`",
        "`oom=`",
        "`~/.copilot/mcp-config.json`",
    ] {
        assert!(full.contains(span), "llms-full.txt is missing {span}");
    }
    for mangled in ["`recipes_read`58`", "`=`0.3.24`", "`-DQMC_COMPLEX`ON`"] {
        assert!(
            !full.contains(mangled),
            "llms-full.txt still has the spliced rendering {mangled}"
        );
    }

    let pages = Command::new("python3")
        .arg(root.join("scripts/gen-llms-txt.py"))
        .arg("--pages-only")
        .current_dir(&root)
        .output()
        .expect("python3 scripts/gen-llms-txt.py --pages-only");
    assert!(
        pages.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&pages.stdout),
        String::from_utf8_lossy(&pages.stderr)
    );
    assert!(root.join("docs/llm/reference/cli.md").is_file());
}

/// Every link the generated public files carry stays within the public
/// document manifest, `scripts/leakguard.toml`, which names CONTRIBUTING.md as
/// the contributor entry; `scripts/llms_leakguard.py` is the pre-check
/// `gen-llms-txt.py` runs.
#[test]
fn public_links_stay_within_the_manifest() {
    let root = repo();
    let config =
        std::fs::read_to_string(root.join("scripts/leakguard.toml")).expect("leakguard.toml");
    assert!(config.contains("\"CONTRIBUTING.md\""), "{config}");
    assert!(config.contains("public_repo_prefixes"), "{config}");
    let output = Command::new("python3")
        .arg(root.join("scripts/llms_leakguard.py"))
        .current_dir(&root)
        .output()
        .expect("python3 scripts/llms_leakguard.py");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The manifest check canonicalizes repository links: traversal out of a
/// public directory, encoded dot segments, backslashes, other refs and raw
/// forms all follow the manifest. `scripts/test_llms_leakguard.py` holds the
/// cases.
#[test]
fn the_manifest_check_rejects_traversal_and_other_refs() {
    let root = repo();
    let output = Command::new("python3")
        .args(["-m", "unittest", "scripts.test_llms_leakguard"])
        .current_dir(&root)
        .output()
        .expect("python3 -m unittest scripts.test_llms_leakguard");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
