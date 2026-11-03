//! Split one Jenkins build list into shards that can build at the same time.
//!
//! Two build-list lines interfere when the parts of their dependency closures
//! that are not installed yet meet: both would build the same module. Each
//! line's closure is computed the way `pr-overlap` computes a root's set, over
//! the robot path minus the installed modules.
//!
//! Lines are grouped first on root-level conflicts only: a line whose own
//! easyconfig is in another line's closure, or two lines naming the same
//! easyconfig, have to build in one shard, because the line carries its own
//! `eb` options and has to build that module itself. Any other module that the
//! closures of two or more groups share moves to a prelude that builds before
//! every shard. That keeps the groups as many and as small as the dependency
//! graph allows. The prelude needs no line of its own: a module in it is never
//! a line's root (that line's group would hold every closure that reaches it),
//! and its dependencies are in every closure it is in, so they are in the
//! prelude too.
//!
//! Groups are then packed into at most `shards` shards by longest processing
//! time first: heaviest group first, each onto the lightest shard, ties broken
//! by fewer groups and then by shard number. A module weighs what `--weights`
//! says it took to build, or the default weight.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// One non-comment line of a build list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardLine {
    /// 1-based line number in the input.
    pub number: usize,
    /// The line as written, trailing whitespace dropped.
    pub text: String,
    /// The first word, the easyconfig filename.
    pub file: String,
    /// Whether the line forces a rebuild (`--rebuild` or `--force`), so its
    /// own module counts as not installed.
    pub forced: bool,
}

/// Parse a build list. Blank lines and `#` comments are skipped; every other
/// line is kept, whatever its first word, so a line that names no easyconfig
/// is reported rather than dropped.
pub fn parse_lines(text: &str) -> Vec<ShardLine> {
    text.lines()
        .enumerate()
        .filter_map(|(i, raw)| {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let mut words = trimmed.split_whitespace();
            let file = words.next()?.to_string();
            let forced = words.any(|w| {
                let key = w.split('=').next().unwrap_or(w);
                key == "--rebuild" || key == "--force"
            });
            Some(ShardLine {
                number: i + 1,
                text: line.to_string(),
                file,
                forced,
            })
        })
        .collect()
}

/// What one line would build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineClosure {
    /// The line's own module, `Name/VersionFull`.
    pub root: String,
    /// Modules not installed yet, dependencies first, the root included
    /// unless it is installed.
    pub to_build: Vec<String>,
}

/// One packed shard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shard {
    /// Indices into the line list, in dependency order.
    pub lines: Vec<usize>,
    /// Modules the shard builds, sorted.
    pub modules: Vec<String>,
    /// Sum of the module weights.
    pub weight: u64,
}

/// The shard plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardPlan {
    /// Group of each line, numbered by the group's first line.
    pub group_of: Vec<usize>,
    /// Shard of each line, 0-based.
    pub shard_of: Vec<usize>,
    /// Modules two or more groups need, in `order`.
    pub prelude: Vec<String>,
    /// Sum of the prelude module weights.
    pub prelude_weight: u64,
    /// The shards, heaviest first.
    pub shards: Vec<Shard>,
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        // The smaller index is the representative, so group numbering follows
        // the input order.
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        parent[hi] = lo;
    }
}

/// Order `members` (line indices) so a line comes after every line whose root
/// its closure needs. Ties keep the input order; a cycle, which the robot path
/// should not contain, falls back to input order for what is left.
fn line_order(members: &[usize], closures: &[LineClosure]) -> Vec<usize> {
    let sets: BTreeMap<usize, BTreeSet<&str>> = members
        .iter()
        .map(|&i| (i, closures[i].to_build.iter().map(String::as_str).collect()))
        .collect();
    let needs = |b: usize, a: usize| {
        a != b
            && closures[a].root != closures[b].root
            && sets[&b].contains(closures[a].root.as_str())
    };
    let mut remaining: Vec<usize> = members.to_vec();
    remaining.sort_unstable();
    let mut out = Vec::with_capacity(remaining.len());
    while !remaining.is_empty() {
        let pick = remaining
            .iter()
            .position(|&b| !remaining.iter().any(|&a| needs(b, a)))
            .unwrap_or(0);
        out.push(remaining.remove(pick));
    }
    out
}

