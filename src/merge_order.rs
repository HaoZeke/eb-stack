#![allow(missing_docs, reason = "merge-order.toml fields are named in formats.org")]

//! Order open pull requests so a shared module is built once.
//!
//! 1. Edge A -> B when a module in B's `to_build` is an easystack entry A adds
//!    (A asks for it by name; B only reaches it). Edges both ways form a cycle.
//! 2. Among pull requests with no remaining incoming edge, pick the one whose
//!    residual build set is smallest, then the lowest pull request number.
//! 3. After picking, add its residual builds to the installed set and recompute
//!    residuals for the rest. `saves` is that difference.
//! 4. A strongly connected component is reported in `cycles`. Edges inside it
//!    are dropped, and rule 2 orders its pull requests with every other pull
//!    request that has no remaining incoming edge.

use crate::jenkins::package_name;
use crate::pr_overlap::{OverlapIndex, PrRecord};
use petgraph::algo::tarjan_scc;
use petgraph::graph::DiGraph;
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Schema of [`MergeOrder`].
pub const MERGE_ORDER_SCHEMA_VERSION: u32 = 1;

/// One merge order. `merged` and `conflicts` stay empty until those flags run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOrder {
    pub schema_version: u32,
    pub generated_at: String,
    pub installed_source: String,
    pub merged: Vec<u64>,
    pub steps: Vec<MergeStep>,
    pub cycles: Vec<Vec<u64>>,
    pub conflicts: Vec<EasystackConflict>,
}

/// One pull request after every earlier step has merged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeStep {
    /// 1-based.
    pub position: usize,
    pub pr: u64,
    /// Direct predecessors in the binding graph, ascending.
    pub after: Vec<u64>,
    /// Residual `to_build`, sorted.
    pub builds: Vec<String>,
    pub saves: Vec<Saved>,
    pub reason: String,
}

/// Modules one later pull request no longer builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub pr: u64,
    pub modules: Vec<String>,
}

/// Easystack edits that collide. `kind` is `same-insertion` or `same-entry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EasystackConflict {
    pub easystack: String,
    pub prs: Vec<u64>,
    pub kind: String,
}

/// `Lib-1.0-GCCcore-15.2.0.eb` asks for `Lib/1.0-GCCcore-15.2.0`.
pub fn entry_module(file: &str) -> Option<String> {
    let base = file.rsplit('/').next().unwrap_or(file);
    let name = package_name(base)?;
    let stem = base.strip_suffix(".eb")?;
    let rest = stem.get(name.len() + 1..).filter(|rest| !rest.is_empty())?;
    Some(format!("{name}/{rest}"))
}

