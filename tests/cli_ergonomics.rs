//! What a person sees when a command fails, and what `--help` promises.

use std::path::Path;
use std::process::Command;

#[test]
fn toolchain_and_stack_policy_use_one_name_across_commands() {
    let help = help_text(&["stack".into(), "solve".into()]);
    assert!(
        help.contains("--stack-policy"),
        "stack solve help should show the shared policy flag:\n{help}"
    );
    assert!(
        !help.contains("--policy "),
        "the old spelling stays an alias and is not a second flag:\n{help}"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let tree = temp.path().join("tree");
    std::fs::create_dir(&tree).expect("tree");
    let missing = temp.path().join("no-policy.toml");
    for flag in ["--stack-policy", "--policy"] {
        assert_status(
            &[
                "stack",
                "solve",
                "--easyconfigs",
                tree.to_str().unwrap(),
                flag,
                missing.to_str().unwrap(),
            ],
            1,
            missing.to_str().unwrap(),
        );
    }

    let source = temp.path().join("recipe.yaml");
    std::fs::write(
        &source,
        "package:\n  name: eon\n  version: 1.0.0\nsource:\n  url: https://example.invalid/eon.tar.gz\n  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .expect("recipe");
    let out_dir = temp.path().join("bundle");
    let ok = output(&[
        "package",
        "inspect",
        "--source",
        source.to_str().unwrap(),
        "--format",
        "conda-forge",
        "--toolchain",
        "foss/2026.1",
        "--out-dir",
        out_dir.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8(ok.stderr).expect("utf8");
    assert!(ok.status.success(), "{stderr}");
    let plan = std::fs::read_to_string(out_dir.join("package.plan.json")).expect("plan");
    assert!(plan.contains("2026.1"), "{plan}");
    assert!(plan.contains("foss"), "{plan}");

    assert_status(
        &[
            "package",
            "inspect",
            "--source",
            source.to_str().unwrap(),
            "--format",
            "conda-forge",
            "--toolchain",
            "foss",
            "--out-dir",
            temp.path().join("bad").to_str().unwrap(),
        ],
        2,
        "--toolchain foss:",
    );
    let both = output(&[
        "package",
        "inspect",
        "--source",
        source.to_str().unwrap(),
        "--toolchain",
        "foss/2026.1",
        "--toolchain-version",
        "2025a",
        "--out-dir",
        temp.path().join("both").to_str().unwrap(),
    ]);
    assert_eq!(both.status.code(), Some(2));
    let stderr = String::from_utf8(both.stderr).expect("utf8");
    assert!(
        stderr.contains("--toolchain") && stderr.contains("cannot be used with"),
        "{stderr}"
    );
}

fn bin() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command.env("RUST_BACKTRACE", "1");
    command
}

fn output(args: &[&str]) -> std::process::Output {
    bin().args(args).output().expect("run eb-stack")
}

fn help_text(args: &[String]) -> String {
    let mut full: Vec<&str> = args.iter().map(String::as_str).collect();
    full.push("--help");
    let result = output(&full);
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).expect("utf8")
}

/// Subcommands a person runs. `help` is not one of them.
fn leaves() -> Vec<Vec<String>> {
    let mut found = Vec::new();
    let mut queue = vec![Vec::new()];
    while let Some(prefix) = queue.pop() {
        let text = help_text(&prefix);
        let mut commands = Vec::new();
        let mut in_commands = false;
        for line in text.lines() {
            if line.trim() == "Commands:" {
                in_commands = true;
                continue;
            }
            if !in_commands {
                continue;
            }
            if line.trim().is_empty() {
                if !commands.is_empty() {
                    break;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("  ") {
                if rest.starts_with(' ') || rest.starts_with('-') {
                    continue;
                }
                if let Some(name) = rest.split_whitespace().next() {
                    if name != "help" {
                        commands.push(name.to_string());
                    }
                }
            } else if !commands.is_empty() {
                break;
            }
        }
        if commands.is_empty() {
            found.push(prefix);
        } else {
            for name in commands {
                let mut next = prefix.clone();
                next.push(name);
                queue.push(next);
            }
        }
    }
    found
}

fn flag_is_described(lines: &[&str], index: usize) -> bool {
    let line = lines[index];
    let trimmed = line.trim();
    if trimmed.starts_with("-h,") || trimmed == "--help" {
        return true;
    }
    // A one-line form puts the description after the flag and its value.
    let after_flag = trimmed
        .split_once(' ')
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    let after_value = after_flag
        .strip_prefix('<')
        .and_then(|rest| rest.split_once('>').map(|(_, tail)| tail.trim()))
        .or_else(|| {
            after_flag
                .strip_prefix('[')
                .and_then(|rest| rest.split_once(']').map(|(_, tail)| tail.trim()))
        })
        .unwrap_or(after_flag);
    if !after_value.is_empty() && !after_value.starts_with('[') && !after_value.starts_with('<') {
        return true;
    }
    let mut next = index + 1;
    while next < lines.len() && lines[next].trim().is_empty() {
        next += 1;
    }
    let Some(following) = lines.get(next) else {
        return false;
    };
    let following_trim = following.trim();
    if following_trim.starts_with("[default:") && following_trim.ends_with(']') {
        return false;
    }
    following.starts_with("          ")
        && !following_trim.is_empty()
        && !following_trim.starts_with("--")
        && !following_trim.starts_with("-h,")
}

#[test]
fn every_command_help_has_an_example_and_describes_every_flag() {
    let commands = leaves();
    assert!(
        commands.len() >= 20,
        "walked too few commands: {commands:?}"
    );
    for command in commands {
        let text = help_text(&command);
        assert!(
            text.contains("Examples:"),
            "{} --help has no example:\n{text}",
            command.join(" ")
        );
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            let is_flag = trimmed.starts_with("--") || trimmed.starts_with("-h,");
            if !is_flag {
                continue;
            }
            assert!(
                flag_is_described(&lines, index),
                "{} flag has no description:\n{line}\n{text}",
                command.join(" ")
            );
        }
    }
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
        stderr.contains("--source") && stderr.contains("/no/such/recipe.yaml"),
        "a missing file names the flag and the path: {stderr}"
    );
    assert!(
        !stderr.contains("Stack backtrace") && !stderr.contains("panicked at"),
        "a missing file is not a crash: {stderr}"
    );
    assert!(missing.stdout.is_empty(), "failures stay on stderr");

    let temp = tempfile::tempdir().expect("tempdir");
    let tree = temp.path().join("tree");
    std::fs::create_dir(&tree).expect("tree");
    let tree = tree.to_str().unwrap();
    let policy = temp.path().join("nope.json");
    let policy = policy.to_str().unwrap();
    assert_names_flag(
        &[
            "stack",
            "solve",
            "--easyconfigs",
            tree,
            "--stack-policy",
            policy,
        ],
        "--stack-policy",
        policy,
    );
    let lock = temp.path().join("nope.lock.json");
    let lock = lock.to_str().unwrap();
    assert_names_flag(&["stack", "sbom", "--lock", lock], "--lock", lock);
    assert_names_flag(&["stack", "easystack", "--lock", lock], "--lock", lock);
    let source = temp.path().join("nope.eb");
    let source = source.to_str().unwrap();
    let out = temp.path().join("out");
    let out = out.to_str().unwrap();
    assert_names_flag(
        &[
            "package",
            "bump",
            "--source",
            source,
            "--toolchain",
            "foss/2026.1",
            "--easyconfigs",
            tree,
            "--out-dir",
            out,
        ],
        "--source",
        source,
    );
    let recipe = temp.path().join("missing.eb");
    let recipe = recipe.to_str().unwrap();
    assert_names_flag(&["recipe", "lint", recipe], "recipe", recipe);
}

#[test]
fn a_missing_lock_names_the_path_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let lock = temp.path().join("nope.lock.json");
    let lock = lock.to_str().unwrap();
    let sbom_out = temp.path().join("out.cdx.json");
    let easy_out = temp.path().join("out.yml");
    for args in [
        vec![
            "stack",
            "sbom",
            "--lock",
            lock,
            "--out",
            sbom_out.to_str().unwrap(),
        ],
        vec![
            "stack",
            "easystack",
            "--lock",
            lock,
            "--out",
            easy_out.to_str().unwrap(),
        ],
    ] {
        let result = output(&args);
        let stderr = String::from_utf8(result.stderr).expect("utf8");
        assert_eq!(result.status.code(), Some(1), "{args:?}\n{stderr}");
        assert!(stderr.contains("--lock"), "{stderr}");
        assert_eq!(
            stderr.matches(lock).count(),
            1,
            "the lock path is named once: {stderr}"
        );
        assert!(!stderr.contains("read "), "{stderr}");
    }
    assert!(!sbom_out.exists());
    assert!(!easy_out.exists());
}

#[test]
fn an_empty_toolchain_name_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("recipe.yaml");
    std::fs::write(
        &source,
        "package:\n  name: eon\n  version: 1.0.0\nsource:\n  url: https://example.invalid/eon.tar.gz\n  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .expect("recipe");
    let out = temp.path().join("out");
    let result = output(&[
        "package",
        "inspect",
        "--source",
        source.to_str().unwrap(),
        "--format",
        "conda-forge",
        "--toolchain-name",
        "",
        "--toolchain-version",
        "2026.1",
        "--out-dir",
        out.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8(result.stderr).expect("utf8");
    assert_eq!(result.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("--toolchain-name must not be empty"),
        "{stderr}"
    );
    assert!(result.stdout.is_empty(), "{stderr}");
    assert!(!out.exists(), "left {}", out.display());
}

fn assert_names_flag(args: &[&str], flag: &str, path: &str) {
    let result = output(args);
    let stderr = String::from_utf8(result.stderr).expect("utf8");
    assert_eq!(result.status.code(), Some(1), "args {args:?}\n{stderr}");
    assert!(
        stderr.contains(flag) && stderr.contains(path),
        "a missing file names {flag} and {path}: {stderr}"
    );
    assert!(
        !stderr.contains("Stack backtrace") && !stderr.contains("panicked at"),
        "{stderr}"
    );
    assert!(
        result.stdout.is_empty(),
        "failures stay on stderr: {stderr}"
    );
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
        "--toolchain foss: expected name/version",
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

#[test]
fn a_bad_input_names_the_flag_the_path_and_the_value() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tree = temp.path().join("tree");
    std::fs::create_dir(&tree).expect("tree");
    let installed = temp.path().join("installed.txt");
    std::fs::write(&installed, "zlib/1.2\n").expect("installed");
    let pr = temp.path().join("shape.json");
    std::fs::write(&pr, r#"{"number":"x"}"#).expect("pr");
    assert_status(
        &[
            "pr-overlap",
            "--pr",
            pr.to_str().unwrap(),
            "--easyconfigs",
            tree.to_str().unwrap(),
            "--installed",
            installed.to_str().unwrap(),
            "--out-dir",
            temp.path().join("out").to_str().unwrap(),
        ],
        1,
        pr.to_str().unwrap(),
    );
    let stderr = String::from_utf8(
        output(&[
            "pr-overlap",
            "--pr",
            pr.to_str().unwrap(),
            "--easyconfigs",
            tree.to_str().unwrap(),
            "--installed",
            installed.to_str().unwrap(),
        ])
        .stderr,
    )
    .expect("utf8");
    assert!(
        stderr.contains("--pr") && stderr.contains('x'),
        "the flag and the bad value are both named: {stderr}"
    );

    assert_status(
        &[
            "component-reach",
            "--component",
            "curl",
            "--version",
            "not a req",
            "--generation",
            "2026.1",
            "--easyconfigs",
            tree.to_str().unwrap(),
        ],
        2,
        "--version not a req:",
    );
    assert_status(
        &[
            "component-reach",
            "--component",
            "curl",
            "--version",
            "*",
            "--generation",
            "2026.1",
            "--easyconfigs",
            tree.to_str().unwrap(),
            "--generated-at",
            "yesterday",
        ],
        2,
        "--generated-at yesterday:",
    );
    assert_status(
        &[
            "rebuild-set",
            "--changed",
            "/no/such/changed-list.txt",
            "--easystack",
            temp.path().join("missing.yml").to_str().unwrap(),
            "--easyconfigs",
            tree.to_str().unwrap(),
            "--installed",
            installed.to_str().unwrap(),
        ],
        1,
        "--changed /no/such/changed-list.txt:",
    );

    let config = temp.path().join("target.toml");
    std::fs::write(
        &config,
        "schema_version = 1\n[[targets]]\nname = \"local\"\n",
    )
    .expect("config");
    assert_status(
        &[
            "target",
            "doctor",
            "--config",
            config.to_str().unwrap(),
            "--target",
            "local",
        ],
        1,
        config.to_str().unwrap(),
    );

    let out_dir = temp.path().join("is-a-directory");
    std::fs::create_dir(&out_dir).expect("dir");
    let lock = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/gromacs_2025_to_next/baseline.lock.json");
    assert_status(
        &[
            "stack",
            "sbom",
            "--lock",
            lock.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
        ],
        1,
        out_dir.to_str().unwrap(),
    );
}