/// Plan the shards.
///
/// `order` is a dependency order over every module the closures name; the
/// prelude is written in it. `weight` gives a module's weight.
pub fn plan(
    closures: &[LineClosure],
    order: &[String],
    weight: &dyn Fn(&str) -> u64,
    shards: usize,
) -> ShardPlan {
    let n = closures.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut rooted: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, closure) in closures.iter().enumerate() {
        rooted.entry(closure.root.as_str()).or_default().push(i);
    }
    for same in rooted.values() {
        for &i in &same[1..] {
            union(&mut parent, same[0], i);
        }
    }
    for (j, closure) in closures.iter().enumerate() {
        for module in &closure.to_build {
            if let Some(lines) = rooted.get(module.as_str()) {
                for &i in lines {
                    union(&mut parent, i, j);
                }
            }
        }
    }

    // Number groups by their first line.
    let mut number: BTreeMap<usize, usize> = BTreeMap::new();
    let mut group_of = vec![0; n];
    for (i, slot) in group_of.iter_mut().enumerate() {
        let root = find(&mut parent, i);
        let next = number.len();
        *slot = *number.entry(root).or_insert(next);
    }
    let groups = number.len();
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); groups];
    let mut modules: Vec<BTreeSet<String>> = vec![BTreeSet::new(); groups];
    for (i, closure) in closures.iter().enumerate() {
        members[group_of[i]].push(i);
        modules[group_of[i]].extend(closure.to_build.iter().cloned());
    }

    let mut needed_by: BTreeMap<&str, usize> = BTreeMap::new();
    for set in &modules {
        for module in set {
            *needed_by.entry(module.as_str()).or_default() += 1;
        }
    }
    let shared: BTreeSet<String> = needed_by
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(module, _)| module.to_string())
        .collect();
    let mut prelude: Vec<String> = order
        .iter()
        .filter(|m| shared.contains(*m))
        .cloned()
        .collect();
    // A module missing from `order` still has to build; it goes last, sorted.
    let in_order: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    prelude.extend(
        shared
            .iter()
            .filter(|m| !in_order.contains(m.as_str()))
            .cloned(),
    );
    let prelude_weight = prelude.iter().map(|m| weight(m)).sum();
    for set in &mut modules {
        set.retain(|m| !shared.contains(m));
    }
    let group_weight: Vec<u64> = modules
        .iter()
        .map(|set| set.iter().map(|m| weight(m)).sum())
        .collect();

    let k = shards.max(1).min(groups.max(1));
    let mut by_weight: Vec<usize> = (0..groups).collect();
    by_weight.sort_by(|a, b| group_weight[*b].cmp(&group_weight[*a]).then(a.cmp(b)));
    let mut load: Vec<(u64, usize)> = vec![(0, 0); k];
    let mut shard_groups: Vec<Vec<usize>> = vec![Vec::new(); k];
    for g in by_weight {
        let target = (0..k)
            .min_by_key(|&s| (load[s].0, load[s].1, s))
            .expect("at least one shard");
        load[target].0 += group_weight[g];
        load[target].1 += 1;
        shard_groups[target].push(g);
    }

    let mut shard_of = vec![0; n];
    let mut out = Vec::new();
    for (s, gs) in shard_groups.iter().enumerate() {
        if gs.is_empty() {
            continue;
        }
        let mut lines: Vec<usize> = gs.iter().flat_map(|&g| members[g].clone()).collect();
        lines.sort_unstable();
        for &i in &lines {
            shard_of[i] = s;
        }
        let set: BTreeSet<String> = gs.iter().flat_map(|&g| modules[g].clone()).collect();
        out.push(Shard {
            lines: line_order(&lines, closures),
            modules: set.into_iter().collect(),
            weight: load[s].0,
        });
    }
    ShardPlan {
        group_of,
        shard_of,
        prelude,
        prelude_weight,
        shards: out,
    }
}

