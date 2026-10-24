//! The dependency-closure page shows command output. Re-run each bash block
//! and diff the following text block.

use std::process::Command;

fn source_blocks(org: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = org;
    while let Some(start) = rest.find("#+begin_src ") {
        rest = &rest[start + "#+begin_src ".len()..];
        let newline = rest.find('\n').expect("source block header");
        let lang = rest[..newline]
            .split_whitespace()
            .next()
            .expect("source language")
            .to_string();
        rest = &rest[newline + 1..];
        let end = rest.find("#+end_src").expect("source block end");
        let body = rest[..end].to_string();
        rest = &rest[end + "#+end_src".len()..];
        out.push((lang, body));
    }
    out
}

#[test]
fn the_dependency_closure_page_matches_fresh_command_output() {
    let org = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/orgmode/explanation/sbom-blast-radius.org"
    ))
    .expect("page");
    let blocks = source_blocks(&org);
    assert!(
        blocks.iter().any(|(lang, _)| lang == "bash"),
        "the page has no commands"
    );
    let demo = tempfile::tempdir().expect("demo dir");
    let demo_path = demo.path().to_str().expect("utf-8 demo path");
    let bin_home = tempfile::tempdir().expect("bin dir");
    std::os::unix::fs::symlink(
        env!("CARGO_BIN_EXE_eb-stack"),
        bin_home.path().join("eb-stack"),
    )
    .expect("symlink eb-stack");
    let path = format!(
        "{}:{}",
        bin_home.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut index = 0;
    let mut compared = 0;
    while index < blocks.len() {
        let (lang, body) = &blocks[index];
        assert_eq!(
            lang, "bash",
            "text block {index} is not the output of a command"
        );
        let script = body.replace("/tmp/sbom-demo", demo_path);
        let output = Command::new("bash")
            .arg("-c")
            .arg(&script)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env("PATH", &path)
            .output()
            .expect("run bash block");
        let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
        let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
        assert!(
            output.status.success(),
            "block {index} failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        if index + 1 < blocks.len() && blocks[index + 1].0 == "text" {
            let expected = blocks[index + 1].1.replace("/tmp/sbom-demo", demo_path);
            assert_eq!(
                stdout.trim_end_matches('\n'),
                expected.trim_end_matches('\n'),
                "block {} stdout drifted",
                index + 1
            );
            compared += 1;
            index += 2;
        } else {
            index += 1;
        }
    }
    assert!(compared > 20, "compared {compared} outputs");
}
