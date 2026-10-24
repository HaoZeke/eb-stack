//! `recipe easyblock` and `recipe hierarchy` against recorded output.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/recipe_command_fixtures")
        .join(name)
}

fn run(args: &[&str]) -> Vec<u8> {
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(args)
        .output()
        .expect("run eb-stack");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{args:?} wrote to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn recipe_easyblock_matches_the_recorded_names_and_charmap() {
    assert_eq!(
        run(&["recipe", "easyblock", "zlib"]),
        std::fs::read(fixture("easyblock-zlib.txt")).expect("zlib fixture")
    );
    assert_eq!(
        run(&["recipe", "easyblock", "c++"]),
        std::fs::read(fixture("easyblock-cxx.txt")).expect("c++ fixture")
    );
    assert_eq!(
        run(&["recipe", "easyblock", "GROMACS"]),
        std::fs::read(fixture("easyblock-gromacs.txt")).expect("GROMACS fixture")
    );
    let charmap = run(&["recipe", "easyblock", "--charmap"]);
    assert_eq!(
        charmap,
        std::fs::read(fixture("easyblock-charmap.json")).expect("charmap fixture")
    );
    let parsed: serde_json::Value = serde_json::from_slice(&charmap).expect("charmap json");
    assert_eq!(parsed["prefix"], "EB_");
    assert_eq!(parsed["charmap"].as_array().expect("charmap").len(), 33);
}

#[test]
fn recipe_hierarchy_matches_the_recorded_tables() {
    let plain = run(&["recipe", "hierarchy"]);
    let exported = run(&["recipe", "hierarchy", "--export"]);
    assert_eq!(
        plain,
        std::fs::read(fixture("hierarchy.json")).expect("hierarchy fixture")
    );
    assert_eq!(
        exported,
        std::fs::read(fixture("hierarchy-export.json")).expect("export fixture")
    );
    let plain_json: serde_json::Value = serde_json::from_slice(&plain).expect("plain json");
    let export_json: serde_json::Value = serde_json::from_slice(&exported).expect("export json");
    assert!(plain_json.get("synthetic").is_none());
    assert_eq!(export_json["generations"], plain_json["generations"]);
    assert!(
        export_json["synthetic"]
            .as_array()
            .expect("synthetic")
            .len()
            >= 2
    );
}
