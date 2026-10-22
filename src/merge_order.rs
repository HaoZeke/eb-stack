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
use crate::pr_overlap::{OverlapIndex, PrInput, PrRecord};
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

/// Order `records`. `merged` pull requests are already installed: their
/// `to_build` modules seed the residual, and they are not steps.
/// `index` supplies the timestamp and the installed-list label.
pub fn plan(records: &[PrRecord], index: &OverlapIndex, merged: &[u64]) -> MergeOrder {
    let mut merged_ids: Vec<u64> = merged.to_vec();
    merged_ids.sort_unstable();
    merged_ids.dedup();
    let merged_set: BTreeSet<u64> = merged_ids.iter().copied().collect();
    let mut installed: BTreeSet<String> = BTreeSet::new();
    for record in records.iter().filter(|r| merged_set.contains(&r.pr)) {
        installed.extend(record.to_build.iter().cloned());
    }
    let active: Vec<&PrRecord> = records.iter().filter(|r| !merged_set.contains(&r.pr)).collect();
    let asked: BTreeMap<u64, BTreeSet<String>> = active.iter().map(|record| {
        let set = record.entries.iter().filter_map(|e| entry_module(&e.easyconfig)).collect();
        (record.pr, set)
    }).collect();
    let builds: BTreeMap<u64, BTreeSet<String>> = active
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
        merged: merged_ids,
        steps,
        cycles,
        conflicts: Vec::new(),
    }
}

/// The first `--merged` number that is not one of `inputs`, if any.
pub fn unknown_merged(inputs: &[u64], merged: &[u64]) -> Option<u64> {
    let known: BTreeSet<u64> = inputs.iter().copied().collect();
    merged.iter().copied().find(|n| !known.contains(n))
}

/// Collisions on one easystack. The insertion point is the 0-based index of
/// the base line before the first added line, or -1 when the addition starts
/// at the top of the file. Identical added lines are `same-entry`.
pub fn easystack_conflicts(prs: &[PrInput]) -> Vec<EasystackConflict> {
    let edits: Vec<(u64, String, String, String)> = prs
        .iter()
        .flat_map(|pr| {
            pr.easystacks.iter().map(|change| {
                (
                    pr.number,
                    change.path.clone(),
                    change.base.clone().unwrap_or_default(),
                    change.head.clone().unwrap_or_default(),
                )
            })
        })
        .collect();
    conflicts_from_edits(&edits)
}

/// Same collisions, read from the text stored on each record.
pub fn conflicts_from_records(records: &[PrRecord]) -> Vec<EasystackConflict> {
    let edits: Vec<(u64, String, String, String)> = records
        .iter()
        .flat_map(|record| {
            record.revisions.iter().filter(|rev| rev.kind == "easystack").map(|rev| {
                (
                    record.pr,
                    rev.path.clone(),
                    rev.base.clone().unwrap_or_default(),
                    rev.head.clone().unwrap_or_default(),
                )
            })
        })
        .collect();
    conflicts_from_edits(&edits)
}

fn conflicts_from_edits(edits: &[(u64, String, String, String)]) -> Vec<EasystackConflict> {
    let mut by_path: BTreeMap<String, Vec<(u64, i64, String)>> = BTreeMap::new();
    for (pr, path, base, head) in edits {
        let (point, added) = first_insertion(base, head);
        if !added.is_empty() {
            by_path.entry(path.clone()).or_default().push((*pr, point, added));
        }
    }
    let mut out = Vec::new();
    for (path, rows) in &by_path {
        if rows.iter().map(|row| row.0).collect::<BTreeSet<_>>().len() < 2 {
            continue;
        }
        let mut by_point: BTreeMap<i64, BTreeSet<u64>> = BTreeMap::new();
        let mut by_text: BTreeMap<&str, BTreeSet<u64>> = BTreeMap::new();
        for (pr, point, added) in rows {
            by_point.entry(*point).or_default().insert(*pr);
            by_text.entry(added.as_str()).or_default().insert(*pr);
        }
        for group in by_point.values().filter(|g| g.len() >= 2) {
            out.push(EasystackConflict {
                easystack: path.clone(),
                prs: group.iter().copied().collect(),
                kind: "same-insertion".into(),
            });
        }
        for group in by_text.values().filter(|g| g.len() >= 2) {
            out.push(EasystackConflict {
                easystack: path.clone(),
                prs: group.iter().copied().collect(),
                kind: "same-entry".into(),
            });
        }
    }
    out.sort_by(|a, b| (&a.easystack, &a.kind, &a.prs).cmp(&(&b.easystack, &b.kind, &b.prs)));
    out
}

fn first_insertion(base: &str, head: &str) -> (i64, String) {
    let b: Vec<&str> = base.lines().collect();
    let h: Vec<&str> = head.lines().collect();
    let (n, m) = (b.len(), h.len());
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = if b[i - 1] == h[j - 1] {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }
    let (mut i, mut j) = (n, m);
    let mut added = Vec::new();
    let mut point = -1i64;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && b[i - 1] == h[j - 1] && dp[i][j] == dp[i - 1][j - 1] + 1 {
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            point = i as i64 - 1;
            added.push(h[j - 1]);
            j -= 1;
        } else {
            i -= 1;
        }
    }
    added.reverse();
    (point, added.join("\n"))
}

