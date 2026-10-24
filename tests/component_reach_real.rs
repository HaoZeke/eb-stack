//! Ignored. easybuild-easyconfigs develop and v5.4.1, generation 2026.1.
//!
//! Defaults are `/tmp/easyconfigs-develop` and `/tmp/easyconfigs-5.4.1`.
//! `EB_EASYCONFIGS_DEVELOP`, `EB_EASYCONFIGS_541`, and `EB_COMPONENT_REACH_OUT`
//! override the checkouts and the report directory.

use eb_stack::component_reach::{component_reach, render_markdown, to_toml, ReachQuery};
use eb_stack::dep_filter::FilterDeps;
use eb_stack::eb_parse::parse_easyconfig_tree;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const GENERATION: &str = "2026.1";

type Hit = (&'static str, &'static str, &'static str);
type Case = (&'static str, &'static [Hit], &'static [&'static str]);

/// `(component, [(file, role, version)], banned filename fragments)`.
const CASES: &[Case] = &[
    (
        "curl",
        &[("cURL-8.17.0-GCCcore-15.2.0.eb", "module", "8.17.0")],
        &[],
    ),
    (
        "HDF5",
        &[
            ("HDF5-2.1.1-gompi-2026.1.eb", "module", "2.1.1"),
            ("HDF5-2.1.1-iimpi-2026.1.eb", "module", "2.1.1"),
            ("HDF5-2.1.1-lompi-2026.1.eb", "module", "2.1.1"),
        ],
        &[],
    ),
    (
        "libz-sys",
        &[
            ("cargo-c-0.10.23-GCCcore-15.2.0.eb", "crate", "1.1.29"),
            ("rsx-0.2.10-GCCcore-15.2.0.eb", "crate", "1.1.28"),
        ],
        &[],
    ),
    (
        "zlib-rs",
        &[
            ("cargo-c-0.10.23-GCCcore-15.2.0.eb", "crate", "0.6.4"),
            ("rsx-0.2.10-GCCcore-15.2.0.eb", "crate", "0.6.3"),
        ],
        &[],
    ),
    (
        "jupyterlab",
        &[
            ("JupyterLab-4.5.8-GCCcore-15.2.0.eb", "module", "4.5.8"),
            ("JupyterLab-4.5.8-GCCcore-15.2.0.eb", "extension", "4.5.8"),
        ],
        &[
            "jupyterlab-server",
            "jupyterlab-widgets",
            "jupyterlab_widgets",
            "jupyterlab-h5web",
        ],
    ),
    (
        "urllib3",
        &[
            (
                "Python-bundle-PyPI-2026.04-GCCcore-15.2.0.eb",
                "extension",
                "2.6.3",
            ),
            ("dulwich-1.2.0-GCCcore-15.2.0.eb", "extension", "2.6.3"),
            ("flit-3.12.0-GCCcore-15.2.0.eb", "extension", "2.7.0"),
        ],
        &[],
    ),
];

#[test]
#[ignore = "needs easybuild-easyconfigs develop and v5.4.1"]
fn known_components_in_generation_2026_1() {
    let out = std::env::var_os("EB_COMPONENT_REACH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("component-reach-real"));
    let trees = [
        (
            "develop",
            env_root("EB_EASYCONFIGS_DEVELOP", "/tmp/easyconfigs-develop"),
        ),
        (
            "v5.4.1",
            env_root("EB_EASYCONFIGS_541", "/tmp/easyconfigs-5.4.1"),
        ),
    ];
    let filter = FilterDeps::empty();
    let mut summary = Vec::new();
    for (label, root) in trees {
        let easyconfigs = root.join("easybuild/easyconfigs");
        assert!(easyconfigs.is_dir(), "missing {}", easyconfigs.display());
        let parsed = parse_easyconfig_tree(&easyconfigs).expect("parse");
        assert!(parsed.skipped.is_empty(), "{label} {:?}", parsed.skipped);
        let by_name = index_easyconfigs(&easyconfigs);
        for (component, hits, absent) in CASES {
            let doc = component_reach(&ReachQuery {
                candidates: &parsed.candidates,
                component,
                version_req: "*",
                generation: GENERATION,
                installed: None,
                easystacks: &[],
                compat: &[],
                filter_deps: &filter,
                generated_at: "2026-10-22T12:00:00Z",
                also_reqs: &[],
                exact_versions: &[],
                purl: None,
                ecosystem: None,
            });
            for (file, role, version) in *hits {
                assert!(
                    doc.carriers.iter().any(|row| {
                        row.easyconfig == *file && row.role == *role && row.version == *version
                    }),
                    "{label} {component} missing {file} {role} {version}"
                );
            }
            for banned in *absent {
                assert!(
                    doc.carriers
                        .iter()
                        .all(|row| { !row.easyconfig.to_ascii_lowercase().contains(banned) }),
                    "{label} {component} carried {banned}"
                );
            }
            let grep_hits = grep_generation(&easyconfigs, component);
            for row in &doc.carriers {
                if row.easyconfig.is_empty() {
                    continue;
                }
                let path = by_name.get(&row.easyconfig).expect(&row.easyconfig);
                let text = std::fs::read_to_string(path).unwrap_or_default();
                assert!(
                    text.to_ascii_lowercase()
                        .contains(&component.to_ascii_lowercase()),
                    "{label} {component} {}",
                    row.easyconfig
                );
                assert!(
                    grep_hits.iter().any(|name| name == &row.easyconfig),
                    "{label} {component} {} missing from grep",
                    row.easyconfig
                );
            }
            assert!(doc.closure_counts.must_rebuild >= 1, "{label} {component}");
            let dir = out.join(label).join(component);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("component-reach.toml"), to_toml(&doc).unwrap()).unwrap();
            std::fs::write(
                dir.join("component-reach.json"),
                serde_json::to_string_pretty(&doc).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join("component-reach.md"), render_markdown(&doc)).unwrap();
            summary.push(serde_json::json!({
                "tree": label,
                "component": component,
                "carriers": doc.carriers.len(),
                "changed": doc.changed.len(),
                "must_rebuild": doc.closure_counts.must_rebuild,
                "review": doc.closure_counts.review,
                "no_rebuild": doc.closure_counts.no_rebuild,
                "grep_generation_files": grep_hits.len(),
            }));
            eprintln!(
                "{label} {component} carriers={} changed={} must={} review={} no={} grep={}",
                doc.carriers.len(),
                doc.changed.len(),
                doc.closure_counts.must_rebuild,
                doc.closure_counts.review,
                doc.closure_counts.no_rebuild,
                grep_hits.len()
            );
        }
    }
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
}

fn env_root(key: &str, default: &str) -> PathBuf {
    std::env::var_os(key)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn grep_generation(root: &Path, component: &str) -> Vec<String> {
    let output = Command::new("grep")
        .args(["-l", "-i", "-F", "-r", "--include=*.eb", component])
        .arg(root)
        .output()
        .expect("grep");
    let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("2026.1") || line.contains("15.2.0"))
        .filter_map(|line| line.rsplit('/').next().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn index_easyconfigs(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "eb") {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    out.insert(name.to_string(), path);
                }
            }
        }
    }
    out
}
