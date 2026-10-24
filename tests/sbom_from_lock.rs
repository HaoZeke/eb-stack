//! `stack sbom --lock` reads the dependency edges back from the recipes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn easyconfigs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next/easyconfigs")
}

#[test]
fn an_sbom_written_from_a_lock_keeps_the_dependency_graph() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = tmp.path().join("stack.lock.json");
    let st = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(easyconfigs())
        .args([
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--lock-out",
        ])
        .arg(&lock)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );

    let out = tmp.path().join("stack.cdx.json");
    let st = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "sbom", "--lock"])
        .arg(&lock)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );

    let sbom: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    let deps = sbom["dependencies"].as_array().unwrap();
    let gromacs = deps
        .iter()
        .find(|d| {
            d["ref"]
                .as_str()
                .unwrap()
                .starts_with("pkg:generic/GROMACS@")
        })
        .expect("a GROMACS entry");
    let edges: Vec<&str> = gromacs["dependsOn"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        edges.iter().any(|r| r.starts_with("pkg:generic/OpenMPI@")),
        "GROMACS depends on OpenMPI in the lock: {edges:?}"
    );
}

#[test]
fn relative_easyconfig_paths_resolve_beside_the_lock_from_another_directory() {
    let root = tempfile::tempdir().unwrap();
    let recipes = root.path().join("recipes");
    std::fs::create_dir(&recipes).unwrap();
    std::fs::write(
        recipes.join("zlib.eb"),
        "name = 'zlib'\nversion = '1.3.1'\n\
         toolchain = {'name': 'system', 'version': ''}\n\
         sources = ['zlib-1.3.1.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n",
    )
    .unwrap();
    std::fs::write(
        recipes.join("app.eb"),
        "name = 'App'\nversion = '1.0'\n\
         toolchain = {'name': 'system', 'version': ''}\n\
         dependencies = [('zlib', '1.3.1')]\n",
    )
    .unwrap();
    let lock = root.path().join("stack.lock.json");
    std::fs::write(
        &lock,
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "toolchain": {"name": "system", "version": ""},
            "packages": [
                {"name": "zlib", "version": "1.3.1", "toolchain": {"name": "system", "version": ""}, "easyconfig_path": "recipes/zlib.eb"},
                {"name": "App", "version": "1.0", "toolchain": {"name": "system", "version": ""}, "easyconfig_path": "recipes/app.eb"}
            ],
            "solver": {"engine": "resolvo", "engine_version": "0", "timestamp": "2026-01-01T00:00:00Z"}
        }))
        .unwrap(),
    )
    .unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let out = elsewhere.path().join("stack.cdx.json");
    let cli = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(elsewhere.path())
        .args(["stack", "sbom", "--lock"])
        .arg(&lock)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        cli.status.success(),
        "{}",
        String::from_utf8_lossy(&cli.stderr)
    );
    let stdout = String::from_utf8(cli.stdout).unwrap();
    assert!(
        stdout.contains("recipes_read=2") && stdout.contains("recipes_unreadable=0"),
        "{stdout}"
    );
    let sbom: serde_json::Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    let app = sbom["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["ref"].as_str().unwrap_or("").contains("App@1.0"))
        .expect("App");
    assert!(
        app["dependsOn"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge.as_str().unwrap_or("").contains("zlib@1.3.1")),
        "{app}"
    );

    let mcp_out = elsewhere.path().join("mcp.cdx.json");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "eb_stack_sbom",
            "arguments": {"lock": lock, "out": mcp_out}
        }
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(elsewhere.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{request}\n").as_bytes())
        .unwrap();
    let mcp = child.wait_with_output().unwrap();
    assert!(
        mcp.status.success(),
        "{}",
        String::from_utf8_lossy(&mcp.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&mcp.stdout).unwrap();
    assert_eq!(response["result"]["isError"], false, "{response}");
    let structured = &response["result"]["structuredContent"];
    assert_eq!(structured["recipes_read"], 2);
    assert_eq!(structured["recipes_unreadable"], 0);
    assert_eq!(structured["with_artifact_facts"], 1);
    assert_eq!(structured["components"], 2);
}
