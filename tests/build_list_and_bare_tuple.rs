//! The build list the closure page writes, and where a bare tuple resolves.
//!
//! The closure page runs `stack solve --build-list-out` on
//! `tests/sbom_recorded/cbc` and calls the result "the easyconfigs to
//! build". `formats.org` says a dependency's path always appears before
//! every co-selected package that needs it, runtime or build-time. A bare
//! dependency tuple does not reach SYSTEM in `stack solve` and `recipe
//! check`; `stack order` and the closures built on `build_order` must agree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn eb_stack(args: &[&str], dir: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(args)
        .env("TMPDIR", dir)
        .output()
        .expect("run eb-stack")
}

struct PageSolve {
    _tmp: tempfile::TempDir,
    lock: serde_json::Value,
    sbom: serde_json::Value,
    build_list: Vec<String>,
}

/// The closure page's `stack solve`, with the build list and SBOM it writes.
fn page_solve() -> PageSolve {
    let tmp = tempfile::tempdir().expect("temp");
    let lock = tmp.path().join("stack.lock.json");
    let sbom = tmp.path().join("stack.cdx.json");
    let list = tmp.path().join("build.list");
    let out = eb_stack(
        &[
            "stack",
            "solve",
            "--easyconfigs",
            "tests/sbom_recorded/cbc",
            "--baseline-easyconfigs",
            "tests/sbom_recorded/cbc",
            "--toolchain",
            "foss/2025b",
            "--root",
            "Cbc",
            "--lock-out",
            lock.to_str().unwrap(),
            "--sbom-out",
            sbom.to_str().unwrap(),
            "--build-list-out",
            list.to_str().unwrap(),
        ],
        tmp.path(),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let read = |path: &Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    let build_list = std::fs::read_to_string(&list)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    PageSolve {
        lock: read(&lock),
        sbom: read(&sbom),
        build_list,
        _tmp: tmp,
    }
}

fn toolchain_label(toolchain: &serde_json::Value) -> String {
    let name = toolchain["name"].as_str().unwrap();
    if name.eq_ignore_ascii_case("system") {
        "system-system".into()
    } else {
        format!("{name}-{}", toolchain["version"].as_str().unwrap())
    }
}

/// bom-ref to the easyconfig path the lock records for it.
fn paths_by_ref(lock: &serde_json::Value) -> BTreeMap<String, String> {
    lock["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                format!(
                    "pkg:generic/{}@{}?toolchain={}",
                    p["name"].as_str().unwrap(),
                    p["version"].as_str().unwrap(),
                    toolchain_label(&p["toolchain"])
                ),
                p["easyconfig_path"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The lock has 58 modules, including two builds of one name at one level
/// (`M4-1.4.19.eb` and `M4-1.4.21.eb`, `binutils-2.44.eb` and
/// `binutils-2.45.eb`). The build list must name every one of them.
#[test]
fn the_build_list_names_every_locked_module() {
    let page = page_solve();
    let locked: BTreeSet<String> = paths_by_ref(&page.lock).into_values().collect();
    let listed: BTreeSet<String> = page.build_list.iter().cloned().collect();
    let missing: Vec<&String> = locked.difference(&listed).collect();
    assert!(
        missing.is_empty(),
        "build list has {} of {} locked modules; missing {missing:?}",
        listed.len(),
        locked.len()
    );
    assert_eq!(
        page.build_list.len(),
        locked.len(),
        "a module is listed twice"
    );
}

/// `formats.org`: a dependency's path always appears before every
/// co-selected package that needs it, runtime or build-time. The edges are
/// the ones the same solve writes into its SBOM.
#[test]
fn the_build_list_puts_every_dependency_first() {
    let page = page_solve();
    let paths = paths_by_ref(&page.lock);
    let position: BTreeMap<&str, usize> = page
        .build_list
        .iter()
        .enumerate()
        .map(|(index, path)| (path.as_str(), index))
        .collect();
    let mut edges: Vec<(String, String, &str)> = Vec::new();
    for entry in page.sbom["dependencies"].as_array().unwrap() {
        let from = entry["ref"].as_str().unwrap().to_string();
        for to in entry["dependsOn"].as_array().into_iter().flatten() {
            edges.push((from.clone(), to.as_str().unwrap().to_string(), "runtime"));
        }
    }
    for component in page.sbom["components"].as_array().unwrap() {
        let from = component["bom-ref"].as_str().unwrap().to_string();
        let build = component["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["name"] == "eb_stack:buildDependsOn")
            .and_then(|p| p["value"].as_str())
            .unwrap_or("");
        for to in build.split(',').filter(|to| !to.is_empty()) {
            edges.push((from.clone(), to.to_string(), "build"));
        }
    }
    let mut late = Vec::new();
    for (from, to, kind) in &edges {
        let (Some(from_at), Some(to_at)) = (
            position.get(paths[from].as_str()),
            position.get(paths[to].as_str()),
        ) else {
            late.push(format!("{kind} {from} -> {to}: not on the list"));
            continue;
        };
        if to_at > from_at {
            late.push(format!(
                "{kind} {from} (line {}) needs {to} (line {})",
                from_at + 1,
                to_at + 1
            ));
        }
    }
    assert!(
        late.is_empty(),
        "{} edges out of order:\n{}",
        late.len(),
        late.join("\n")
    );
}

fn only_system_tree() -> tempfile::TempDir {
    let tree = tempfile::tempdir().expect("temp");
    let zlib = tree.path().join("easybuild/easyconfigs/z/zlib");
    let app = tree.path().join("easybuild/easyconfigs/a/App");
    std::fs::create_dir_all(&zlib).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        zlib.join("zlib-1.3.1.eb"),
        "name = 'zlib'\nversion = '1.3.1'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    std::fs::write(
        app.join("App-1.0-foss-2025b.eb"),
        "name = 'App'\nversion = '1.0'\n\
         toolchain = {'name': 'foss', 'version': '2025b'}\n\
         dependencies = [('zlib', '1.3.1')]\n",
    )
    .unwrap();
    tree
}

/// `stack solve` and `recipe check` reject a bare zlib tuple under foss-2025b
/// when only the SYSTEM recipe exists. `stack order` resolves through
/// `build_order::resolve_dep`, the resolver pr-overlap, rebuild-set and the
/// reach commands share, and must not order App on that SYSTEM zlib.
#[test]
fn stack_order_keeps_a_bare_dependency_off_system() {
    let tree = only_system_tree();
    let dir = tree.path().to_str().unwrap();
    let out_file = tree.path().join("build-order.txt");
    let solve = eb_stack(
        &[
            "stack",
            "solve",
            "--easyconfigs",
            dir,
            "--toolchain",
            "foss/2025b",
            "--root",
            "App",
            "--lock-out",
            tree.path().join("stack.lock.json").to_str().unwrap(),
        ],
        tree.path(),
    );
    assert!(!solve.status.success(), "stack solve took the SYSTEM zlib");
    let order = eb_stack(
        &[
            "stack",
            "order",
            "--easyconfigs",
            dir,
            "--root",
            "App",
            "--out",
            out_file.to_str().unwrap(),
        ],
        tree.path(),
    );
    let written = std::fs::read_to_string(&out_file).unwrap_or_default();
    assert!(
        !order.status.success(),
        "stack order resolved a bare tuple to SYSTEM, which stack solve and \
         recipe check reject:\n{written}"
    );
}

/// `recipe check` tells the author to write the SYSTEM tuple. `stack solve`
/// on the same recipe only says the package is missing, though the tree has
/// it at SYSTEM.
#[test]
fn stack_solve_says_how_to_reach_a_system_only_dependency() {
    let tree = tempfile::tempdir().expect("temp");
    let java = tree.path().join("easybuild/easyconfigs/j/Java");
    let app = tree.path().join("easybuild/easyconfigs/a/App");
    std::fs::create_dir_all(&java).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        java.join("Java-17.eb"),
        "name = 'Java'\nversion = '17'\ntoolchain = SYSTEM\n",
    )
    .unwrap();
    std::fs::write(
        app.join("App-1.0-foss-2025b.eb"),
        "name = 'App'\nversion = '1.0'\n\
         toolchain = {'name': 'foss', 'version': '2025b'}\n\
         dependencies = [('Java', '17')]\n",
    )
    .unwrap();
    let out = eb_stack(
        &[
            "stack",
            "solve",
            "--easyconfigs",
            tree.path().to_str().unwrap(),
            "--toolchain",
            "foss/2025b",
            "--root",
            "App",
            "--lock-out",
            tree.path().join("stack.lock.json").to_str().unwrap(),
        ],
        tree.path(),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("('Java', '17', '', SYSTEM)"),
        "stack solve does not say the tree has Java 17 at SYSTEM:\n{stderr}"
    );
}
