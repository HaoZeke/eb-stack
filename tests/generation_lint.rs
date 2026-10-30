//! Toolchain-generation lint, seeded from EESSI software-layer pull requests.
//!
//! Each pull-request fixture is the easystack change at a real head commit,
//! trimmed to a few lines. #1472 first put pv on GCCcore-12.3.0 (2023a) into a
//! 2025.06 easystack and was later moved to 2023.06. #1834 first added fastp
//! to 2025.06 and 2026.06 in one pull request and was split on review. #1497
//! added GROMACS 2026.2 to foss-2025b, next to GROMACS 2025.4.

use eb_stack::mcp::handle_message;
use serde_json::json;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/generation_lint_fixtures")
        .join(name)
}

fn path(name: &str) -> String {
    fixture(name).to_str().unwrap().to_string()
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

const PV_2025_06: &str = "pv-1.7.24-GCCcore-12.3.0.eb in eessi-2025.06-eb-5.2.1-2025b.yml is outside EESSI 2025.06: GCCcore-12.3.0 is generation 2023a, which EESSI 2023.06 carries; 2025.06 carries 2024a, 2025a, 2025b";

#[test]
fn pr_1472_first_push_puts_a_2023a_recipe_in_2025_06() {
    let output = run(&[
        "stack",
        "generation",
        "--pr",
        &path("prs/1472-first-push.json"),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert_eq!(
        stdout,
        format!("#1472: {PV_2025_06}\noutside=1 mixed=0 versions=0\n")
    );
    assert!(stderr.contains(PV_2025_06), "{stderr}");
}

#[test]
fn pr_1472_after_the_move_to_2023_06_passes() {
    let output = run(&["stack", "generation", "--pr", &path("prs/1472-moved.json")]);
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert_eq!(text(&output.stdout), "outside=0 mixed=0 versions=0\n");
}

#[test]
fn pr_1472_head_file_as_an_easystack_fails_the_same_way() {
    let output = run(&[
        "stack",
        "generation",
        "--easystack",
        &path("easystacks/software.eessi.io/2025.06/eessi-2025.06-eb-5.2.1-2025b.yml"),
    ]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert_eq!(
        stdout,
        format!("{PV_2025_06}\noutside=1 mixed=0 versions=0\n")
    );
}

#[test]
fn pr_1834_before_the_split_targets_two_eessi_versions() {
    let output = run(&[
        "stack",
        "generation",
        "--pr",
        &path("prs/1834-before-split.json"),
    ]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert_eq!(
        stdout,
        "#1834: the easystacks are for 2 EESSI versions: 2025.06 (eessi-2025.06-eb-5.4.1-2025b.yml), 2026.06 (eessi-2026.06-eb-5.4.1-2026.1.yml); the bot builds one EESSI version per pull request\noutside=0 mixed=1 versions=0\n"
    );

    let split = run(&["stack", "generation", "--pr", &path("prs/1834-split.json")]);
    assert!(split.status.success(), "{}", text(&split.stdout));
}

#[test]
fn pr_1497_adds_a_second_gromacs_to_2025b_and_needs_an_allow() {
    let pr = path("prs/1497.json");
    let refused = run(&["stack", "generation", "--pr", &pr]);
    let stdout = text(&refused.stdout);
    assert_eq!(refused.status.code(), Some(1), "{stdout}");
    assert_eq!(
        stdout,
        "#1497: GROMACS has 2 versions in generation 2025b: 2025.4 (already in the stack), 2026.2 (added: GROMACS-2026.2-foss-2025b.eb); pass --allow-versions GROMACS@2025b to accept\noutside=0 mixed=0 versions=1\n"
    );

    let allowed = run(&[
        "stack",
        "generation",
        "--pr",
        &pr,
        "--allow-versions",
        "GROMACS@2025b",
    ]);
    assert!(allowed.status.success(), "{}", text(&allowed.stdout));

    let other_generation = run(&[
        "stack",
        "generation",
        "--pr",
        &pr,
        "--allow-versions",
        "GROMACS@2025a",
    ]);
    assert_eq!(other_generation.status.code(), Some(1));
}

#[test]
fn deployed_easystacks_count_and_their_own_duplicates_do_not() {
    let output = run(&[
        "stack",
        "generation",
        "--easystack",
        &path("easystacks/1497/eessi-2025.06-eb-5.3.0-2025b.yml"),
        "--deployed",
        &path("deployed"),
    ]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.starts_with("GROMACS has 2 versions in generation 2025b: 2025.4 (already in the stack), 2026.2 (added: GROMACS-2026.2-foss-2025b.eb);"),
        "{stdout}"
    );
    assert!(!stdout.contains("CMake"), "{stdout}");
    assert!(
        stdout.ends_with("outside=0 mixed=0 versions=1\n"),
        "{stdout}"
    );
}

#[test]
fn a_path_for_another_version_than_the_flag_is_a_finding() {
    let output = run(&[
        "stack",
        "generation",
        "--pr",
        &path("prs/1834-split.json"),
        "--eessi-version",
        "2026.06",
    ]);
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains(
            "#1834: eessi-2025.06-eb-5.4.1-2025b.yml is for EESSI 2025.06, not --eessi-version 2026.06"
        ),
        "{stdout}"
    );
}

#[test]
fn usage_errors_exit_2() {
    let none = run(&["stack", "generation"]);
    assert_eq!(none.status.code(), Some(2), "{}", text(&none.stderr));
    assert!(text(&none.stderr).contains("pass at least one --easystack or --pr"));

    let dir = tempfile::tempdir().expect("tempdir");
    let bare = dir.path().join("stack.yml");
    std::fs::write(&bare, "easyconfigs:\n  - zlib-1.3.1-GCCcore-14.3.0.eb\n").unwrap();
    let unnamed = run(&["stack", "generation", "--easystack", bare.to_str().unwrap()]);
    assert_eq!(unnamed.status.code(), Some(2), "{}", text(&unnamed.stderr));
    assert!(text(&unnamed.stderr).contains("pass --eessi-version"));

    let named = run(&[
        "stack",
        "generation",
        "--easystack",
        bare.to_str().unwrap(),
        "--eessi-version",
        "2025.06",
    ]);
    assert!(named.status.success(), "{}", text(&named.stderr));

    let bad_allow = run(&[
        "stack",
        "generation",
        "--pr",
        &path("prs/1497.json"),
        "--allow-versions",
        "@2025b",
    ]);
    assert_eq!(
        bad_allow.status.code(),
        Some(2),
        "{}",
        text(&bad_allow.stderr)
    );
}

#[test]
fn the_howto_quotes_the_output_of_each_command() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let page = std::fs::read_to_string(root.join("docs/orgmode/howto/generation-lint.org"))
        .expect("how-to page");
    let mut rest = page.as_str();
    let mut checked = 0;
    while let Some(start) = rest.find("#+begin_src bash\n") {
        rest = &rest[start + "#+begin_src bash\n".len()..];
        let end = rest.find("#+end_src").expect("bash block closed");
        let words: Vec<&str> = rest[..end]
            .split_whitespace()
            .filter(|word| *word != "\\")
            .collect();
        rest = &rest[end..];
        let open = rest
            .find("#+begin_example\n")
            .expect("example after command");
        rest = &rest[open + "#+begin_example\n".len()..];
        let close = rest.find("#+end_example").expect("example closed");
        let quoted = &rest[..close];
        assert_eq!(words.first(), Some(&"eb-stack"), "{words:?}");
        let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
            .current_dir(&root)
            .args(&words[1..])
            .output()
            .expect("run eb-stack");
        assert_eq!(text(&output.stdout), quoted, "{words:?}");
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[test]
fn the_mcp_tool_refuses_the_same_plans() {
    let call = |arguments: serde_json::Value| {
        handle_message(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "eb_stack_generation", "arguments": arguments}
        }))
        .expect("tools/call")
    };
    let refused = call(json!({"pr": [path("prs/1472-first-push.json")]}));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let message = refused["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(message, format!("#1472: {PV_2025_06}"));

    let allowed = call(json!({
        "pr": [path("prs/1497.json")],
        "allow_versions": ["GROMACS@2025b"]
    }));
    assert_eq!(allowed["result"]["isError"], false, "{allowed}");
    assert_eq!(
        allowed["result"]["structuredContent"]["text"],
        "outside=0 mixed=0 versions=0\n"
    );
}
