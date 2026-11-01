//! The solved dependency DAG is the lock's recipes, not a second solver.

use std::fs;
use std::path::Path;
use std::process::Command;

use eb_stack::{
    solved_dependency_dot, write_solved_dependency_graph, LockPackage, SolverMeta, StackLock,
    Toolchain,
};

fn toolchain(name: &str, version: &str) -> Toolchain {
    Toolchain {
        name: name.into(),
        version: version.into(),
    }
}

fn package(
    name: &str,
    version: &str,
    toolchain: Toolchain,
    versionsuffix: Option<&str>,
    path: &str,
) -> LockPackage {
    LockPackage {
        name: name.into(),
        version: version.into(),
        toolchain,
        versionsuffix: versionsuffix.map(str::to_string),
        easyconfig_path: path.into(),
    }
}

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}

#[test]
fn solved_dot_names_versions_toolchains_and_requirements() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        dir,
        "App-1.2-foss-2025a.eb",
        "\
name = 'App'
version = '1.2'
toolchain = {'name': 'foss', 'version': '2025a'}
dependencies = [
    ('Lib', '>=1.4,<1.9', '-fb', ('GCC', '14.2.0')),
]
builddependencies = [
    ('CMake', '3.30.0'),
]
",
    );
    write(
        dir,
        "Lib-1.5-GCC-14.2.0-fb.eb",
        "\
name = 'Lib'
version = '1.5'
versionsuffix = '-fb'
toolchain = {'name': 'GCC', 'version': '14.2.0'}
dependencies = []
",
    );
    write(
        dir,
        "CMake-3.30.0-GCCcore-14.2.0.eb",
        "\
name = 'CMake'
version = '3.30.0'
toolchain = {'name': 'GCCcore', 'version': '14.2.0'}
dependencies = []
",
    );
    let lock = StackLock {
        schema_version: 1,
        toolchain: toolchain("foss", "2025a"),
        generation_label: None,
        packages: vec![
            package(
                "App",
                "1.2",
                toolchain("foss", "2025a"),
                None,
                "App-1.2-foss-2025a.eb",
            ),
            package(
                "Lib",
                "1.5",
                toolchain("GCC", "14.2.0"),
                Some("-fb"),
                "Lib-1.5-GCC-14.2.0-fb.eb",
            ),
            package(
                "CMake",
                "3.30.0",
                toolchain("GCCcore", "14.2.0"),
                None,
                "CMake-3.30.0-GCCcore-14.2.0.eb",
            ),
        ],
        solver: SolverMeta {
            engine: "resolvo_cdcl_sat".into(),
            engine_version: "test".into(),
            timestamp: "STABLE".into(),
            criteria: Vec::new(),
        },
    };
    let lock_path = dir.join("stack.lock.json");
    fs::write(&lock_path, serde_json::to_string(&lock).unwrap()).unwrap();
    let dot = solved_dependency_dot(&lock, &lock_path);
    assert!(dot.contains("label=\"App/1.2\\nfoss-2025a\""), "{dot}");
    assert!(dot.contains("label=\"Lib/1.5-fb\\nGCC-14.2.0\""), "{dot}");
    assert!(
        dot.contains("label=\"CMake/3.30.0\\nGCCcore-14.2.0\""),
        "{dot}"
    );
    assert!(
        dot.contains("label=\">=1.4,<1.9 suffix -fb toolchain GCC/14.2.0\""),
        "{dot}"
    );
    assert!(
        dot.contains("label=\"==3.30.0\", color=\"#004D40\", style=dashed"),
        "{dot}"
    );
    let lib = dot
        .lines()
        .find(|line| line.contains("Lib/1.5-fb"))
        .unwrap();
    let cmake = dot
        .lines()
        .find(|line| line.contains("CMake/3.30.0"))
        .unwrap();
    let lib_id = lib.split_whitespace().next().unwrap();
    let cmake_id = cmake.split_whitespace().next().unwrap();
    assert!(dot.contains(&format!("-> {lib_id} [label=\">=1.4,<1.9")));
    assert!(dot.contains(&format!("-> {cmake_id} [label=\"==3.30.0\"")));

    let dot_path = dir.join("stack.dot");
    write_solved_dependency_graph(&lock, &lock_path, &dot_path).unwrap();
    assert_eq!(fs::read_to_string(&dot_path).unwrap(), dot);

    if Command::new("dot").arg("-V").output().is_ok() {
        let png = dir.join("stack.png");
        write_solved_dependency_graph(&lock, &lock_path, &png).unwrap();
        let bytes = fs::read(&png).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"), "png magic");
    }
}
