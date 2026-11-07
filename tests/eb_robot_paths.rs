//! `eb_easyblock_dry_run` keeps the robot paths EasyBuild is configured with.
//!
//! A robot directory the call adds goes first, then every path
//! `eb --show-config` reports for `robot-paths`, spelled out, because an
//! empty element in `--robot-paths` stands only for EasyBuild's built-in
//! default and drops `EASYBUILD_ROBOT_PATHS` and configuration files.

use eb_stack::mcp::handle_message;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write_executable(path: &Path, text: &str) {
    std::fs::write(path, text).expect("script");
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

fn dry_run(arguments: serde_json::Value) -> serde_json::Value {
    handle_message(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "eb_easyblock_dry_run", "arguments": arguments}
    }))
    .expect("tool response")
}

/// The tests set `EB_STACK_EB` and `EB_STACK_WORKSPACE` for the process.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn a_dry_run_searches_its_robot_paths_then_the_configured_ones() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("ws");
    std::fs::create_dir_all(workspace.join("robot")).expect("robot");
    std::fs::write(workspace.join("demo.eb"), "name = 'demo'\n").expect("recipe");
    std::fs::write(
        workspace.join("mod.py"),
        "class EB_demo(object):\n    pass\n",
    )
    .expect("module");
    let calls = temp.path().join("calls");
    let fake = temp.path().join("eb");
    write_executable(
        &fake,
        &format!(
            "#!/bin/sh\necho \"$*\" >> {calls}\nif [ \"$1\" = --show-config ]; then\n  echo 'buildpath         (D) = /p/build'\n  echo 'robot-paths       (E) = /site/one, /site/two'\n  exit 0\nfi\necho '== building and installing demo/1...'\nexit 0\n",
            calls = calls.display()
        ),
    );
    std::env::set_var("EB_STACK_EB", &fake);
    std::env::set_var("EB_STACK_WORKSPACE", &workspace);

    let with_robot =
        dry_run(json!({"recipe": "demo.eb", "easyblocks": ["mod.py"], "robot": ["robot"]}));
    let logged = std::fs::read_to_string(&calls).expect("calls");
    std::fs::remove_file(&calls).expect("reset");
    let without_robot = dry_run(json!({"recipe": "demo.eb", "easyblocks": ["mod.py"]}));
    let logged_without = std::fs::read_to_string(&calls).expect("calls");
    std::env::remove_var("EB_STACK_EB");
    std::env::remove_var("EB_STACK_WORKSPACE");

    assert_eq!(with_robot["result"]["isError"], false, "{with_robot}");
    let lines: Vec<&str> = logged.lines().collect();
    assert_eq!(lines.len(), 2, "{logged}");
    assert_eq!(lines[0], "--show-config");
    let robot = workspace.join("robot").canonicalize().expect("robot path");
    let expected = format!("--robot-paths={}:/site/one:/site/two", robot.display());
    assert!(
        lines[1].split(' ').any(|word| word == expected),
        "want {expected} in {}",
        lines[1]
    );

    // Without extra robot paths EasyBuild is not asked and keeps its own.
    assert_eq!(without_robot["result"]["isError"], false, "{without_robot}");
    let lines: Vec<&str> = logged_without.lines().collect();
    assert_eq!(lines.len(), 1, "{logged_without}");
    assert!(!lines[0].contains("--robot-paths"), "{logged_without}");
    assert!(!lines[0].contains("--show-config"), "{logged_without}");
}

/// A workspace with a recipe, an easyblock and a robot directory, and a fake
/// `eb` whose `--show-config` branch is `probe` and whose run is `run`.
fn workspace(probe: &str, run: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("ws");
    std::fs::create_dir_all(workspace.join("robot")).expect("robot");
    std::fs::write(workspace.join("demo.eb"), "name = 'demo'\n").expect("recipe");
    std::fs::write(
        workspace.join("mod.py"),
        "class EB_demo(object):\n    pass\n",
    )
    .expect("module");
    let fake = temp.path().join("eb");
    write_executable(
        &fake,
        &format!(
            "#!/bin/sh\necho \"$*\" >> {calls}\nif [ \"$1\" = --show-config ]; then\n{probe}\nfi\n{run}\n",
            calls = temp.path().join("calls").display()
        ),
    );
    std::env::set_var("EB_STACK_EB", &fake);
    std::env::set_var("EB_STACK_WORKSPACE", &workspace);
    (temp, workspace)
}