/// Markdown table appended to `overlap.md` when `--merge-order` is set.
pub fn render_section(order: &MergeOrder, repo_url: &str) -> String {
    let link = |n: u64| -> String {
        if repo_url.is_empty() {
            format!("#{n}")
        } else {
            format!("[#{n}]({}/pull/{n})", repo_url.trim_end_matches('/'))
        }
    };
    let mut out = String::from(
        "## Merge order\n\n| Position | PR | Builds | Saves | After |\n|---|---|---|---|---|\n",
    );
    for step in &order.steps {
        let saves = step
            .saves
            .iter()
            .map(|saved| {
                let mods = saved.modules.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ");
                format!("{} ({mods})", link(saved.pr))
            })
            .collect::<Vec<_>>()
            .join("; ");
        let after = step.after.iter().copied().map(link).collect::<Vec<_>>().join(", ");
        out.push_str(&format!(
            "| {} | {} | {} | {saves} | {after} |\n",
            step.position,
            link(step.pr),
            step.builds.len()
        ));
    }
    out.push('\n');
    if !order.merged.is_empty() {
        let list = order.merged.iter().copied().map(link).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("Treated as merged: {list}.\n\n"));
    }
    if !order.cycles.is_empty() {
        for cycle in &order.cycles {
            let list = cycle.iter().copied().map(link).collect::<Vec<_>>().join(", ");
            out.push_str(&format!("Cycle: {list}.\n"));
        }
        out.push('\n');
    }
    if !order.conflicts.is_empty() {
        out.push_str("| Easystack | Pull requests | Kind |\n|---|---|---|\n");
        for conflict in &order.conflicts {
            let prs = conflict.prs.iter().copied().map(link).collect::<Vec<_>>().join(", ");
            out.push_str(&format!("| `{}` | {prs} | {} |\n", conflict.easystack, conflict.kind));
        }
        out.push('\n');
    }
    out
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
            revisions: Vec::new(),
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
        }), &[])
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

    #[test]
    fn merged_pull_requests_shrink_the_residual_and_are_not_steps() {
        let lib = "Lib/1.0-GCCcore-15.2.0";
        let mid = "Mid/1.0-GCCcore-15.2.0";
        let app = "App/2.0-GCCcore-15.2.0";
        let records = [
            rec(1, &[lib], &["Lib-1.0-GCCcore-15.2.0.eb"]),
            rec(2, &[mid, lib], &["Mid-1.0-GCCcore-15.2.0.eb"]),
            rec(3, &[app, mid, lib], &["App-2.0-GCCcore-15.2.0.eb"]),
        ];
        let index = overlap_index(&records, &OverlapConfig {
            eessi_version: "2026.06".into(),
            installed_source: "x86_64/amd/zen4".into(),
            exclude_paths: Vec::new(),
            include_paths: Vec::new(),
            generated_at: "2026-10-22T00:00:00Z".into(),
            filter_deps: FilterDeps::empty(),
            easyblocks: None,
        });
        let order = plan(&records, &index, &[1]);
        assert_eq!(order.merged, [1]);
        assert_eq!(builds(&order), vec![(2, vec![mid.into()]), (3, vec![app.into()])]);
        assert_eq!(unknown_merged(&[1, 2, 3], &[9]), Some(9));
        assert_eq!(unknown_merged(&[1, 2], &[]), None);
    }

    #[test]
    fn same_insertion_point_and_same_added_text_are_conflicts() {
        use crate::pr_overlap::EasystackChange;
        let pr = |n: u64, head: &str| PrInput {
            number: n,
            head_sha: "h".into(),
            title: None,
            files: Vec::new(),
            easystacks: vec![EasystackChange {
                path: "s.yml".into(),
                base: Some("a\nb\nc\n".into()),
                head: Some(head.into()),
                base_blob: None,
                head_blob: None,
            }],
        };
        let found = easystack_conflicts(&[
            pr(1, "a\nb\nX\nc\n"),
            pr(2, "a\nb\nY\nc\n"),
            pr(3, "a\nb\nX\nc\n"),
        ]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, "same-entry");
        assert_eq!(found[0].prs, [1, 3]);
        assert_eq!(found[1].kind, "same-insertion");
        assert_eq!(found[1].prs, [1, 2, 3]);
        let top = easystack_conflicts(&[pr(4, "Z\na\nb\nc\n"), pr(5, "Z\na\nb\nc\n")]);
        assert!(top.iter().any(|c| c.kind == "same-insertion" && c.prs == [4, 5]));
        let mut stored = [
            rec(1, &[], &[]),
            rec(2, &[], &[]),
            rec(3, &[], &[]),
        ];
        for (record, head) in stored.iter_mut().zip(["a\nb\nX\nc\n", "a\nb\nY\nc\n", "a\nb\nX\nc\n"]) {
            record.revisions.push(crate::pr_overlap::FileRevision {
                path: "s.yml".into(),
                kind: "easystack".into(),
                base_blob: Some("b".into()),
                head_blob: Some("h".into()),
                base: Some("a\nb\nc\n".into()),
                head: Some(head.into()),
            });
        }
        assert_eq!(conflicts_from_records(&stored), found);
        let section = render_section(&order(&[rec(10, &["D/1"], &["D-1.eb"])]), "");
        assert!(section.contains("| Position | PR | Builds | Saves | After |"));
        assert!(section.contains("| 1 | #10 | 1 |  |  |"));
    }
}