/// Read a weights file: `module<TAB>seconds` per line, `#` comments and blank
/// lines skipped. A fractional value rounds up to whole seconds. A later line
/// for the same module replaces an earlier one.
pub fn parse_weights(text: &str) -> Result<BTreeMap<String, u64>, String> {
    let mut out = BTreeMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (module, value) = line
            .split_once('\t')
            .or_else(|| line.split_once(char::is_whitespace))
            .ok_or_else(|| format!("line {}: want module<TAB>seconds, got {line}", i + 1))?;
        let seconds: f64 = value.trim().parse().map_err(|_| {
            format!(
                "line {}: {} is not a number of seconds",
                i + 1,
                value.trim()
            )
        })?;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(format!(
                "line {}: {} is not a number of seconds",
                i + 1,
                value.trim()
            ));
        }
        out.insert(module.trim().to_string(), seconds.ceil() as u64);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closure(root: &str, to_build: &[&str]) -> LineClosure {
        LineClosure {
            root: root.into(),
            to_build: to_build.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn one(_: &str) -> u64 {
        1
    }

    #[test]
    fn comments_and_blank_lines_are_skipped_and_text_is_kept() {
        let lines =
            parse_lines("# head\n\nA-1.eb --hooks=/x.py  \n  # indented\nB-2.eb --rebuild\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].number, 3);
        assert_eq!(lines[0].text, "A-1.eb --hooks=/x.py");
        assert!(!lines[0].forced);
        assert_eq!(lines[1].file, "B-2.eb");
        assert!(lines[1].forced);
    }

    #[test]
    fn a_shared_dependency_goes_to_the_prelude_and_the_lines_stay_apart() {
        let closures = [
            closure("A/1", &["M/1", "A/1"]),
            closure("B/1", &["M/1", "B/1"]),
        ];
        let order: Vec<String> = ["M/1", "A/1", "B/1"].map(String::from).to_vec();
        let planned = plan(&closures, &order, &one, 4);
        assert_eq!(planned.prelude, vec!["M/1"]);
        assert_eq!(planned.shards.len(), 2);
        assert_ne!(planned.shard_of[0], planned.shard_of[1]);
        assert_eq!(planned.shards[0].modules, vec!["A/1"]);
    }

    #[test]
    fn a_line_another_line_needs_shares_its_shard_and_builds_first() {
        let closures = [
            closure("B/1", &["M/1", "A/1", "B/1"]),
            closure("A/1", &["M/1", "A/1"]),
            closure("C/1", &["M/1", "C/1"]),
        ];
        let order: Vec<String> = ["M/1", "A/1", "B/1", "C/1"].map(String::from).to_vec();
        let planned = plan(&closures, &order, &one, 8);
        assert_eq!(planned.group_of, vec![0, 0, 1]);
        assert_eq!(planned.prelude, vec!["M/1"]);
        let shard = &planned.shards[planned.shard_of[0]];
        assert_eq!(shard.lines, vec![1, 0]);
        assert_eq!(shard.modules, vec!["A/1", "B/1"]);
    }

    #[test]
    fn groups_pack_by_weight_into_at_most_the_requested_shards() {
        let closures = [
            closure("A/1", &["A/1"]),
            closure("B/1", &["B/1"]),
            closure("C/1", &["C/1"]),
        ];
        let order: Vec<String> = ["A/1", "B/1", "C/1"].map(String::from).to_vec();
        let weight = |m: &str| match m {
            "A/1" => 10,
            "B/1" => 6,
            _ => 5,
        };
        let planned = plan(&closures, &order, &weight, 2);
        assert_eq!(planned.shards.len(), 2);
        assert_eq!(planned.shards[0].weight, 10);
        assert_eq!(planned.shards[1].weight, 11);
        assert_eq!(planned.shard_of, vec![0, 1, 1]);
    }

    #[test]
    fn installed_roots_still_get_a_shard_without_an_empty_one() {
        let closures = [closure("A/1", &[]), closure("B/1", &[])];
        let planned = plan(&closures, &[], &one, 2);
        assert_eq!(planned.shards.len(), 2);
        assert_eq!(planned.shards[0].weight, 0);
    }

    #[test]
    fn weights_round_up_and_reject_garbage() {
        let w = parse_weights("# m\tsecs\nA/1\t12.2\nB/1 3\n").unwrap();
        assert_eq!(w["A/1"], 13);
        assert_eq!(w["B/1"], 3);
        assert!(parse_weights("A/1\tlong\n").is_err());
        assert!(parse_weights("A/1\n").is_err());
    }
}
