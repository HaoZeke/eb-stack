//! Group new software-layer roots into pull requests whose builds do not overlap.
//!
//! A root's new-module set is what `pr-overlap` computes for one easystack
//! line: the root's closure over the robot path, every pinned version kept,
//! minus what the stack already installs. Two roots whose sets meet must ship
//! together, or two pull requests would build the same module, which EESSI
//! reviewers reject. A root whose set meets an open pull request's set waits
//! for that pull request, as conda-forge's migrator waits for a predecessor
//! that is not built yet.
//!
//! The roots are the vertices of a conflict graph with an edge wherever two
//! sets intersect; each connected component must be one pull request or part
//! of one. Components are packed into batches of at most `size` roots by
//! first-fit in decreasing size with ties broken by name, so the same inputs
//! give the same batches. A component larger than `size` is one batch of its
//! own. Batches have no build edges between them: a module two batches both
//! needed would have put their roots in one component.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// One candidate root and what it would build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootSet {
    /// Easyconfig file name of the root.
    pub root: String,
    /// Modules the root's easystack line would build, the root's own included.
    pub to_build: BTreeSet<String>,
    /// Requirements no easyconfig on the robot path satisfies.
    pub unresolved: Vec<String>,
}

/// An open pull request and what it would build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSet {
    /// Pull request number.
    pub number: u64,
    /// Modules it would build.
    pub to_build: BTreeSet<String>,
}

/// One planned pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Batch {
    /// 1-based position in the plan.
    pub index: usize,
    /// Root easyconfigs, sorted.
    pub roots: Vec<String>,
    /// Every module the batch builds, sorted.
    pub modules: Vec<String>,
}

/// A root that waits for open pull requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wait {
    /// Root easyconfig.
    pub root: String,
    /// Open pull requests that build part of the root's set.
    pub prs: Vec<u64>,
    /// The modules shared with them.
    pub shared: Vec<String>,
}

/// The whole plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatchPlan {
    /// The pull requests to open, in order.
    pub batches: Vec<Batch>,
    /// Roots that wait for open pull requests.
    pub waits: Vec<Wait>,
    /// Roots with requirements nothing on the robot path provides.
    pub unresolved: BTreeMap<String, Vec<String>>,
    /// Roots the stack already installs.
    pub deployed: Vec<String>,
}

/// Plan batches of at most `size` roots. `size` 0 is read as 1.
pub fn plan(roots: &[RootSet], open: &[OpenSet], size: usize) -> BatchPlan {
    let size = size.max(1);
    let mut sorted: Vec<&RootSet> = roots.iter().collect();
    sorted.sort_by(|a, b| a.root.cmp(&b.root));
    sorted.dedup_by(|a, b| a.root == b.root);

    let mut waits = Vec::new();
    let mut unresolved = BTreeMap::new();
    let mut deployed = Vec::new();
    let mut live: Vec<&RootSet> = Vec::new();
    for r in sorted {
        if !r.unresolved.is_empty() {
            unresolved.insert(r.root.clone(), r.unresolved.clone());
            continue;
        }
        if r.to_build.is_empty() {
            deployed.push(r.root.clone());
            continue;
        }
        let mut prs = Vec::new();
        let mut shared = BTreeSet::new();
        for o in open {
            let meet: Vec<&String> = r.to_build.intersection(&o.to_build).collect();
            if !meet.is_empty() {
                prs.push(o.number);
                shared.extend(meet.into_iter().cloned());
            }
        }
        if prs.is_empty() {
            live.push(r);
        } else {
            prs.sort_unstable();
            prs.dedup();
            waits.push(Wait {
                root: r.root.clone(),
                prs,
                shared: shared.into_iter().collect(),
            });
        }
    }

    // Union roots that share a module.
    let mut parent: Vec<usize> = (0..live.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, r) in live.iter().enumerate() {
        for m in &r.to_build {
            match owner.get(m.as_str()) {
                Some(&j) => {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a.max(b)] = a.min(b);
                    }
                }
                None => {
                    owner.insert(m, i);
                }
            }
        }
    }
    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..live.len() {
        let root = find(&mut parent, i);
        components.entry(root).or_default().push(i);
    }
    let mut components: Vec<Vec<usize>> = components.into_values().collect();
    components.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| live[a[0]].root.cmp(&live[b[0]].root))
    });

    let mut bins: Vec<Vec<usize>> = Vec::new();
    for comp in components {
        if comp.len() >= size {
            bins.push(comp);
            continue;
        }
        match bins
            .iter_mut()
            .find(|bin| bin.len() < size && bin.len() + comp.len() <= size)
        {
            Some(bin) => bin.extend(comp),
            None => bins.push(comp),
        }
    }

    let batches = bins
        .into_iter()
        .enumerate()
        .map(|(n, bin)| {
            let mut roots: Vec<String> = bin.iter().map(|&i| live[i].root.clone()).collect();
            roots.sort();
            let modules: BTreeSet<String> = bin
                .iter()
                .flat_map(|&i| live[i].to_build.iter().cloned())
                .collect();
            Batch {
                index: n + 1,
                roots,
                modules: modules.into_iter().collect(),
            }
        })
        .collect();
    BatchPlan {
        batches,
        waits,
        unresolved,
        deployed,
    }
}

