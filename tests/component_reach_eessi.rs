//! Reach of three CVEs in the EESSI 2025.06 easystack set.
//!
//! Ignored unless `scripts/fetch-upstream-trees.sh` has been sourced.
//! `EB_EASYCONFIGS_DEVELOP` is the pinned easybuild-easyconfigs checkout.
//! `EB_EESSI_SOFTWARE_LAYER` is EESSI/software-layer at the commit that
//! script records. The test reads those trees and does not clone.

use eb_stack::component_reach::{
    component_reach, cve_reach, cve_to_toml, render_cve_markdown, render_markdown,
    CveReachDocument, ReachDocument, ReachQuery, ReachScope,
};
use eb_stack::dep_filter::FilterDeps;
use eb_stack::easystack::parse_easystack_entries;
use eb_stack::eb_parse::parse_easyconfig_file;
use eb_stack::osv::{load_recorded_osv, resolve_vulnerability};
use eb_stack::rebuild_set::StackEasystack;
use eb_stack::write_json_pretty;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const STACK_PREFIX: &str = "easystacks/software.eessi.io/2025.06/";

#[test]
#[ignore = "needs pinned trees from scripts/fetch-upstream-trees.sh"]
fn cves_in_the_eessi_2025_06_easystacks() {
    let recorded = recorded_osv();
    let client = load_recorded_osv(&recorded).expect("recorded OSV responses");
    let files = easystack_files();
    assert!(
        files.len() >= 100,
        "expected the 2025.06 easystack set, got {}",
        files.len()
    );
    let stacks = load_easystacks(&files);
    let names = easystack_names(&stacks);
    assert!(
        names.len() > 900,
        "easystacks named {} easyconfigs",
        names.len()
    );
    let root = easyconfigs_root();
    let index = index_easyconfigs(&root);
    let (candidates, missing, skipped) = parse_named(&index, &names);
    assert!(
        skipped.is_empty(),
        "unparsed stack easyconfigs: {}",
        skipped.join("; ")
    );
    assert!(
        missing.len() < 20,
        "{} stack easyconfigs are absent from {}: {}",
        missing.len(),
        root.display(),
        missing.join(", ")
    );
    assert!(candidates.len() > 900, "parsed {}", candidates.len());
    let filter = FilterDeps::empty();
    let out = output_dir();
    let _ = std::fs::remove_dir_all(&out);

    let webpki = resolve_vulnerability("CVE-2026-93599", &client).expect("rustls-webpki CVE");
    assert!(
        webpki
            .components
            .iter()
            .any(|item| item.name == "rustls-webpki"),
        "{:?}",
        webpki.components
    );
    let webpki_doc = reach(
        &webpki.id,
        &webpki.components,
        &webpki.notes,
        &candidates,
        "2025a",
        &stacks,
        &filter,
    );
    let webpki_hit = require_carrier(
        &webpki_doc,
        "rustls-webpki",
        "uv-0.7.13-GCCcore-14.2.0.eb",
        "crate",
        "0.102.8",
    );
    assert!(webpki_hit.closure_counts.must_rebuild >= 1);
    assert!(webpki_hit
        .changed
        .iter()
        .any(|file| file == "uv-0.7.13-GCCcore-14.2.0.eb"));
    assert!(names.contains("uv-0.7.13-GCCcore-14.2.0.eb"));
    write_cve(&out.join("CVE-2026-93599-2025a"), &webpki_doc);

    let urllib = resolve_vulnerability("CVE-2025-66471", &client).expect("urllib3 CVE");
    let urllib_doc = reach(
        &urllib.id,
        &urllib.components,
        &urllib.notes,
        &candidates,
        "2025b",
        &stacks,
        &filter,
    );
    let urllib_hit = require_carrier(
        &urllib_doc,
        "urllib3",
        "Python-bundle-PyPI-2025.07-GCCcore-14.3.0.eb",
        "extension",
        "2.5.0",
    );
    assert!(urllib_hit.closure_counts.must_rebuild >= 1);
    write_cve(&out.join("CVE-2025-66471-2025b"), &urllib_doc);

    let libxpm = resolve_vulnerability("CVE-2026-94287", &client).expect("libXpm CVE");
    for (generation, file) in [
        ("2025b", "X11-20250608-GCCcore-14.3.0.eb"),
        ("2025a", "X11-20250521-GCCcore-14.2.0.eb"),
        ("2024a", "X11-20240607-GCCcore-13.3.0.eb"),
    ] {
        let doc = reach(
            &libxpm.id,
            &libxpm.components,
            &libxpm.notes,
            &candidates,
            generation,
            &stacks,
            &filter,
        );
        let hit = require_carrier(&doc, "libXpm", file, "component", "3.5.17");
        assert!(hit.closure_counts.must_rebuild >= 1, "{generation}");
        assert!(
            names.contains(file),
            "{file} is not in the 2025.06 easystacks"
        );
        write_cve(&out.join(format!("CVE-2026-94287-{generation}")), &doc);
    }

    let pastel = component_reach(&ReachQuery {
        candidates: &candidates,
        component: "pastel",
        version_req: "0.2.1",
        also_reqs: &[],
        exact_versions: &[],
        purl: Some("pkg:pypi/pastel@0.2.1"),
        ecosystem: Some("PyPI"),
        generation: "2025b",
        installed: None,
        easystacks: &stacks,
        compat: &[],
        filter_deps: &filter,
        generated_at: "2026-10-24T00:00:00Z",
    });
    assert!(
        pastel.carriers.iter().any(|row| {
            row.easyconfig == "Python-bundle-PyPI-2025.07-GCCcore-14.3.0.eb"
                && row.role == "wheel"
                && row.version == "0.2.1"
        }),
        "pastel wheel missing: {:?}",
        pastel.carriers
    );
    let cleo = component_reach(&ReachQuery {
        candidates: &candidates,
        component: "cleo",
        version_req: "2.1.0",
        also_reqs: &[],
        exact_versions: &[],
        purl: Some("pkg:pypi/cleo@2.1.0"),
        ecosystem: Some("PyPI"),
        generation: "2025b",
        installed: None,
        easystacks: &stacks,
        compat: &[],
        filter_deps: &filter,
        generated_at: "2026-10-24T00:00:00Z",
    });
    assert!(
        cleo.carriers.iter().any(|row| {
            row.easyconfig == "Python-bundle-PyPI-2025.07-GCCcore-14.3.0.eb"
                && row.role == "wheel"
                && row.version == "2.1.0"
        }),
        "cleo wheel missing: {:?}",
        cleo.carriers
    );
    let wheel_dir = out.join("wheels-2025b");
    std::fs::create_dir_all(&wheel_dir).unwrap();
    std::fs::write(wheel_dir.join("pastel.md"), render_markdown(&pastel)).unwrap();
    std::fs::write(wheel_dir.join("cleo.md"), render_markdown(&cleo)).unwrap();
    write_json_pretty(&wheel_dir.join("pastel.json"), &pastel).unwrap();

    let cli = cli_rustls(&index, &names, &files, &out.join("cli-rustls"), &recorded);
    assert_eq!(cli["kind"], "cve");
    assert_eq!(cli["cve"], "CVE-2026-93599");
    let cli_carriers = cli["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["component"] == "rustls-webpki")
        .expect("cli rustls-webpki");
    assert!(cli_carriers["carriers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| {
            row["easyconfig"] == "uv-0.7.13-GCCcore-14.2.0.eb"
                && row["role"] == "crate"
                && row["version"] == "0.102.8"
        }));
}

