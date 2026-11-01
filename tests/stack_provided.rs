//! `stack provided` fails when a build list names baseline software.

use eb_stack::mcp::handle_message;
use serde_json::json;
use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run(extra: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command.current_dir(root()).args([
        "stack",
        "provided",
        "--build-list",
        "tests/provided_fixtures/site.list",
        "--installed",
        "tests/provided_fixtures/installed.txt",
        "--baseline-lock",
        "tests/provided_fixtures/baseline.lock.json",
    ]);
    command.args(extra);
    command.output().expect("stack provided")
}

#[test]
fn a_line_the_baseline_provides_exits_1_and_an_allow_keeps_it() {
    let failed = run(&[]);
    let stdout = String::from_utf8(failed.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert_eq!(failed.status.code(), Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("provided FFTW-3.3.10-gompi-2026.1.eb name=FFTW "),
        "{stdout}"
    );
    assert!(stdout.contains("name=Python "), "{stdout}");
    assert!(stdout.contains("name=GROMACS "), "{stdout}");
    assert!(stdout.contains("unparsed not-a-version.eb "), "{stdout}");
    assert!(!stdout.contains("VASP"), "{stdout}");
    assert!(
        stderr.contains("3 build-list lines name software the baseline already provides"),
        "{stderr}"
    );
    assert!(
        stderr.contains("1 build-list line names no package"),
        "{stderr}"
    );

    let allowed = run(&["--allow", "gromacs"]);
    let stdout = String::from_utf8(allowed.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&allowed.stderr);
    assert_eq!(allowed.status.code(), Some(1), "{stdout}{stderr}");
    assert!(!stdout.contains("name=GROMACS "), "{stdout}");
    assert!(stdout.contains("allowed=1"), "{stdout}");
    assert!(stdout.contains("hits=2"), "{stdout}");
}

#[test]
fn a_site_only_list_exits_0() {
    let dir = tempfile::tempdir().expect("temp");
    let list = dir.path().join("site.list");
    std::fs::write(
        &list,
        "VASP-6.5.1-foss-2026.1.eb\n# FFTW-3.3.10-gompi-2026.1.eb\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "provided", "--build-list"])
        .arg(&list)
        .arg("--installed")
        .arg(root().join("tests/provided_fixtures/installed.txt"))
        .output()
        .expect("stack provided");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "hits=0 unparsed=0 allowed=0 provided=2 toolchain=0 hooks=0 family=0 licensed=0\n"
        ),
        "{stdout}"
    );
}

#[test]
fn missing_baseline_is_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args([
            "stack",
            "provided",
            "--build-list",
            "tests/provided_fixtures/site.list",
        ])
        .current_dir(root())
        .output()
        .expect("stack provided");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("--installed or --baseline-lock"),
        "{stderr}"
    );
}

#[test]
fn mcp_reports_the_same_hits() {
    let response = handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_provided",
            "arguments": {
                "build_list": [root().join("tests/provided_fixtures/site.list")],
                "installed": [root().join("tests/provided_fixtures/installed.txt")],
                "baseline_lock": root().join("tests/provided_fixtures/baseline.lock.json"),
                "allow": ["GROMACS"]
            }
        }
    }))
    .expect("response");
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("name=FFTW"), "{text}");
    assert!(text.contains("name=Python"), "{text}");
    assert!(!text.contains("name=GROMACS"), "{text}");
    assert!(text.contains("sets=--hooks=/sw/hooks/site.py"), "{text}");
    assert!(text.contains("uses=foss-2025b"), "{text}");
}

#[test]
fn a_family_lock_names_the_lock_and_a_licensed_name_warns() {
    let dir = tempfile::tempdir().expect("temp");
    let list = dir.path().join("site.list");
    std::fs::write(
        &list,
        "\
SciPy-1.16.0-foss-2026.1.eb
OpenBLAS-0.3.32-lfoss-2026.1.eb
VASP-6.4.3-foss-2026.1.eb
code-server-4.99.0-GCCcore-15.2.0.eb
",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args(["stack", "provided", "--build-list"])
        .arg(&list)
        .arg("--baseline-lock")
        .arg(root().join("tests/provided_fixtures/baseline.lock.json"))
        .arg("--family-lock")
        .arg(root().join("tests/provided_fixtures/lfoss.lock.json"))
        .arg("--licensed-file")
        .arg(root().join("data/site-licensed.txt"))
        .output()
        .expect("stack provided");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("family SciPy-1.16.0-foss-2026.1.eb name=SciPy "),
        "{stdout}"
    );
    assert!(
        stdout.contains("tests/provided_fixtures/lfoss.lock.json"),
        "{stdout}"
    );
    assert!(
        stdout.contains("use that lock instead of the site layer"),
        "{stdout}"
    );
    assert!(stdout.contains("lfoss is the LLVM family"), "{stdout}");
    assert!(
        stdout.contains("licensed VASP-6.4.3-foss-2026.1.eb name=VASP "),
        "{stdout}"
    );
    assert!(!stdout.contains("code-server"), "{stdout}");
    assert!(stderr.contains("belong in a family lock"), "{stderr}");
    assert!(!stderr.contains("group-restricted"), "{stderr}");

    let strict = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .args(["stack", "provided", "--build-list"])
        .arg(&list)
        .arg("--baseline-lock")
        .arg(root().join("tests/provided_fixtures/baseline.lock.json"))
        .arg("--allow")
        .arg("SciPy")
        .arg("--allow")
        .arg("OpenBLAS")
        .arg("--licensed")
        .arg("VASP")
        .arg("--strict")
        .arg("--format")
        .arg("github")
        .output()
        .expect("strict");
    let stdout = String::from_utf8(strict.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&strict.stderr);
    assert_eq!(strict.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.contains("::warning "), "{stdout}");
    assert!(stdout.contains("title=licensed::"), "{stdout}");
    assert!(!stdout.contains("title=family::"), "{stdout}");
    assert!(stdout.contains("title=toolchain::"), "{stdout}");
    assert!(stderr.contains("group-restricted"), "{stderr}");
}
