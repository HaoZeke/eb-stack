//! Explicit module destinations must not borrow a root from another tree.
use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn fixture(root: &Path) {
    fs::create_dir_all(root.join("robot")).unwrap();
    for name in ["Foo", "Need", "Reuse"] {
        fs::write(
            root.join("robot").join(format!("{name}-1.eb")),
            format!(
                "name = '{name}'\nversion = '1'\ntoolchain = SYSTEM\n{}",
                if name == "Foo" {
                    "dependencies = [('Need', '1'), ('Reuse', '1')]\n"
                } else {
                    ""
                }
            ),
        )
        .unwrap();
    }
    for name in ["Foo", "Reuse"] {
        fs::create_dir_all(root.join("cpu").join(name)).unwrap();
        fs::write(root.join("cpu").join(name).join("1.lua"), "-- module\n").unwrap();
    }
    fs::create_dir_all(root.join("noarch")).unwrap();
}

fn run(root: &Path, row: &str, label: &str, lifted: bool) -> std::process::Output {
    fs::write(root.join("rows"), format!("{row}\n")).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    cmd.env_remove("EASYBUILD_FILTER_DEPS")
        .args(["stack", "shard", "--build-list"])
        .arg(root.join("rows"))
        .arg("--easyconfigs")
        .arg(root.join("robot"))
        .arg("--installed-modules-dir")
        .arg(root.join("cpu"))
        .args(["--shards", "1", "--materialize", "--build-robot-path"])
        .arg(root.join("robot"))
        .arg("--out-dir")
        .arg(root.join(label));
    if lifted {
        cmd.arg("--lift-cpu-prerequisites");
    }
    cmd.output().unwrap()
}

fn plan(root: &Path, row: &str, label: &str, lifted: bool) -> Value {
    let output = run(root, row, label, lifted);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&fs::read(root.join(label).join("plan.json")).unwrap()).unwrap()
}

#[test]
fn explicit_destination_controls_root_closure_cost_and_row_retention() {
    for separator in ["=", " "] {
        for lifted in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            fixture(root);
            let row = format!(
                "Foo-1.eb --installpath-modules{separator}{}",
                root.join("noarch").display()
            );
            let missing = plan(root, &row, "missing", lifted);
            assert_eq!(missing["lines"][0]["text"], row);
            let closure = missing["lines"][0]["closure"].as_array().unwrap();
            assert!(closure.contains(&Value::from("Foo/1")));
            assert!(closure.contains(&Value::from("Need/1")));
            assert!(!closure.contains(&Value::from("Reuse/1")));
            assert_eq!(closure.len(), 2);
            let filename = if lifted {
                "cpu-prerequisites.list"
            } else {
                "shard-01.list"
            };
            assert!(fs::read_to_string(root.join("missing").join(filename))
                .unwrap()
                .lines()
                .any(|l| l == row));
            let local_filename = filename.replace(".list", ".local.list");
            assert!(
                fs::read_to_string(root.join("missing").join(local_filename))
                    .unwrap()
                    .contains("--installpath-modules")
            );
            if lifted {
                assert_eq!(missing["cpu_prerequisites"]["weight"], 2);
                assert!(
                    missing["cpu_prerequisites"]["inode_footprint"]
                        .as_u64()
                        .unwrap()
                        > 2000
                );
            }
            if !lifted {
                assert_eq!(missing["shards"][0]["weight"], 2);
                assert!(missing["shards"][0]["inode_footprint"].as_u64().unwrap() > 1000);
            }
            fs::create_dir_all(root.join("noarch/all/Foo")).unwrap();
            fs::write(root.join("noarch/all/Foo/1.lua"), "-- module\n").unwrap();
            let present = plan(root, &row, "present", lifted);
            assert_eq!(present["lines"][0]["closure"], serde_json::json!([]));
            assert_ne!(missing["inputs"]["sha256"], present["inputs"]["sha256"]);
            assert_eq!(present["lines"][0]["text"], row);
            if lifted {
                assert_eq!(present["cpu_prerequisites"]["weight"], 0);
                assert_eq!(present["cpu_prerequisites"]["inode_footprint"], 1000);
            }
        }
    }
}

#[test]
fn default_roots_reuse_global_modules_but_explicit_roots_do_not_require_global_presence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture(root);
    let default = plan(root, "Foo-1.eb", "default", false);
    assert_eq!(default["lines"][0]["closure"], serde_json::json!([]));
    fs::rename(root.join("cpu/Foo"), root.join("unrelated")).unwrap();
    fs::create_dir_all(root.join("noarch/all/Foo")).unwrap();
    fs::write(root.join("noarch/all/Foo/1"), "# Tcl module\n").unwrap();
    let row = format!(
        "Foo-1.eb --installpath-modules={}",
        root.join("noarch").display()
    );
    let explicit = plan(root, &row, "explicit", false);
    assert_eq!(explicit["lines"][0]["closure"], serde_json::json!([]));
}

#[test]
fn malformed_or_unresolved_explicit_destinations_cannot_reuse_global_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fixture(root);
    for option in [
        "--installpath-modules",
        "--installpath-modules=",
        "--installpath-modules relative/path",
        "--installpath-modules=$UNRESOLVED/modules",
        "--installpath-modules=/root/$UNRESOLVED",
        "--installpath-modules=/one --installpath-modules=/two",
    ] {
        let output = run(root, &format!("Foo-1.eb {option}"), "invalid", false);
        assert!(!output.status.success(), "{option}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("literal absolute path"));
    }
}