/// A short file-system name for a batch: its index and first root's name.
pub fn batch_slug(batch: &Batch) -> String {
    let first = batch.roots.first().map(String::as_str).unwrap_or("empty");
    let name: String = first
        .split('-')
        .next()
        .unwrap_or(first)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("batch-{:02}-{name}", batch.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str, mods: &[&str]) -> RootSet {
        RootSet {
            root: name.into(),
            to_build: mods.iter().map(|m| m.to_string()).collect(),
            unresolved: Vec::new(),
        }
    }

    #[test]
    fn sharing_roots_ship_together_and_others_pack_three_per_batch() {
        let roots = vec![
            root("A.eb", &["A/1", "lib/1"]),
            root("B.eb", &["B/1", "lib/1"]),
            root("C.eb", &["C/1"]),
            root("D.eb", &["D/1"]),
            root("E.eb", &["E/1"]),
            root("F.eb", &["F/1"]),
        ];
        let p = plan(&roots, &[], 3);
        let groups: Vec<Vec<String>> = p.batches.iter().map(|b| b.roots.clone()).collect();
        assert_eq!(
            groups,
            vec![
                vec!["A.eb".to_string(), "B.eb".into(), "C.eb".into()],
                vec!["D.eb".to_string(), "E.eb".into(), "F.eb".into()],
            ]
        );
        assert!(p.batches[0].modules.contains(&"lib/1".to_string()));
        // No module is built by two batches.
        let mut seen = BTreeSet::new();
        for b in &p.batches {
            for m in &b.modules {
                assert!(seen.insert(m.clone()), "{m} in two batches");
            }
        }
    }

    #[test]
    fn a_root_that_meets_an_open_pr_waits_and_others_proceed() {
        let roots = vec![
            root("dask.eb", &["dask/1", "bokeh/3"]),
            root("tmux.eb", &["tmux/3"]),
        ];
        let open = vec![OpenSet {
            number: 1776,
            to_build: ["bokeh/3".to_string()].into(),
        }];
        let p = plan(&roots, &open, 3);
        assert_eq!(p.waits.len(), 1);
        assert_eq!(p.waits[0].prs, vec![1776]);
        assert_eq!(p.waits[0].shared, vec!["bokeh/3".to_string()]);
        assert_eq!(p.batches.len(), 1);
        assert_eq!(p.batches[0].roots, vec!["tmux.eb".to_string()]);
    }

    #[test]
    fn deployed_unresolved_and_oversized_components_are_reported() {
        let mut broken = root("X.eb", &["X/1"]);
        broken.unresolved = vec!["Y >= 2".into()];
        let roots = vec![
            root("Done.eb", &[]),
            broken,
            root("P.eb", &["P/1", "core/1"]),
            root("Q.eb", &["Q/1", "core/1"]),
            root("R.eb", &["R/1", "core/1"]),
            root("S.eb", &["S/1", "core/1"]),
        ];
        let p = plan(&roots, &[], 3);
        assert_eq!(p.deployed, vec!["Done.eb".to_string()]);
        assert!(p.unresolved.contains_key("X.eb"));
        assert_eq!(p.batches.len(), 1);
        assert_eq!(p.batches[0].roots.len(), 4);
    }

    #[test]
    fn the_plan_does_not_depend_on_input_order() {
        let mut roots = vec![
            root("Z.eb", &["Z/1"]),
            root("A.eb", &["A/1", "x/1"]),
            root("M.eb", &["M/1", "x/1"]),
            root("B.eb", &["B/1"]),
        ];
        let forward = plan(&roots, &[], 3);
        roots.reverse();
        assert_eq!(plan(&roots, &[], 3), forward);
    }

    #[test]
    fn slug_names_the_first_root() {
        let b = Batch {
            index: 2,
            roots: vec!["BLAST+-2.17.0-gompi-2026.1.eb".into()],
            modules: vec![],
        };
        assert_eq!(batch_slug(&b), "batch-02-blast_");
    }
}