fn require_dir(key: &str) -> PathBuf {
    let Ok(value) = std::env::var(key) else {
        panic!("{key} is unset; source env.sh from scripts/fetch-upstream-trees.sh");
    };
    let path = PathBuf::from(&value);
    assert!(
        path.is_dir(),
        "{key} is not a directory ({}); source env.sh from scripts/fetch-upstream-trees.sh",
        path.display()
    );
    path
}

fn output_dir() -> PathBuf {
    std::env::var_os("EB_COMPONENT_REACH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("component-reach-eessi"))
}

fn easystack_files() -> Vec<PathBuf> {
    let root = require_dir("EB_EESSI_SOFTWARE_LAYER");
    let prefix = root.join(STACK_PREFIX);
    assert!(
        prefix.is_dir(),
        "missing {} under EB_EESSI_SOFTWARE_LAYER",
        prefix.display()
    );
    let mut files = Vec::new();
    let mut stack = vec![prefix];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        {
            let path = entry.expect("dirent").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("yml") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn load_easystacks(files: &[PathBuf]) -> Vec<StackEasystack> {
    files
        .iter()
        .map(|path| StackEasystack {
            path: path.display().to_string(),
            text: std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display())),
        })
        .collect()
}

fn easystack_names(stacks: &[StackEasystack]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for stack in stacks {
        let entries = parse_easystack_entries(&stack.text)
            .unwrap_or_else(|error| panic!("parse {}: {error}", stack.path));
        for entry in entries {
            names.insert(entry.file);
        }
    }
    names
}

fn easyconfigs_root() -> PathBuf {
    let root = require_dir("EB_EASYCONFIGS_DEVELOP").join("easybuild/easyconfigs");
    assert!(
        root.is_dir(),
        "missing {}; EB_EASYCONFIGS_DEVELOP must be the easybuild-easyconfigs checkout",
        root.display()
    );
    root
}

fn index_easyconfigs(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        {
            let path = entry.expect("dirent").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("eb") {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    out.insert(name.to_string(), path);
                }
            }
        }
    }
    out
}

