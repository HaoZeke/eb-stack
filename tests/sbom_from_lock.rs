//! `stack sbom --lock` reads the dependency edges back from the recipes.

use std::path::PathBuf;
use std::process::Command;

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