fn reset() {
    std::env::remove_var("EB_STACK_EB");
    std::env::remove_var("EB_STACK_WORKSPACE");
}

fn child_gone(path: &Path) -> bool {
    let pid = std::fs::read_to_string(path).expect("child pid");
    let stat = format!("/proc/{}/stat", pid.trim());
    for _ in 0..60 {
        match std::fs::read_to_string(&stat) {
            Ok(text)
                if !text
                    .rsplit(')')
                    .next()
                    .unwrap_or("")
                    .trim_start()
                    .starts_with('Z') =>
            {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => return true,
        }
    }
    false
}

#[test]
fn timeout_s_stops_a_hanging_show_config_probe_and_its_child() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (temp, _) = workspace(
        "  sleep 300 &\n  echo $! > \"$(dirname \"$0\")/child.pid\"\n  wait\n  exit 0",
        "exit 0",
    );
    let started = std::time::Instant::now();
    let response = dry_run(
        json!({"recipe": "demo.eb", "easyblocks": ["mod.py"], "robot": ["robot"], "timeout_s": 10}),
    );
    let took = started.elapsed();
    reset();
    assert_eq!(response["result"]["isError"], true, "{response}");
    let text = response.to_string();
    assert!(
        text.contains("--show-config did not finish within the 10 s timeout"),
        "{text}"
    );
    assert!(took < std::time::Duration::from_secs(14), "took {took:?}");
    assert!(
        child_gone(&temp.path().join("child.pid")),
        "probe child still runs"
    );
    let calls = std::fs::read_to_string(temp.path().join("calls")).expect("calls");
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        ["--show-config"],
        "the run did not start"
    );
}

#[test]
fn a_show_config_probe_that_floods_its_output_is_stopped() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (temp, workspace) = workspace("  yes robot-paths\n  exit 0", "exit 0");
    let started = std::time::Instant::now();
    let response = dry_run(
        json!({"recipe": "demo.eb", "easyblocks": ["mod.py"], "robot": ["robot"], "timeout_s": 60}),
    );
    let took = started.elapsed();
    reset();
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert!(
        response
            .to_string()
            .contains("--show-config wrote more than"),
        "{response}"
    );
    assert!(took < std::time::Duration::from_secs(20), "took {took:?}");
    let probe_output = std::fs::read_dir(workspace.join(".eb-stack/dry-run"))
        .expect("runs")
        .flatten()
        .map(|entry| entry.path().join("show-config.out"))
        .find(|path| path.is_file())
        .expect("probe output");
    let size = std::fs::metadata(&probe_output).expect("size").len();
    assert!(
        size <= eb_stack::eb_run::SHOW_CONFIG_OUTPUT_LIMIT,
        "{size} bytes"
    );
    drop(temp);
}

#[test]
fn the_dry_run_gets_what_the_probe_left_of_timeout_s() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (temp, _) = workspace(
        "  sleep 6\n  echo 'robot-paths       (E) = /site/one'\n  exit 0",
        "sleep 60",
    );
    let started = std::time::Instant::now();
    let response = dry_run(
        json!({"recipe": "demo.eb", "easyblocks": ["mod.py"], "robot": ["robot"], "timeout_s": 10}),
    );
    let took = started.elapsed();
    reset();
    assert_eq!(response["result"]["isError"], false, "{response}");
    let body = &response["result"]["structuredContent"];
    assert_eq!(body["timed_out"], true, "{body}");
    assert!(
        took < std::time::Duration::from_secs(13),
        "probe and dry run took {took:?} under timeout_s 10"
    );
    let calls = std::fs::read_to_string(temp.path().join("calls")).expect("calls");
    assert_eq!(calls.lines().count(), 2, "{calls}");
}
