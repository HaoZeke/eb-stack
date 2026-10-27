//! `eb-stack stack from-commit` against a tiny easyconfigs and easyblocks checkout.

use eb_stack::mcp::handle_message;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _keep: tempfile::TempDir,
    easyconfigs: PathBuf,
    easyblocks: PathBuf,
    c1: String,
    c3: String,
    b1: String,
    demo: PathBuf,
    missing_tag: PathBuf,
    blocks: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("-c")
        .arg("commit.gpgsign=false")
        .arg("-c")
        .arg("user.name=Rohit Goswami")
        .arg("-c")
        .arg("user.email=rgoswami@ieee.org")
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_string()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit_all(dir: &Path, message: &str) -> String {
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn fixture() -> Fixture {
    let keep = tempfile::tempdir().unwrap();
    let root = keep.path();
    let easyconfigs = root.join("easyconfigs");
    let easyblocks = root.join("easyblocks");
    std::fs::create_dir_all(&easyconfigs).unwrap();
    std::fs::create_dir_all(&easyblocks).unwrap();
    git(&easyconfigs, &["init", "-b", "main"]);
    write(&easyconfigs, "README", "root\n");
    let _c0 = commit_all(&easyconfigs, "root");
    git(&easyconfigs, &["tag", "easybuild-easyconfigs-v5.0.0"]);
    write(
        &easyconfigs,
        "easybuild/easyconfigs/f/Foo/Foo-1.0.eb",
        "one\n",
    );
    write(
        &easyconfigs,
        "easybuild/easyconfigs/b/Bar/Bar-1.0.eb",
        "old\n",
    );
    let c1 = commit_all(&easyconfigs, "add Foo and Bar");
    write(
        &easyconfigs,
        "easybuild/easyconfigs/b/Bar/Bar-1.0.eb",
        "new\n",
    );
    let c2 = commit_all(&easyconfigs, "patch Bar");
    git(&easyconfigs, &["tag", "easybuild-easyconfigs-v5.4.0"]);
    write(
        &easyconfigs,
        "easybuild/easyconfigs/n/New/New-1.0.eb",
        "after\n",
    );
    let c3 = commit_all(&easyconfigs, "add New");

    git(&easyblocks, &["init", "-b", "main"]);
    write(&easyblocks, "README", "blocks\n");
    let _b0 = commit_all(&easyblocks, "easyblocks root");
    write(
        &easyblocks,
        "easybuild/easyblocks/generic/foo.py",
        "class Foo\n",
    );
    let b1 = commit_all(&easyblocks, "add foo");
    git(&easyblocks, &["tag", "easybuild-easyblocks-v5.4.0"]);

    let missing = "0123456789abcdef0123456789abcdef01234567";
    let demo = root.join("20261011-eb-5.4.0-UCX.yml");
    std::fs::write(
        &demo,
        format!(
            "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n  - Bar-1.0.eb:\n      options:\n        from-commit: {c1}\n  - New-1.0.eb:\n      options:\n        from-commit: {c3}\n  - Foo-1.0.eb:\n      options:\n        from-commit: {missing}\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c2}\n        include-easyblocks-from-commit: {b1}\n"
        ),
    )
    .unwrap();
    let missing_tag = root.join("20261011-eb-9.9.9-missing.yml");
    std::fs::write(
        &missing_tag,
        format!("easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {c1}\n"),
    )
    .unwrap();
    let blocks = root.join("20261011-eb-5.4.0-blocks.yml");
    let b0 = git(&easyblocks, &["rev-parse", "HEAD~1"]);
    std::fs::write(
        &blocks,
        format!(
            "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        include-easyblocks-from-commit: {b0}\n"
        ),
    )
    .unwrap();

    Fixture {
        _keep: keep,
        easyconfigs,
        easyblocks,
        c1,
        c3,
        b1,
        demo,
        missing_tag,
        blocks,
    }
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("eb-stack")
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf8 stdout")
}

#[test]
fn cli_classifies_pins_and_exits_nonzero_when_one_is_released() {
    let fixture = fixture();
    let output = run(&[
        "stack",
        "from-commit",
        "--easystack",
        fixture.demo.to_str().unwrap(),
        "--easystack",
        fixture.missing_tag.to_str().unwrap(),
        "--easystack",
        fixture.blocks.to_str().unwrap(),
        "--easyconfigs-repo",
        fixture.easyconfigs.to_str().unwrap(),
        "--easyblocks-repo",
        fixture.easyblocks.to_str().unwrap(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = stdout_of(&output);
    assert!(
        text.contains(&format!(
            "status=released blob=same version=5.4.0 tag=easybuild-easyconfigs-v5.4.0 kind=from-commit commit={}",
            fixture.c1
        )),
        "{text}"
    );
    assert!(
        text.contains("file=Bar-1.0.eb") && text.contains("blob=differs"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "status=newer version=5.4.0 tag=easybuild-easyconfigs-v5.4.0 kind=from-commit commit={}",
            fixture.c3
        )),
        "{text}"
    );
    assert!(text.contains("reason=commit-missing"), "{text}");
    assert!(text.contains("reason=tag-missing"), "{text}");
    assert!(text.contains("version=9.9.9"), "{text}");
    assert!(
        text.contains(&format!(
            "status=released blob=same version=5.4.0 tag=easybuild-easyblocks-v5.4.0 kind=include-easyblocks-from-commit commit={}",
            fixture.b1
        )),
        "{text}"
    );
    assert!(
        text.contains("kind=include-easyblocks-from-commit") && text.contains("blob=differs"),
        "{text}"
    );
    assert!(
        text.ends_with("pins=8 released=5 newer=1 unknown=2\n"),
        "{text}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("5 from-commit pin(s) already in the EasyBuild release"),
        "{stderr}"
    );

    let newer_only = run(&[
        "stack",
        "from-commit",
        "--easystack",
        fixture.demo.to_str().unwrap(),
        "--easybuild-version",
        "5.0.0",
        "--easyconfigs-repo",
        fixture.easyconfigs.to_str().unwrap(),
        "--easyblocks-repo",
        fixture.easyblocks.to_str().unwrap(),
    ]);
    // The easyblocks pin is still checked against 5.0.0, whose tag is absent,
    // and the easyconfig pins are newer than v5.0.0. No pin is released.
    assert!(
        newer_only.status.success(),
        "{}",
        String::from_utf8_lossy(&newer_only.stderr)
    );
    let overridden = stdout_of(&newer_only);
    assert!(overridden.contains("version=5.0.0"), "{overridden}");
    assert!(overridden.contains("status=newer"), "{overridden}");
    assert!(!overridden.contains("status=released"), "{overridden}");
    assert!(overridden.contains("reason=tag-missing"), "{overridden}");

    let forced = run(&[
        "stack",
        "from-commit",
        "--easystack",
        fixture.missing_tag.to_str().unwrap(),
        "--easybuild-version",
        "5.4.0",
        "--easyconfigs-repo",
        fixture.easyconfigs.to_str().unwrap(),
    ]);
    assert_eq!(forced.status.code(), Some(1));
    let forced_text = stdout_of(&forced);
    assert!(
        forced_text.contains("status=released blob=same version=5.4.0"),
        "{forced_text}"
    );
    assert!(!forced_text.contains("version=9.9.9"), "{forced_text}");

    let unnamed = fixture._keep.path().join("rebuild.yml");
    std::fs::write(
        &unnamed,
        format!(
            "easyconfigs:\n  - Foo-1.0.eb:\n      options:\n        from-commit: {}\n",
            fixture.c1
        ),
    )
    .unwrap();
    let missing_version = run(&[
        "stack",
        "from-commit",
        "--easystack",
        unnamed.to_str().unwrap(),
        "--easyconfigs-repo",
        fixture.easyconfigs.to_str().unwrap(),
    ]);
    assert_eq!(missing_version.status.code(), Some(2));
    let err = String::from_utf8(missing_version.stderr).unwrap();
    assert!(err.contains("--easybuild-version"), "{err}");
    assert!(err.contains("eb-X.Y.Z"), "{err}");

    let json_out = run(&[
        "stack",
        "from-commit",
        "--easystack",
        fixture.missing_tag.to_str().unwrap(),
        "--easyconfigs-repo",
        fixture.easyconfigs.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        json_out.status.success(),
        "{}",
        String::from_utf8_lossy(&json_out.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout_of(&json_out)).unwrap();
    assert_eq!(parsed["unknown"], 1);
    assert_eq!(parsed["released"], 0);
    assert_eq!(parsed["pins"][0]["reason"], "tag-missing");
    assert_eq!(parsed["pins"][0]["version"], "9.9.9");
}

#[test]
fn mcp_from_commit_matches_the_cli() {
    let fixture = fixture();
    let args = [
        "stack",
        "from-commit",
        "--easystack",
        fixture.demo.to_str().unwrap(),
        "--easystack",
        fixture.blocks.to_str().unwrap(),
        "--easyconfigs-repo",
        fixture.easyconfigs.to_str().unwrap(),
        "--easyblocks-repo",
        fixture.easyblocks.to_str().unwrap(),
        "--json",
    ];
    let cli = run(&args);
    assert_eq!(cli.status.code(), Some(1));
    let cli_stdout = stdout_of(&cli);
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_from_commit",
            "arguments": {
                "easystack": [fixture.demo, fixture.blocks],
                "easyconfigs_repo": fixture.easyconfigs,
                "easyblocks_repo": fixture.easyblocks,
                "json": true
            }
        }
    }))
    .expect("mcp");
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["text"].as_str().unwrap(), cli_stdout);
    assert_eq!(body["released"], 5);
    assert_eq!(body["newer"], 1);
    assert_eq!(body["unknown"], 1);
    let pins = body["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 7);
    assert_eq!(pins[1]["blob"], "differs");
    assert_eq!(pins[1]["status"], "released");
    assert_eq!(pins[2]["status"], "newer");
    assert_eq!(pins[3]["reason"], "commit-missing");
    assert_eq!(pins[5]["kind"], "include-easyblocks-from-commit");

    let cli_err = run(&[
        "stack",
        "from-commit",
        "--easystack",
        fixture.demo.to_str().unwrap(),
    ]);
    assert_eq!(cli_err.status.code(), Some(2));
    let cli_message = String::from_utf8(cli_err.stderr)
        .unwrap()
        .trim_start_matches("error: ")
        .trim()
        .to_string();
    let mcp_err = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_from_commit",
            "arguments": {"easystack": [fixture.demo]}
        }
    }))
    .expect("mcp error");
    assert_eq!(mcp_err["result"]["isError"], true, "{mcp_err}");
    assert_eq!(
        mcp_err["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .trim(),
        cli_message
    );
}
