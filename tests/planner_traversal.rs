//! `stack shard` over robot and module trees that symlinks loop back into or
//! alias: the plan finishes, with the closures of the plain tree.

#![cfg(unix)]

use serde_json::Value;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);

fn recipe(dir: &Path, name: &str, dependency: Option<&str>) {
    std::fs::create_dir_all(dir).unwrap();
    let deps = dependency
        .map(|d| format!("dependencies = [('{d}', '1.0')]\n"))
        .unwrap_or_default();
    std::fs::write(
        dir.join(format!("{name}-1.0.eb")),
        format!(
            "easyblock = 'Tarball'\nname = '{name}'\nversion = '1.0'\n\
             homepage = 'https://example.org'\ndescription = '{name}'\n\
             toolchain = SYSTEM\nsources = []\n{deps}moduleclass = 'tools'\n"
        ),
    )
    .unwrap();
}

fn five_roots(root: &Path) {
    for name in ["A", "B", "C", "D", "E"] {
        recipe(&root.join(name.to_lowercase()).join(name), name, None);
    }
}

/// Run `stack shard` on the five roots; fail when it outlives the deadline.
fn shard(dir: &Path, easyconfigs: &Path, extra: &[&Path]) -> Value {
    let list = dir.join("five.list");
    std::fs::write(&list, "A-1.0.eb\nB-1.0.eb\nC-1.0.eb\nD-1.0.eb\nE-1.0.eb\n").unwrap();
    let out = dir.join("out");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    cmd.args(["stack", "shard", "--build-list"])
        .arg(&list)
        .arg("--easyconfigs")
        .arg(easyconfigs)
        .args([
            "--shards",
            "1",
            "--generated-at",
            "2026-11-08T00:00:00Z",
            "--out-dir",
        ])
        .arg(&out)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for path in extra {
        cmd.arg("--installed-modules-dir").arg(path);
    }
    let mut child = cmd.spawn().expect("run eb-stack");
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
            assert!(status.success(), "{stderr}");
            break;
        }
        if start.elapsed() > DEADLINE {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("stack shard did not finish within {DEADLINE:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    serde_json::from_slice(&std::fs::read(out.join("plan.json")).unwrap()).unwrap()
}

fn closure(plan: &Value, easyconfig: &str) -> Vec<String> {
    plan["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["easyconfig"] == easyconfig)
        .unwrap()["closure"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn symlinks_back_into_the_robot_root_do_not_stop_the_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("robot");
    five_roots(&root);
    symlink(".", root.join("loop1")).unwrap();
    symlink(".", root.join("loop2")).unwrap();
    symlink("../..", root.join("a/A/up")).unwrap();
    let plan = shard(tmp.path(), &root, &[]);
    for name in ["A", "B", "C", "D", "E"] {
        assert_eq!(
            closure(&plan, &format!("{name}-1.0.eb")),
            [format!("{name}/1.0")]
        );
    }
}

#[test]
fn nested_symlink_aliases_read_each_directory_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("robot");
    five_roots(&root);
    // 2^24 paths reach the last level through x and y links, without a cycle.
    for level in 0..24 {
        let dir = root.join(format!("L{level}"));
        std::fs::create_dir_all(&dir).unwrap();
        symlink(format!("../L{}", level + 1), dir.join("x")).unwrap();
        symlink(format!("../L{}", level + 1), dir.join("y")).unwrap();
    }
    recipe(&root.join("L24"), "G", None);
    let plan = shard(tmp.path(), &root, &[]);
    assert_eq!(closure(&plan, "A-1.0.eb"), ["A/1.0"]);
}

#[test]
fn symlinks_back_into_a_modules_tree_do_not_stop_the_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("robot");
    five_roots(&root);
    let modules = tmp.path().join("modules");
    for name in ["A", "B", "C", "D", "E"] {
        std::fs::create_dir_all(modules.join(name)).unwrap();
        std::fs::write(modules.join(name).join("1.0.lua"), "whatis('x')\n").unwrap();
    }
    symlink("..", modules.join("A/up1")).unwrap();
    symlink("..", modules.join("A/up2")).unwrap();
    let plan = shard(tmp.path(), &root, &[&modules]);
    for name in ["A", "B", "C", "D", "E"] {
        assert!(
            closure(&plan, &format!("{name}-1.0.eb")).is_empty(),
            "{name} is installed"
        );
    }
}
