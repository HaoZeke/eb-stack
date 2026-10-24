//! `stack sbom --lock` reads the dependency edges back from the recipes.
//! `stack solve --sbom-out` on the recorded Cbc slice keeps one dependency
//! entry per component, and the checksum that component's own recipe states.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
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

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

#[test]
fn stack_solve_sbom_out_matches_stack_sbom_for_the_same_lock() {
    let fftw_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("easyconfigs");
    copy_dir(&easyconfigs(), &tree);
    let fftw = tree.join("foss-2025b/FFTW-3.3.10-foss-2025b.eb");
    let mut recipe = std::fs::read_to_string(&fftw).unwrap();
    recipe.push_str(&format!(
        "\nsources = ['fftw-3.3.10.tar.gz']\nchecksums = ['{fftw_hash}']\n"
    ));
    std::fs::write(&fftw, recipe).unwrap();

    let lock = root.path().join("stack.lock.json");
    let from_solve = root.path().join("solve.cdx.json");
    let solved = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(&tree)
        .args([
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--lock-out",
        ])
        .arg(&lock)
        .args(["--sbom-out"])
        .arg(&from_solve)
        .output()
        .unwrap();
    assert!(
        solved.status.success(),
        "{}",
        String::from_utf8_lossy(&solved.stderr)
    );

    let from_sbom = root.path().join("sbom.cdx.json");
    let emitted = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "sbom", "--lock"])
        .arg(&lock)
        .arg("--out")
        .arg(&from_sbom)
        .output()
        .unwrap();
    assert!(
        emitted.status.success(),
        "{}",
        String::from_utf8_lossy(&emitted.stderr)
    );

    let mut solve_doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&from_solve).unwrap()).unwrap();
    let mut sbom_doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&from_sbom).unwrap()).unwrap();
    solve_doc.as_object_mut().unwrap().remove("serialNumber");
    sbom_doc.as_object_mut().unwrap().remove("serialNumber");
    assert_eq!(solve_doc, sbom_doc);
    let encoded = serde_json::to_string(&solve_doc).unwrap();
    assert!(
        encoded.contains(fftw_hash),
        "both documents carry the FFTW checksum"
    );
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn eb_stack(args: &[&str], dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(repo())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(args)
        .env("TMPDIR", dir)
        .output()
        .expect("run eb-stack")
}

/// The closure page's `stack solve` command, written under a temp directory.
fn page_solve(easyconfigs: &str) -> (tempfile::TempDir, serde_json::Value, serde_json::Value) {
    let tmp = tempfile::tempdir().expect("temp");
    let lock = tmp.path().join("stack.lock.json");
    let sbom = tmp.path().join("stack.cdx.json");
    let out = eb_stack(
        &[
            "stack",
            "solve",
            "--easyconfigs",
            easyconfigs,
            "--toolchain",
            "foss/2025b",
            "--root",
            "Cbc",
            "--lock-out",
            lock.to_str().unwrap(),
            "--sbom-out",
            sbom.to_str().unwrap(),
        ],
        tmp.path(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lock).unwrap()).unwrap();
    let sbom: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sbom).unwrap()).unwrap();
    (tmp, lock, sbom)
}

fn property<'a>(component: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    component["properties"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == name)
        .and_then(|p| p["value"].as_str())
}

/// CycloneDX 1.5 `dependencies` is `uniqueItems`, and one `ref` names one
/// component. Every component has exactly one dependency entry.
#[test]
fn every_closure_component_has_one_dependency_entry() {
    let (_tmp, _lock, sbom) = page_solve("tests/sbom_recorded/cbc");
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for entry in sbom["dependencies"].as_array().unwrap() {
        *seen
            .entry(entry["ref"].as_str().unwrap().to_string())
            .or_default() += 1;
    }
    let duplicated: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
    let components: BTreeSet<String> = sbom["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["bom-ref"].as_str().unwrap().to_string())
        .collect();
    let missing: Vec<_> = components
        .iter()
        .filter(|r| !seen.contains_key(*r))
        .collect();
    assert!(
        duplicated.is_empty() && missing.is_empty(),
        "duplicated dependency refs {duplicated:?}; components with no entry {missing:?}"
    );
}

/// A component's sha256 is the checksum its own easyconfig states.
#[test]
fn every_closure_component_carries_its_own_recipe_checksum() {
    let (_tmp, _lock, sbom) = page_solve("tests/sbom_recorded/cbc");
    let mut wrong = Vec::new();
    for component in sbom["components"].as_array().unwrap() {
        let Some(path) = property(component, "easybuild:easyconfig_path") else {
            continue;
        };
        let recipe = eb_stack::resolve_easyconfig_file(&repo().join(path)).expect("recipe");
        let stated: BTreeSet<String> = recipe
            .checksums
            .iter()
            .filter(|c| c.len() == 64)
            .cloned()
            .collect();
        let carried: BTreeSet<String> = component["hashes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| h["content"].as_str().map(str::to_string))
            .collect();
        if stated != carried {
            wrong.push(format!(
                "{}: recipe {:?}, sbom {:?}",
                component["bom-ref"], stated, carried
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// `Cbc-2.10.12-foss-2025b.eb` names `('zlib', '1.3.1')`. The page quotes a
/// `dependsOn` edge to zlib 1.2.11.
#[test]
fn cbc_depends_on_the_zlib_its_recipe_names() {
    let (_tmp, _lock, sbom) = page_solve("tests/sbom_recorded/cbc");
    let cbc = sbom["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["ref"].as_str().unwrap().starts_with("pkg:generic/Cbc@"))
        .expect("Cbc entry");
    let zlib: Vec<&str> = cbc["dependsOn"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str())
        .filter(|r| r.starts_with("pkg:generic/zlib@"))
        .collect();
    assert!(
        zlib.len() == 1 && zlib[0].starts_with("pkg:generic/zlib@1.3.1"),
        "Cbc dependsOn {zlib:?}"
    );
}
