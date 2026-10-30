//! Ignored unless the pinned EasyBuild trees from `scripts/fetch-upstream-trees.sh`
//! are present.
//!
//! `EB_EASYCONFIGS` is the develop `easybuild/easyconfigs` directory.
//! `EB_EASYCONFIGS_541` is the v5.4.1 checkout; the tests join
//! `easybuild/easyconfigs`. `EB_STACK_ZEN4` is an optional directory of `prs/*.toml`
//! records; without it the zen4 tests build those records from the develop
//! tree. `EB_STACK_ZEN4_PRS` is the matching pull-request JSON.

use eb_stack::build_order::{closure_against_installed, module_name, Choice, ModuleKey};
use eb_stack::dep_filter::FilterDeps;
use eb_stack::domain::Candidate;
use eb_stack::pr_overlap::{
    file_revisions, overlap_index, pr_record, OverlapConfig, PrInput, PrRecord, RobotTrees,
};
use eb_stack::{merge_order, parse_easyconfig_tree};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

const STACK: &str = "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml";

#[test]
#[ignore]
fn real_trees_build_each_module_in_one_step() {
    let develop = std::env::var("EB_EASYCONFIGS").expect("EB_EASYCONFIGS");
    let release = easyconfigs_under("EB_EASYCONFIGS_541");
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
    zen4_order(false);
}

#[test]
#[ignore]
fn zen4_hunk_conflicts_from_fetched_revisions() {
    zen4_order(true);
}

fn easyconfigs_under(key: &str) -> String {
    let path = PathBuf::from(std::env::var(key).unwrap_or_else(|_| panic!("{key} is required")));
    let nested = path.join("easybuild/easyconfigs");
    if nested.is_dir() {
        nested.display().to_string()
    } else {
        path.display().to_string()
    }
}

fn easyconfigs_dir() -> PathBuf {
    let raw = std::env::var("EB_EASYCONFIGS")
        .or_else(|_| std::env::var("EB_EASYCONFIGS_DEVELOP"))
        .expect("EB_EASYCONFIGS or EB_EASYCONFIGS_DEVELOP");
    let path = PathBuf::from(raw);
    let nested = path.join("easybuild/easyconfigs");
    if nested.is_dir() {
        nested
    } else {
        path
    }
}

/// Records and pull-request JSON built from the develop easyconfigs tree,
/// so the zen4 tests run without a saved 2026-10-22 dump.
fn synthesize_zen4() -> &'static (tempfile::TempDir, tempfile::TempDir) {
    static CELL: OnceLock<(tempfile::TempDir, tempfile::TempDir)> = OnceLock::new();
    CELL.get_or_init(|| {
        let parsed = parse_easyconfig_tree(&easyconfigs_dir()).expect("parse develop tree");
        let installed = installed_from_roots(&parsed.candidates);
        let rows = closures(&parsed.candidates, &installed);
        let (dep, apps) = chain(&rows).expect("no chain in the develop tree");
        let mut inputs = vec![input(1, &dep.1)];
        for (i, app) in apps.iter().enumerate() {
            inputs.push(input((i + 2) as u64, &app.1));
        }
        let trees = RobotTrees {
            base: parsed.candidates,
            commits: BTreeMap::new(),
        };
        let cfg = config();
        let records = tempfile::tempdir().expect("records");
        let jsons = tempfile::tempdir().expect("json");
        std::fs::create_dir(records.path().join("prs")).expect("prs");
        for pr in &inputs {
            let record = pr_record(pr, &trees, &installed, &cfg).expect("record");
            std::fs::write(
                records
                    .path()
                    .join("prs")
                    .join(format!("{}.toml", record.pr)),
                eb_stack::pr_overlap::record_to_toml(&record).expect("toml"),
            )
            .expect("write record");
            std::fs::write(
                jsons.path().join(format!("{}.json", pr.number)),
                serde_json::to_string_pretty(pr).expect("json"),
            )
            .expect("write json");
        }
        (records, jsons)
    })
}

fn zen4_location() -> (PathBuf, Option<PathBuf>) {
    if let Ok(dir) = std::env::var("EB_STACK_ZEN4") {
        return (
            PathBuf::from(dir),
            std::env::var("EB_STACK_ZEN4_PRS").ok().map(PathBuf::from),
        );
    }
    let (records, jsons) = synthesize_zen4();
    (
        records.path().to_path_buf(),
        Some(jsons.path().to_path_buf()),
    )
}

fn zen4_order(with_revisions: bool) {
    let (dir, generated_prs) = zen4_location();
    let dir = dir.display().to_string();
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
    if with_revisions {
        let fetched = std::env::var("EB_STACK_ZEN4_PRS").unwrap_or_else(|_| {
            generated_prs
                .expect("synthesized pull-request JSON")
                .display()
                .to_string()
        });
        let mut inputs = Vec::new();
        for entry in std::fs::read_dir(&fetched).expect("fetched") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("json");
            inputs.push(
                serde_json::from_str::<PrInput>(&text)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display())),
            );
        }
        for record in &mut records {
            let Some(pr) = inputs.iter().find(|pr| pr.number == record.pr) else {
                panic!("no fetch for {}", record.pr);
            };
            record.revisions = file_revisions(pr, &config);
            assert!(
                record.revisions.iter().any(|rev| rev.kind == "easystack"),
                "pr {} stored no easystack revision",
                record.pr
            );
            println!(
                "pr {} record_head {} fetch_head {} revisions {}",
                record.pr,
                record.head_sha,
                pr.head_sha,
                record.revisions.len()
            );
        }
    }
    let index = overlap_index(&records, &config);
    let mut order = merge_order::plan(&records, &index, &[]);
    if with_revisions {
        order.conflicts = merge_order::conflicts_from_records(&records);
    }
    let again = merge_order::plan(&records, &index, &[]);
    assert_eq!(order.steps, again.steps);
    assert_once(&records, &order);
    println!(
        "{}",
        merge_order::render_section(&order, "https://github.com/EESSI/software-layer")
    );
    for conflict in &order.conflicts {
        let positions: Vec<(usize, u64)> = order
            .steps
            .iter()
            .filter(|step| conflict.prs.contains(&step.pr))
            .map(|step| (step.position, step.pr))
            .collect();
        println!(
            "conflict {} {} prs {:?} positions {:?}",
            conflict.kind, conflict.easystack, conflict.prs, positions
        );
    }
    if with_revisions {
        if let Ok(dest) = std::env::var("EB_STACK_ZEN4_OUT") {
            let prs = Path::new(&dest).join("prs");
            std::fs::create_dir_all(&prs).expect("out");
            for record in &records {
                std::fs::write(
                    prs.join(format!("{}.toml", record.pr)),
                    eb_stack::pr_overlap::record_to_toml(record).expect("toml"),
                )
                .expect("write");
            }
            std::fs::write(
                Path::new(&dest).join("merge-order.toml"),
                eb_stack::pr_overlap::to_toml(&order).expect("order"),
            )
            .expect("order");
        }
    }
    println!(
        "zen4 prs={} steps={} union={} conflicts={}",
        records.len(),
        order.steps.len(),
        order.steps.iter().map(|s| s.builds.len()).sum::<usize>(),
        order.conflicts.len()
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
        build_scope: eb_stack::pr_overlap::BuildScope::Added,
    }
}
