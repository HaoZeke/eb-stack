//! What a person sees when a command fails, and what `--help` promises.

use std::path::Path;
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

fn assert_status(args: &[&str], code: i32, needle: &str) {
    let result = output(args);
    let stderr = String::from_utf8(result.stderr).expect("utf8");
    assert_eq!(result.status.code(), Some(code), "args {args:?}\n{stderr}");
    assert!(
        stderr.contains(needle),
        "args {args:?} missing {needle:?}\n{stderr}"
    );
    assert!(
        !stderr.contains("Stack backtrace") && !stderr.contains("panicked at"),
        "{stderr}"
    );
}

#[test]
fn a_rejected_command_line_exits_2_and_a_failed_check_exits_1() {
    assert_status(&["easyblock-reach"], 2, "pass --file, --range, or --pr");
    assert_status(
        &[
            "package",
            "inspect",
            "--source",
            "recipe.yaml",
            "--format",
            "nope",
            "--toolchain-version",
            "2026.1",
            "--out-dir",
            "out",
        ],
        2,
        "--format nope:",
    );
    assert_status(
        &[
            "stack",
            "solve",
            "--easyconfigs",
            "tree",
            "--toolchain",
            "foss",
            "--root",
            "zlib",
        ],
        2,
        "--toolchain wants name/version",
    );
    assert_status(
        &["recipe", "templates", "--name", "Foo"],
        2,
        "--version, or --export",
    );
    assert_status(
        &["recipe", "format", "--out", "one.eb", "a.eb", "b.eb"],
        2,
        "--out requires exactly one recipe path",
    );
    assert_status(
        &[
            "package",
            "bump",
            "--source",
            "recipe.eb",
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2026.1",
            "--easyconfigs",
            "tree",
            "--dep",
            "notaversion",
            "--out-dir",
            "out",
        ],
        2,
        "--dep expects NAME=VERSION, got \"notaversion\"",
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let tree = temp.path().join("tree");
    std::fs::create_dir(&tree).expect("tree");
    let installed = temp.path().join("installed.txt");
    std::fs::write(&installed, "zlib/1.2\n").expect("installed");
    let pr = temp.path().join("pr.json");
    std::fs::write(
        &pr,
        r#"{"number":1,"head_sha":"abc","title":"t","easystacks":[]}"#,
    )
    .expect("pr");
    let stack = temp.path().join("stack.yml");
    std::fs::write(&stack, "easyconfigs: []\n").expect("stack");
    assert_status(
        &[
            "pr-overlap",
            "--pr",
            pr.to_str().unwrap(),
            "--easyconfigs",
            tree.to_str().unwrap(),
            "--installed",
            installed.to_str().unwrap(),
            "--filter-deps",
            "zlib=1=2",
            "--out-dir",
            temp.path().join("overlap").to_str().unwrap(),
        ],
        2,
        "--filter-deps:",
    );
    assert_status(
        &[
            "pr-overlap",
            "--pr",
            pr.to_str().unwrap(),
            "--easyconfigs",
            tree.to_str().unwrap(),
            "--installed",
            installed.to_str().unwrap(),
            "--commit-tree",
            "nocolon",
            "--out-dir",
            temp.path().join("overlap2").to_str().unwrap(),
        ],
        2,
        "--commit-tree wants <commit>=<dir>, got nocolon",
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/rebuild_set_fixtures");
    assert_status(
        &[
            "rebuild-set",
            "--changed",
            "zlib-1.3.1-GCCcore-15.2.0.eb",
            "--easystack",
            root.join("eessi-2026.06-eb-5.4.0-2026.1.yml")
                .to_str()
                .unwrap(),
            "--easyconfigs",
            root.join("release").to_str().unwrap(),
            "--installed",
            root.join("installed.txt").to_str().unwrap(),
            "--check-static",
            "--out-dir",
            temp.path().join("rebuild").to_str().unwrap(),
        ],
        2,
        "--check-static needs --prefix",
    );

    let bad_lock = temp.path().join("bad.lock.json");
    std::fs::write(&bad_lock, "{not json").expect("lock");
    assert_status(
        &[
            "stack",
            "sbom",
            "--lock",
            bad_lock.to_str().unwrap(),
            "--out",
            temp.path().join("sbom.json").to_str().unwrap(),
        ],
        1,
        bad_lock.to_str().unwrap(),
    );
    let bad_eb = temp.path().join("bad.eb");
    std::fs::write(&bad_eb, "name = 'Z'\nversion = \n").expect("eb");
    assert_status(
        &["recipe", "lint", bad_eb.to_str().unwrap()],
        1,
        "style errors",
    );
}
