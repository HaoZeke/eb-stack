//! What a person sees when a command fails, and what `--help` promises.

use std::process::Command;

fn bin() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command.env("RUST_BACKTRACE", "1");
    command
}

fn output(args: &[&str]) -> std::process::Output {
    bin().args(args).output().expect("run eb-stack")
}

#[test]
fn a_failure_names_the_problem_and_prints_no_backtrace() {
    let missing = output(&[
        "package",
        "inspect",
        "--source",
        "/no/such/recipe.yaml",
        "--toolchain-version",
        "2026.1",
        "--out-dir",
        "/tmp/eb-stack-cli-ergonomics-out",
    ]);
    assert_eq!(missing.status.code(), Some(1));
    let stderr = String::from_utf8(missing.stderr).expect("utf8");
    assert!(
        stderr.starts_with("error: "),
        "failures use the same prefix as clap: {stderr}"
    );
    assert!(
        stderr.contains("/no/such/recipe.yaml"),
        "the missing path is part of the message: {stderr}"
    );
    assert!(
        !stderr.contains("Stack backtrace") && !stderr.contains("panicked at"),
        "a missing file is not a crash: {stderr}"
    );
    assert!(missing.stdout.is_empty(), "failures stay on stderr");
}
