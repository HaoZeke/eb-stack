//! Ignored. `EB_STACK_EC_DEVELOP` and `EB_STACK_EC_V541` are easyconfig trees.
//! `EB_STACK_ZEN4` is a directory of 2026-10-22 `prs/*.toml` records.

use eb_stack::build_order::{closure_against_installed, module_name, Choice, ModuleKey};
use eb_stack::dep_filter::FilterDeps;
use eb_stack::domain::Candidate;
use eb_stack::pr_overlap::{
    overlap_index, pr_record, OverlapConfig, PrInput, PrRecord, RobotTrees,
};
use eb_stack::{merge_order, parse_easyconfig_tree};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

const STACK: &str = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml";

#[test]
#[ignore]
fn real_trees_build_each_module_in_one_step() {
    let develop = std::env::var("EB_STACK_EC_DEVELOP").expect("EB_STACK_EC_DEVELOP");
    let release = std::env::var("EB_STACK_EC_V541").expect("EB_STACK_EC_V541");
    for (label, root) in [("v5.4.1", release), ("develop", develop)] {
        let start = Instant::now();
        let parsed =
            parse_easyconfig_tree(Path::new(&root)).unwrap_or_else(|e| panic!("{label}: {e}"));
        let (chose, union, steps) = check(&parsed.candidates);
        println!(
            "{label} candidates={} skipped={} chose={chose} union={union} steps={steps} ms={}",
            parsed.candidates.len(),
            parsed.skipped.len(),
            start.elapsed().as_millis()
        );
    }
}

#[test]
#[ignore]
fn zen4_records_from_2026_10_22() {
    let dir = std::env::var("EB_STACK_ZEN4").expect("EB_STACK_ZEN4");
    let mut records = Vec::new();
    for entry in std::fs::read_dir(Path::new(&dir).join("prs")).expect("prs") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).expect("read");
        records.push(
            toml::from_str::<PrRecord>(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
        );
    }
    records.sort_by_key(|r| r.pr);
    let config = config();
    let index = overlap_index(&records, &config);
    let order = merge_order::plan(&records, &index, &[]);
    let again = merge_order::plan(&records, &index, &[]);
    assert_eq!(order.steps, again.steps);
    assert_once(&records, &order);
    println!(
        "{}",
        merge_order::render_section(&order, "https://github.com/EESSI/software-layer")
    );
    println!(
        "zen4 prs={} steps={} union={}",
        records.len(),
        order.steps.len(),
        order.steps.iter().map(|s| s.builds.len()).sum::<usize>()
    );
}

fn check(cands: &[Candidate]) -> (String, usize, usize) {
    let installed = installed_from_roots(cands);
    let rows = closures(cands, &installed);
    let (dep, apps) = chain(&rows).unwrap_or_else(|| panic!("no chain in {} rows", rows.len()));
    println!("dep {} ({})", dep.0, dep.1);
    for app in &apps {
        println!("app {} ({})", app.0, app.1);
    }
    let mut inputs = vec![input(1, &dep.1)];
    for (i, app) in apps.iter().enumerate() {
        inputs.push(input((i + 2) as u64, &app.1));
    }
    let trees = RobotTrees {
        base: cands.to_vec(),
        commits: BTreeMap::new(),
    };
    let cfg = config();
    let records: Vec<PrRecord> = inputs
        .iter()
        .filter_map(|p| pr_record(p, &trees, &installed, &cfg))
        .collect();
    assert!(records.len() >= 3, "records {}", records.len());
    let index = overlap_index(&records, &cfg);
    let order = merge_order::plan(&records, &index, &[]);
    let again = merge_order::plan(&records, &index, &[]);
    assert_eq!(order, again, "two runs differed");
    let union = assert_once(&records, &order);
    let chose = std::iter::once(dep.0)
        .chain(apps.into_iter().map(|a| a.0))
        .collect::<Vec<_>>()
        .join(" ");
    (chose, union, order.steps.len())
}

fn assert_once(records: &[PrRecord], order: &merge_order::MergeOrder) -> usize {
    let union: BTreeSet<&str> = records
        .iter()
        .flat_map(|r| r.to_build.iter().map(String::as_str))
        .collect();
    let mut seen = BTreeSet::new();
    for step in &order.steps {
        for module in &step.builds {
            assert!(seen.insert(module.as_str()), "twice: {module}");
        }
    }
    assert_eq!(seen.len(), union.len());
    assert_eq!(seen, union);
    union.len()
}

