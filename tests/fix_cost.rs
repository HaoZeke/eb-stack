//! `eb-stack rebuild-set --findings` prices a version bump against an in-place
//! patch for each synthetic advisory.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fix_cost_fixtures")
}

#[test]
fn a_findings_export_prices_a_bump_against_an_in_place_patch() {
    let temp = tempfile::tempdir().expect("tempdir");
    let out = temp.path().join("out");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args(["rebuild-set", "--easyconfigs"])
        .arg(fixtures().join("release"))
        .arg("--installed")
        .arg(fixtures().join("installed.txt"))
        .args(["--installed-label", "x86_64/amd/zen4"])
        .args(["--generated-at", "2026-11-01T00:00:00Z"])
        .arg("--findings")
        .arg(fixtures().join("findings.csv"))
        .arg("--reachability")
        .arg(fixtures().join("reach.txt"))
        .arg("--out-dir")
        .arg(&out)
        .output()
        .expect("run rebuild-set");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains("backport=3")
            && stdout.contains("fix_routes=7 bump=10/12 in_place=8/12 recommended=3/12"),
        "{stdout}"
    );

    let value: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(out.join("fix-cost.json")).expect("read json"),
    )
    .expect("json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["stack_modules"], 12);
    assert_eq!(value["findings"], 7);
    assert_eq!(value["rollup"]["bump_modules"], 10);
    assert_eq!(value["rollup"]["bump_fraction"], 0.8333);
    assert_eq!(value["rollup"]["in_place_modules"], 8);
    assert_eq!(value["rollup"]["in_place_fraction"], 0.6667);
    assert_eq!(value["rollup"]["recommended_modules"], 3);
    assert_eq!(value["rollup"]["recommended_fraction"], 0.25);
    assert_eq!(value["rollup"]["static_unknown_modules"], 1);

    let ranked = value["ranked"].as_array().expect("ranked");
    let ids: Vec<&str> = ranked
        .iter()
        .map(|route| route["vuln_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["ADV-3", "ADV-2", "ADV-5", "ADV-6", "ADV-1", "ADV-4", "ADV-7"]
    );

    let zlib = ranked
        .iter()
        .find(|route| route["vuln_id"] == "ADV-1")
        .unwrap();
    assert_eq!(zlib["recommended"], "defer");
    assert_eq!(zlib["reach"], "unclassified");
    assert_eq!(
        zlib["patch_files"],
        serde_json::json!(["zlib-1.2_ADV-1.patch"])
    );
    assert_eq!(zlib["bump"]["breaks_reproducibility"], true);
    assert_eq!(zlib["bump"]["modules"], 5);
    assert_eq!(
        zlib["in_place"]["names"],
        serde_json::json!([
            "AppStatic/1.0-GCCcore-15.2.0",
            "Kit/1.0-GCCcore-15.2.0",
            "zlib/1.2-GCCcore-15.2.0"
        ])
    );
    assert!(zlib["bump"]["names"]
        .as_array()
        .unwrap()
        .iter()
        .all(|name| name != "Kit/1.0-GCCcore-15.2.0"));
    assert_eq!(
        zlib["bundles"],
        serde_json::json!(["Kit/1.0-GCCcore-15.2.0"])
    );
    assert_eq!(
        zlib["static_linkers"],
        serde_json::json!(["AppStatic/1.0-GCCcore-15.2.0"])
    );
    assert_eq!(
        zlib["static_unknown"],
        serde_json::json!(["AppUnknown/1.0-GCCcore-15.2.0"])
    );
    assert_eq!(zlib["not_static"].as_array().unwrap().len(), 2);

    let curl = ranked
        .iter()
        .find(|route| route["vuln_id"] == "ADV-2")
        .unwrap();
    assert_eq!(curl["recommended"], "in-place");
    assert_eq!(curl["reach"], "network-facing");
    let notebook = ranked
        .iter()
        .find(|route| route["vuln_id"] == "ADV-3")
        .unwrap();
    assert_eq!(notebook["recommended"], "in-place");
    assert_eq!(notebook["reach"], "cross-user");
    let python_patch = ranked
        .iter()
        .find(|route| route["vuln_id"] == "ADV-5")
        .unwrap();
    assert_eq!(python_patch["recommended"], "in-place");
    assert_eq!(python_patch["reach"], "network-facing");
    assert!(python_patch["reason"]
        .as_str()
        .unwrap()
        .contains("ABI is stable"));
    let python_minor = ranked
        .iter()
        .find(|route| route["vuln_id"] == "ADV-6")
        .unwrap();
    assert_eq!(python_minor["recommended"], "bump");
    assert_eq!(python_minor["reach"], "network-facing");
    let win = ranked
        .iter()
        .find(|route| route["vuln_id"] == "ADV-7")
        .unwrap();
    assert_eq!(win["recommended"], "defer");
    assert_eq!(win["reach"], "not-reachable");

    let markdown = std::fs::read_to_string(out.join("fix-cost.md")).expect("md");
    assert!(markdown.contains("breaks reproducibility"));
    assert!(markdown.contains("Static link unknown: `AppUnknown/1.0-GCCcore-15.2.0`"));
    let tsv = std::fs::read_to_string(out.join("fix-cost.tsv")).expect("tsv");
    assert!(tsv.lines().next().unwrap().starts_with("rank\tvuln_id\t"));
    assert!(tsv.lines().nth(1).unwrap().starts_with("1\tADV-3\t"));
}