fn parse_named(
    index: &BTreeMap<String, PathBuf>,
    names: &BTreeSet<String>,
) -> (Vec<eb_stack::domain::Candidate>, Vec<String>, Vec<String>) {
    let mut candidates = Vec::new();
    let mut missing = Vec::new();
    let mut skipped = Vec::new();
    for name in names {
        let Some(path) = index.get(name) else {
            missing.push(name.clone());
            continue;
        };
        match parse_easyconfig_file(path) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => skipped.push(format!("{name}: {error}")),
        }
    }
    (candidates, missing, skipped)
}

fn reach(
    cve: &str,
    components: &[eb_stack::osv::ResolvedComponent],
    notes: &[String],
    candidates: &[eb_stack::domain::Candidate],
    generation: &str,
    stacks: &[StackEasystack],
    filter: &FilterDeps,
) -> CveReachDocument {
    cve_reach(
        cve,
        components,
        notes,
        &[],
        &ReachScope {
            candidates,
            generation,
            installed: None,
            easystacks: stacks,
            compat: &[],
            filter_deps: filter,
            generated_at: "2026-10-24T00:00:00Z",
        },
    )
}

fn require_carrier<'a>(
    document: &'a CveReachDocument,
    component: &str,
    file: &str,
    role: &str,
    version: &str,
) -> &'a ReachDocument {
    let result = document
        .results
        .iter()
        .find(|item| item.component == component)
        .unwrap_or_else(|| {
            panic!(
                "{component} not in {} (packages: {:?})",
                document.cve,
                document
                    .results
                    .iter()
                    .map(|item| item.component.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        result
            .carriers
            .iter()
            .any(|row| { row.easyconfig == file && row.role == role && row.version == version }),
        "{component} missing {file} {role} {version}; carriers: {:?}",
        result
            .carriers
            .iter()
            .map(|row| format!("{} {} {}", row.easyconfig, row.role, row.version))
            .collect::<Vec<_>>()
    );
    result
}

fn write_cve(dir: &Path, document: &CveReachDocument) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("component-reach.toml"),
        cve_to_toml(document).unwrap(),
    )
    .unwrap();
    write_json_pretty(&dir.join("component-reach.json"), document).unwrap();
    std::fs::write(
        dir.join("component-reach.md"),
        render_cve_markdown(document),
    )
    .unwrap();
}

fn recorded_osv() -> PathBuf {
    let dir = Path::new(file!())
        .parent()
        .expect("test file directory")
        .join("component_reach_eessi/osv");
    let manifest = std::fs::read_to_string(dir.join("queries.json"))
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    assert!(
        manifest.contains("\"fetched\": \"2026-10-24\""),
        "the recorded OSV responses name the day they were saved:\n{manifest}"
    );
    for id in ["CVE-2026-93599", "CVE-2025-66471", "CVE-2026-94287"] {
        let url = format!("https://api.osv.dev/v1/vulns/{id}");
        assert!(
            manifest.contains(&url),
            "queries.json does not record {url}"
        );
    }
    dir
}

fn cli_rustls(
    index: &BTreeMap<String, PathBuf>,
    names: &BTreeSet<String>,
    stacks: &[PathBuf],
    out: &Path,
    osv: &Path,
) -> serde_json::Value {
    let link_dir = out.join("easyconfigs");
    std::fs::create_dir_all(&link_dir).unwrap();
    for name in names {
        if let Some(path) = index.get(name) {
            let dest = link_dir.join(name);
            if !dest.exists() {
                std::os::unix::fs::symlink(path, dest).unwrap();
            }
        }
    }
    let report = out.join("report");
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "component-reach",
            "--cve",
            "CVE-2026-93599",
            "--generation",
            "2025a",
            "--easyconfigs",
        ])
        .arg(&link_dir)
        .args(["--generated-at", "2026-10-24T00:00:00Z", "--osv-cache"])
        .arg(osv)
        .args(["--out-dir"])
        .arg(&report);
    for path in stacks {
        command.arg("--easystack").arg(path);
    }
    let output = command.output().expect("cli");
    assert!(
        output.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(out.join("cli-stdout.txt"), &output.stdout).unwrap();
    serde_json::from_str(&std::fs::read_to_string(report.join("component-reach.json")).unwrap())
        .unwrap()
}
