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
    assert!(index.contains("AGENTS.md"));

    let full = std::fs::read_to_string(root.join("llms-full.txt")).expect("read llms-full.txt");
    assert!(full.contains("# Command-line reference"));
    assert!(full.contains("eb-stack package plan"));

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