fn installed_from_roots(cands: &[Candidate]) -> BTreeSet<String> {
    let mut roots = Vec::new();
    if let Some(c) = pick(cands, "GCCcore", Some("15.2.0")) {
        println!("root {}", module_name(&ModuleKey::of(c)));
        roots.push(ModuleKey::of(c));
    }
    if let Some(c) = pick(cands, "zlib", None) {
        println!("root {}", module_name(&ModuleKey::of(c)));
        roots.push(ModuleKey::of(c));
    }
    assert!(
        !roots.is_empty(),
        "neither GCCcore-15.2.0 nor zlib is in the tree"
    );
    let closed = closure_against_installed(cands, &roots, Choice::Newest, &|_| false);
    let mut installed: BTreeSet<String> = closed.to_build.iter().map(module_name).collect();
    installed.extend(roots.iter().map(module_name));
    installed
}

type Named = (String, String);
type Row = (String, String, BTreeSet<String>);

fn closures(cands: &[Candidate], installed: &BTreeSet<String>) -> Vec<Row> {
    let mut pool: Vec<&Candidate> = cands
        .iter()
        .filter(|c| c.toolchain.name == "GCCcore" && c.toolchain.version == "15.2.0")
        .filter(|c| !installed.contains(&module_name(&ModuleKey::of(c))))
        .filter(|c| !c.easyconfig_path.is_empty())
        .collect();
    pool.sort_by_key(|c| (c.name.clone(), c.version.clone(), c.easyconfig_path.clone()));
    let mut rows = Vec::new();
    for c in pool {
        let key = ModuleKey::of(c);
        let closed =
            closure_against_installed(cands, std::slice::from_ref(&key), Choice::Newest, &|k| {
                installed.contains(&module_name(k))
            });
        let builds: BTreeSet<String> = closed.to_build.iter().map(module_name).collect();
        if (2..=8).contains(&builds.len()) {
            let file = Path::new(&c.easyconfig_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            rows.push((module_name(&key), file, builds));
        }
    }
    rows
}

/// Dep D whose build set contains another chosen module, plus two modules that reach D.
fn chain(rows: &[Row]) -> Option<(Named, Vec<Named>)> {
    for (d_mod, d_file, d_builds) in rows {
        let reaches_other = d_builds
            .iter()
            .any(|m| m != d_mod && rows.iter().any(|r| &r.0 == m));
        if !reaches_other {
            continue;
        }
        let apps: Vec<(String, String)> = rows
            .iter()
            .filter(|r| &r.0 != d_mod && r.2.contains(d_mod))
            .take(2)
            .map(|r| (r.0.clone(), r.1.clone()))
            .collect();
        if apps.len() == 2 {
            return Some(((d_mod.clone(), d_file.clone()), apps));
        }
    }
    None
}

fn pick<'a>(cands: &'a [Candidate], name: &str, version: Option<&str>) -> Option<&'a Candidate> {
    cands
        .iter()
        .filter(|c| c.name == name && version.is_none_or(|v| c.version == v))
        .min_by_key(|c| {
            let rank = if c.toolchain.name == "GCCcore" && c.toolchain.version == "15.2.0" {
                0
            } else {
                1
            };
            (rank, c.toolchain.label(), c.easyconfig_path.clone())
        })
}

fn input(n: u64, file: &str) -> PrInput {
    let base = "easyconfigs:\n  - zlib-1.3.1-GCCcore-15.2.0.eb\n";
    serde_json::from_value(serde_json::json!({
        "number": n,
        "head_sha": format!("{n:040}"),
        "title": format!("PR {n}"),
        "easystacks": [{"path": STACK, "base": base, "head": format!("{base}  - {file}\n")}],
    }))
    .expect("input")
}

fn config() -> OverlapConfig {
    OverlapConfig {
        eessi_version: "2026.06".into(),
        installed_source: "x86_64/amd/zen4".into(),
        exclude_paths: Vec::new(),
        include_paths: Vec::new(),
        generated_at: "2026-10-22T00:00:00Z".into(),
        filter_deps: FilterDeps::empty(),
        easyblocks: None,
    }
}
