//! CUDA compute-capability checks for PR easystacks.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/cuda_cc_fixtures")
        .join(name)
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

#[test]
fn failing_pr_1461_shape_is_flagged() {
    let output = run(&[
        "stack",
        "cuda-cc",
        "--easystack",
        fixture("easystacks/pr-1461-failing.yml").to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("CUDA 12.6 drops compute capabilities 12.0f, 9.0a")
            || stdout.contains("CUDA 12.6 drops compute capabilities 9.0a, 12.0f"),
        "{stdout}"
    );
    assert!(
        stdout.contains("missing target capabilities 9.0 for zen4-cc90"),
        "{stdout}"
    );
    assert!(
        stdout.contains("missing target capabilities 9.0 for grace-cc90"),
        "{stdout}"
    );
    assert!(
        stdout.contains("below grace-cc90 minimum CUDA 13.1"),
        "{stdout}"
    );
    assert!(
        stdout.contains("easyblock map rejects compute capabilities"),
        "{stdout}"
    );
    assert!(stdout.contains("findings=5"), "{stdout}");
}

#[test]
fn fixed_shape_passes() {
    let output = run(&[
        "stack",
        "cuda-cc",
        "--easystack",
        fixture("easystacks/pr-1461-fixed.yml").to_str().unwrap(),
        "--easyconfigs",
        fixture("easyconfigs").to_str().unwrap(),
    ]);
    let stdout = text(&output.stdout);
    let stderr = text(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("checked=1 findings=0"), "{stdout}");
}