/// Order `records`. `index` supplies the timestamp and the installed-list label.
pub fn plan(records: &[PrRecord], index: &OverlapIndex) -> MergeOrder {
    let asked: BTreeMap<u64, BTreeSet<String>> = records.iter().map(|record| {
        let set = record.entries.iter().filter_map(|e| entry_module(&e.easyconfig)).collect();
        (record.pr, set)
    }).collect();
    let builds: BTreeMap<u64, BTreeSet<String>> = records
        .iter()
        .map(|record| (record.pr, record.to_build.iter().cloned().collect()))
        .collect();
    let mut graph = DiGraph::<u64, ()>::new();
    let mut node = BTreeMap::new();
    for pr in asked.keys() {
        node.insert(*pr, graph.add_node(*pr));
    }
    for (&from, from_asked) in &asked {
        for (&to, to_builds) in &builds {
            let reaches = from != to
                && from_asked.iter().any(|m| to_builds.contains(m) && !asked[&to].contains(m));
            if reaches {
                graph.add_edge(node[&from], node[&to], ());
            }
        }
    }
    let mut cycles = Vec::new();
    let mut which = BTreeMap::new();
    for component in tarjan_scc(&graph) {
        if component.len() < 2 {
            continue;
        }
        let mut prs: Vec<u64> = component.into_iter().map(|n| graph[n]).collect();
        prs.sort_unstable();
        let id = cycles.len();
        for pr in &prs {
            which.insert(*pr, id);
        }
        cycles.push(prs);
    }
    cycles.sort();
    let mut open: BTreeMap<u64, BTreeSet<u64>> =
        graph.node_weights().map(|pr| (*pr, BTreeSet::new())).collect();
    for edge in graph.edge_references() {
        let (from, to) = (graph[edge.source()], graph[edge.target()]);
        let inside = which.get(&from).is_some_and(|id| which.get(&to) == Some(id));
        if !inside {
            open.entry(to).or_default().insert(from);
        }
    }
    let original = open.clone();
    let mut installed: BTreeSet<String> = BTreeSet::new();
    let mut remaining: BTreeSet<u64> = open.keys().copied().collect();
    let mut steps = Vec::new();
    while !remaining.is_empty() {
        let mut ready: Vec<u64> = remaining.iter().copied().filter(|pr| open[pr].is_empty()).collect();
        if ready.is_empty() {
            ready.extend(remaining.iter().copied());
        }
        let pick = ready.into_iter().min_by_key(|pr| (builds[pr].difference(&installed).count(), *pr)).expect("ready");
        let residual: Vec<String> = builds[&pick].difference(&installed).cloned().collect();
        let built: BTreeSet<&str> = residual.iter().map(String::as_str).collect();
        let mut saves = Vec::new();
        for pr in &remaining {
            if *pr == pick {
                continue;
            }
            let modules: Vec<String> = builds[pr]
                .difference(&installed)
                .filter(|module| built.contains(module.as_str()))
                .cloned()
                .collect();
            if !modules.is_empty() {
                saves.push(Saved { pr: *pr, modules });
            }
        }
        let n = residual.len();
        let reason = if which.contains_key(&pick) {
            format!("smallest residual build set ({n}) inside a cycle")
        } else {
            format!("smallest residual build set ({n})")
        };
        remaining.remove(&pick);
        for preds in open.values_mut() {
            preds.remove(&pick);
        }
        installed.extend(residual.iter().cloned());
        steps.push(MergeStep {
            position: steps.len() + 1,
            pr: pick,
            after: original[&pick].iter().copied().collect(),
            builds: residual,
            saves,
            reason,
        });
    }
    MergeOrder {
        schema_version: MERGE_ORDER_SCHEMA_VERSION,
        generated_at: index.generated_at.clone(),
        installed_source: index.installed_source.clone(),
        merged: Vec::new(),
        steps,
        cycles,
        conflicts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dep_filter::FilterDeps;
    use crate::pr_overlap::{overlap_index, OverlapConfig, RecordEntry};

    fn rec(pr: u64, builds: &[&str], asked: &[&str]) -> PrRecord {
        PrRecord {
            schema_version: 1,
            pr,
            head_sha: "h".into(),
            title: None,
            generated_at: "t".into(),
            eessi_version: "2026.06".into(),
            installed_source: "x86_64/amd/zen4".into(),
            easystack_files: vec!["s.yml".into()],
            to_build: builds.iter().map(|s| (*s).to_string()).collect(),
            already_installed: Vec::new(),
            unresolved: Vec::new(),
            notes: Vec::new(),
            entries: asked.iter().map(|file| RecordEntry {
                easystack: "s.yml".into(),
                easyconfig: (*file).to_string(),
                from_commit: None,
                eb_version: None,
            }).collect(),
        }
    }

    fn order(records: &[PrRecord]) -> MergeOrder {
        plan(records, &overlap_index(records, &OverlapConfig {
            eessi_version: "2026.06".into(),
            installed_source: "x86_64/amd/zen4".into(),
            exclude_paths: Vec::new(),
            include_paths: Vec::new(),
            generated_at: "2026-10-22T00:00:00Z".into(),
            filter_deps: FilterDeps::empty(),
            easyblocks: None,
        }))
    }

    fn builds(order: &MergeOrder) -> Vec<(u64, Vec<String>)> {
        order.steps.iter().map(|s| (s.pr, s.builds.clone())).collect()
    }

    #[test]
    fn chain_diamond_cycle_and_tie_break() {
        let lib = "Lib/1.0-GCCcore-15.2.0";
        let mid = "Mid/1.0-GCCcore-15.2.0";
        let app = "App/2.0-GCCcore-15.2.0";
        assert_eq!(entry_module("Lib-1.0-GCCcore-15.2.0.eb").as_deref(), Some(lib));
        let chain = order(&[
            rec(1, &[lib], &["Lib-1.0-GCCcore-15.2.0.eb"]),
            rec(2, &[mid, lib], &["Mid-1.0-GCCcore-15.2.0.eb"]),
            rec(3, &[app, mid, lib], &["App-2.0-GCCcore-15.2.0.eb"]),
        ]);
        assert!(chain.cycles.is_empty());
        assert_eq!(builds(&chain), vec![
            (1, vec![lib.into()]),
            (2, vec![mid.into()]),
            (3, vec![app.into()]),
        ]);
        assert_eq!(chain.steps[0].saves[0].modules, [lib]);
        assert_eq!(chain.steps[1].after, [1]);
        assert_eq!(chain.steps[1].saves[0].modules, [mid]);
        assert_eq!(chain.steps[2].after, [1, 2]);
        let once: usize = chain.steps.iter().map(|s| s.builds.len()).sum();
        assert_eq!(once, 3);

        let diamond = order(&[
            rec(1, &[lib], &["Lib-1.0-GCCcore-15.2.0.eb"]),
            rec(2, &[app, lib], &["App-2.0-GCCcore-15.2.0.eb"]),
            rec(3, &["B/1.0-GCCcore-15.2.0", lib], &["B-1.0-GCCcore-15.2.0.eb"]),
        ]);
        assert_eq!(diamond.steps.iter().map(|s| s.pr).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(diamond.steps[0].saves.iter().map(|s| s.pr).collect::<Vec<_>>(), [2, 3]);

        let cycle = order(&[
            rec(2, &[lib, mid], &["Mid-1.0-GCCcore-15.2.0.eb"]),
            rec(1, &[lib, mid], &["Lib-1.0-GCCcore-15.2.0.eb"]),
        ]);
        assert_eq!(cycle.cycles, vec![vec![1, 2]]);
        assert_eq!(builds(&cycle), vec![(1, vec![lib.into(), mid.into()]), (2, vec![])]);
        assert!(cycle.steps[0].after.is_empty());
        assert!(cycle.steps[0].reason.contains("inside a cycle"));

        let tie = order(&[
            rec(30, &["A/1", "B/1"], &["A-1.eb", "B-1.eb"]),
            rec(20, &["C/1"], &["C-1.eb"]),
            rec(10, &["D/1"], &["D-1.eb"]),
        ]);
        assert_eq!(tie.steps.iter().map(|s| s.pr).collect::<Vec<_>>(), [10, 20, 30]);

        let shared = "Shared/1.0-GCCcore-15.2.0";
        let both = order(&[
            rec(8, &[shared, "Extra/1"], &["Shared-1.0-GCCcore-15.2.0.eb", "Extra-1.eb"]),
            rec(4, &[shared], &["Shared-1.0-GCCcore-15.2.0.eb"]),
        ]);
        assert_eq!(builds(&both), vec![(4, vec![shared.into()]), (8, vec!["Extra/1".into()])]);
        assert!(both.steps[0].after.is_empty(), "both ask for the module, so there is no edge");
    }
}
