//! The order to build a set of easyconfigs in, as a graph problem.
//!
//! This is deliberately not [`crate::select`]. Choosing which versions a site
//! should carry is a constraint problem with one answer per package name, and
//! that is what a *stack* is. Sequencing a build is a different question: given
//! what the recipes already pin, in what order can they be built. Mokhov,
//! Mitchell and Peyton Jones separate exactly these two concerns, the task
//! description from the scheduler, in doi:10.1145/3236774, and conflating them
//! is why asking the stack solver for a build order produced conflicts that
//! were policy decisions rather than facts about the recipes.
//!
//! So a node here is a whole module, name and version and toolchain and
//! versionsuffix together, the way a functional deployment model keys a
//! package by its full identity and lets several coexist
//! (doi:10.1017/s0956796810000195). Two versions of binutils, or Perl at
//! GCCcore and at SYSTEM, are simply two nodes. Nothing has to be reconciled,
//! because EasyBuild installs them side by side as different modules, which is
//! what makes them co-installable in the sense of doi:10.1145/2522920.2522927.
//!
//! What remains is a choice function for requirements that admit more than one
//! candidate, and a topological sort. Both are deterministic, so the same tree
//! and the same roots give the same order every time.
//!
//! The graph is petgraph's, and the algorithms are its own rather than
//! hand-rolled: `toposort` for the order, `tarjan_scc` to name every cycle in
//! full when there is one, and `greedy_feedback_arc_set` to say which edges
//! would break it. daggy was the other candidate and refuses a cyclic graph at
//! insertion, returning `WouldCycle` for the edge that closed it. That is the
//! wrong shape here: an easyconfig tree genuinely contains bootstrap cycles,
//! and the useful answer names the whole cycle rather than the one edge that
//! happened to be added last.

use crate::domain::{Candidate, DepReq};
use crate::version::{cmp_version, matches_req};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::{BTreeMap, HashMap, HashSet};

/// A module's full identity, which is what makes two builds the same build.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModuleKey {
    /// Package name.
    pub name: String,
    /// Version as the easyconfig states it.
    pub version: String,
    /// Toolchain, written as EasyBuild names it in a module.
    pub toolchain: String,
    /// Versionsuffix, empty when the recipe has none.
    pub versionsuffix: String,
}

impl ModuleKey {
    /// The key for one candidate.
    pub fn of(candidate: &Candidate) -> Self {
        Self {
            name: candidate.name.clone(),
            version: candidate.version.clone(),
            toolchain: candidate.toolchain.identity_label(),
            versionsuffix: candidate.versionsuffix.clone().unwrap_or_default(),
        }
    }
}

impl std::fmt::Display for ModuleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}{}",
            self.name, self.version, self.toolchain, self.versionsuffix
        )
    }
}

/// Why one build has to happen before another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The dependent loads it at run time.
    Runtime,
    /// The dependent needs it present to build.
    Build,
    /// The dependent is built *with* it: its toolchain, which no easyconfig
    /// lists among its dependencies because EasyBuild reads it off the
    /// `toolchain` line instead.
    Toolchain,
}

impl std::fmt::Display for Edge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime => write!(f, "runtime"),
            Self::Build => write!(f, "build"),
            Self::Toolchain => write!(f, "toolchain"),
        }
    }
}

/// The build graph: an edge runs from a dependency to what needs it, so a
/// topological order is already the order to build in.
pub type BuildGraph = DiGraph<ModuleKey, Edge>;

/// Why an order could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    /// No package of that name is in the tree.
    UnknownRoot {
        /// What was asked for.
        requested: String,
        /// Names close enough to be worth offering.
        suggestions: Vec<String>,
    },
    /// The package exists; that version of it does not.
    NoSuchVersion {
        /// The package name, which was found.
        name: String,
        /// The requirement that matched nothing.
        requirement: String,
        /// What the tree does carry, newest first.
        available: Vec<String>,
    },
    /// A requirement matches nothing, with the module that stated it.
    Unsatisfied {
        /// The module whose dependency could not be met.
        from: ModuleKey,
        /// The dependency as the recipe wrote it.
        requirement: String,
        /// Versions of that package the tree carries.
        available: Vec<String>,
    },
    /// The graph has a cycle. Every module in the strongly connected
    /// component is named, not just the edge that happened to close it, since
    /// a bootstrap chain is broken by choosing where to cut the whole loop.
    Cycle(Vec<ModuleKey>),
    /// Two recipes share a module key. Last-write would drop one of them.
    DuplicateModule {
        /// The key both recipes claim.
        key: ModuleKey,
        /// First recipe that claimed the key.
        first: String,
        /// Second recipe that claimed the same key.
        second: String,
    },
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRoot {
                requested,
                suggestions,
            } => {
                write!(f, "no package named {requested} in this tree")?;
                if !suggestions.is_empty() {
                    write!(f, ". Did you mean {}?", suggestions.join(", "))?;
                }
                Ok(())
            }
            Self::NoSuchVersion {
                name,
                requirement,
                available,
            } => write!(
                f,
                "{name} has no version matching {requirement}. This tree carries {}",
                summarize(available)
            ),
            Self::Unsatisfied {
                from,
                requirement,
                available,
            } => {
                write!(f, "{from} requires {requirement}, which nothing satisfies")?;
                if available.is_empty() {
                    write!(f, ". No package of that name is in this tree")
                } else {
                    write!(f, ". This tree carries {}", summarize(available))
                }
            }
            Self::Cycle(component) => {
                let names: Vec<String> = component.iter().map(ToString::to_string).collect();
                write!(f, "dependency cycle among {}", names.join(", "))
            }
            Self::DuplicateModule { key, first, second } => {
                write!(f, "{key} is provided by both {first} and {second}")
            }
        }
    }
}

impl std::error::Error for OrderError {}

/// A short, readable list: enough to act on, not a wall of versions.
fn summarize(items: &[String]) -> String {
    const SHOWN: usize = 6;
    if items.len() <= SHOWN {
        return items.join(", ");
    }
    format!(
        "{}, and {} more",
        items[..SHOWN].join(", "),
        items.len() - SHOWN
    )
}

/// Names worth offering when the one asked for is not there.
///
/// Case first, because a wrong capital is the commonest miss in a tree whose
/// names capitalise inconsistently, some shouted, some lowercase, some mixed.
/// Then a prefix, then a single-character typo.
fn near_names(requested: &str, candidates: &[Candidate]) -> Vec<String> {
    let wanted = requested.to_ascii_lowercase();
    let mut names: Vec<&str> = candidates.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();

    let mut out: Vec<String> = Vec::new();
    for name in names {
        let lower = name.to_ascii_lowercase();
        if lower == wanted
            || lower.starts_with(&wanted)
            || wanted.starts_with(&lower)
            || within_one_edit(&lower, &wanted)
        {
            out.push(name.to_string());
        }
        if out.len() == 5 {
            break;
        }
    }
    out
}

/// Whether two names differ by at most one insertion, deletion or substitution.
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (long, short) = if a.len() >= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let mut skipped = false;
    let (mut i, mut j) = (0usize, 0usize);
    while i < long.len() && j < short.len() {
        if long[i] == short[j] {
            i += 1;
            j += 1;
            continue;
        }
        if skipped {
            return false;
        }
        skipped = true;
        if long.len() == short.len() {
            i += 1;
            j += 1;
        } else {
            i += 1;
        }
    }
    true
}

/// Which candidate to take when a requirement admits several.
///
/// Sequencing does not decide policy, so this is deliberately small: the
/// question is only which of the admissible builds the order should contain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Choice {
    /// Newest version wins, which matches what a recipe means by `>=`.
    #[default]
    Newest,
    /// Oldest admissible version, for reproducing what an old tree built.
    Oldest,
}

/// The part of a module name after the `/`: version, then the toolchain unless
/// it is the system one, then the versionsuffix.
fn module_version(candidate: &Candidate) -> String {
    let toolchain = if crate::hierarchy::is_system_toolchain(&candidate.toolchain) {
        String::new()
    } else {
        format!(
            "-{}-{}",
            candidate.toolchain.name, candidate.toolchain.version
        )
    };
    format!(
        "{}{toolchain}{}",
        candidate.version,
        candidate.versionsuffix.as_deref().unwrap_or("")
    )
}

/// Whether a candidate satisfies one stated dependency.
///
/// The rules are EasyBuild's: a version requirement, an optional toolchain that
/// names one build and no other, and an optional versionsuffix.
fn satisfies(candidate: &Candidate, dep: &DepReq, recipe: &Candidate) -> bool {
    // Implicit pins stay inside the generation. SYSTEM is only admitted when
    // the recipe is SYSTEM or the tuple names SYSTEM.
    if crate::hierarchy::is_system_toolchain(&candidate.toolchain)
        && !crate::hierarchy::is_system_toolchain(&recipe.toolchain)
        && !dep
            .toolchain
            .as_ref()
            .is_some_and(crate::hierarchy::is_system_toolchain)
    {
        return false;
    }
    if candidate.exts_list.iter().any(|ext| {
        ext.name == dep.name
            && (dep.version_req.is_empty()
                || matches_req(&ext.version, &dep.version_req)
                || dep
                    .version_req
                    .strip_prefix("==")
                    .is_some_and(|pinned| pinned == ext.version))
    }) {
        return true;
    }
    if candidate.name != dep.name {
        return false;
    }
    // A dependency names a module, and a module name puts the toolchain where
    // a reader of the tuple would not expect it: intel-2016b asks for binutils
    // 2.26 with versionsuffix -GCCcore-5.4.0, and what provides it is binutils
    // 2.26 built at GCCcore-5.4.0 carrying no versionsuffix at all. Comparing
    // whole module names accepts that without loosening anything, because it
    // is string equality rather than a relaxed match.
    if let Some(pinned) = dep.version_req.strip_prefix("==") {
        let wanted = format!("{pinned}{}", dep.versionsuffix.as_deref().unwrap_or(""));
        if module_version(candidate) == wanted {
            // Module identity already includes suffix. A toolchain pin still
            // has to hold: returning here used to skip it.
            return dep
                .toolchain
                .as_ref()
                .is_none_or(|want| crate::hierarchy::toolchains_match(&candidate.toolchain, want));
        }
    }
    if !dep.version_req.is_empty() {
        // A dependency may name the version alone, or the version with the
        // versionsuffix run onto it, because that is what the module ends up
        // called: foss-2019a asks for GCC 8.2.0-2.31.1, and the recipe that
        // provides it is version 8.2.0 with versionsuffix -2.31.1. Both
        // spellings have to match or every recipe using the second one reads
        // as unsatisfiable.
        let with_suffix = format!(
            "{}{}",
            candidate.version,
            candidate.versionsuffix.as_deref().unwrap_or("")
        );
        if !matches_req(&candidate.version, &dep.version_req)
            && !matches_req(&with_suffix, &dep.version_req)
        {
            return false;
        }
    }
    if let Some(want) = dep.toolchain.as_ref() {
        if !crate::hierarchy::toolchains_match(&candidate.toolchain, want) {
            return false;
        }
    }
    // A 2-tuple leaves versionsuffix None, which is "no suffix", not "any
    // suffix". Treating None as any would let Newest pick a -bare/-CUDA
    // build for ('Python', '3.11.3').
    dep.versionsuffix.as_deref().unwrap_or("") == candidate.versionsuffix.as_deref().unwrap_or("")
}

/// How far a candidate sits from the recipe that needs it.
///
/// EasyBuild resolves an unpinned dependency inside the recipe's own
/// generation, taking the lowest level that has it, and only a tuple that
/// names a toolchain reaches outside. Without that discipline a closure walks
/// into whatever generation happens to hold the newest matching version, which
/// is how a 2026 root ends up pulling a GCCcore-11.3.0 bootstrap chain and
/// closing a cycle that does not exist within either generation.
fn distance(
    candidate: &Candidate,
    recipe: &Candidate,
    members: &[crate::domain::Toolchain],
) -> usize {
    if crate::hierarchy::toolchains_match(&candidate.toolchain, &recipe.toolchain) {
        return 0;
    }
    // Members run lowest level first, so walking from the recipe downwards
    // gives nearer levels a smaller distance.
    if let Some(at) = members
        .iter()
        .position(|m| crate::hierarchy::toolchains_match(m, &candidate.toolchain))
    {
        let recipe_at = members
            .iter()
            .position(|m| crate::hierarchy::toolchains_match(m, &recipe.toolchain))
            .unwrap_or(members.len());
        if at <= recipe_at {
            return 1 + (recipe_at - at);
        }
    }
    usize::MAX
}

/// A `--roots name==version` pin matches the recipe version, or the version
/// with versionsuffix glued on: foss-2019a writes GCC 8.2.0-2.31.1 for a
/// recipe whose version is 8.2.0 and whose suffix is -2.31.1.
fn root_version_matches(candidate: &Candidate, version_req: &str) -> bool {
    if version_req.is_empty() {
        return true;
    }
    if matches_req(&candidate.version, version_req) {
        return true;
    }
    let with_suffix = format!(
        "{}{}",
        candidate.version,
        candidate.versionsuffix.as_deref().unwrap_or("")
    );
    matches_req(&with_suffix, version_req)
}

/// Pick one candidate from those a requirement admits.
fn choose<'a>(admissible: &[&'a Candidate], choice: Choice) -> Option<&'a Candidate> {
    admissible.iter().copied().max_by(|a, b| {
        let by_version = cmp_version(&a.version, &b.version);
        let ordered = match choice {
            Choice::Newest => by_version,
            Choice::Oldest => by_version.reverse(),
        };
        // Ties break on the whole key so the answer cannot depend on the
        // order the tree happened to be read in.
        ordered.then_with(|| cmp_candidate_identity(b, a))
    })
}

fn cmp_candidate_identity(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    a.name
        .cmp(&b.name)
        .then_with(|| a.version.cmp(&b.version))
        .then_with(|| cmp_identity_toolchain(&a.toolchain, &b.toolchain))
        .then_with(|| {
            a.versionsuffix
                .as_deref()
                .unwrap_or("")
                .cmp(b.versionsuffix.as_deref().unwrap_or(""))
        })
}

fn cmp_identity_toolchain(
    a: &crate::domain::Toolchain,
    b: &crate::domain::Toolchain,
) -> std::cmp::Ordering {
    cmp_identity_label_parts(
        a.is_system(),
        &a.name,
        &a.version,
        b.is_system(),
        &b.name,
        &b.version,
    )
}

fn cmp_identity_label_parts(
    a_system: bool,
    a_name: &str,
    a_version: &str,
    b_system: bool,
    b_name: &str,
    b_version: &str,
) -> std::cmp::Ordering {
    let a = if a_system {
        ["system", "", ""]
    } else {
        [a_name, "-", a_version]
    };
    let b = if b_system {
        ["system", "", ""]
    } else {
        [b_name, "-", b_version]
    };
    let mut left = a.into_iter().flat_map(str::chars);
    let mut right = b.into_iter().flat_map(str::chars);
    loop {
        match (left.next(), right.next()) {
            (Some(x), Some(y)) => {
                let order = x.cmp(&y);
                if order != std::cmp::Ordering::Equal {
                    return order;
                }
            }
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
        }
    }
}

fn candidates_by_key(
    candidates: &[Candidate],
) -> Result<BTreeMap<ModuleKey, &Candidate>, OrderError> {
    let mut map = BTreeMap::new();
    for candidate in candidates {
        let key = ModuleKey::of(candidate);
        if let Some(previous) = map.insert(key.clone(), candidate) {
            if previous.easyconfig_path != candidate.easyconfig_path {
                return Err(OrderError::DuplicateModule {
                    key,
                    first: previous.easyconfig_path.clone(),
                    second: candidate.easyconfig_path.clone(),
                });
            }
        }
    }
    Ok(map)
}

fn candidates_named<'a>(
    by_name: &HashMap<&str, Vec<&'a Candidate>>,
    by_ext: &HashMap<&str, Vec<&'a Candidate>>,
    name: &str,
) -> Vec<&'a Candidate> {
    let mut named = Vec::new();
    if let Some(candidates) = by_name.get(name) {
        named.extend(candidates.iter().copied());
    }
    if let Some(candidates) = by_ext.get(name) {
        for candidate in candidates {
            if !named.iter().any(|seen| std::ptr::eq(*seen, *candidate)) {
                named.push(*candidate);
            }
        }
    }
    named
}

/// Build the graph the recipes describe, reachable from `roots`.
///
/// Nodes are whole modules and edges run from a dependency to what needs it.
/// The graph is returned even when it has a cycle, because naming the cycle is
/// more useful than refusing to hand it over.
// The error carries what someone needs to fix the failure: the versions that
// exist, the names that were nearly right. That makes it a large Err variant on
// a path that returns at most once per run, which is a trade worth taking:
// boxing it would save a few bytes and cost every caller a dereference.
#[allow(clippy::result_large_err)]
pub fn build_graph(
    candidates: &[Candidate],
    roots: &[String],
    choice: Choice,
) -> Result<BuildGraph, OrderError> {
    let mut graph: BuildGraph = DiGraph::new();
    let mut index: HashMap<ModuleKey, NodeIndex> = HashMap::new();
    let mut queue: Vec<ModuleKey> = Vec::new();
    let by_key = candidates_by_key(candidates)?;
    let mut by_name: HashMap<&str, Vec<&Candidate>> = HashMap::new();
    let mut by_ext: HashMap<&str, Vec<&Candidate>> = HashMap::new();
    for candidate in candidates {
        by_name
            .entry(candidate.name.as_str())
            .or_default()
            .push(candidate);
        for ext in &candidate.exts_list {
            by_ext.entry(ext.name.as_str()).or_default().push(candidate);
        }
    }

    let node_for = |graph: &mut BuildGraph,
                    index: &mut HashMap<ModuleKey, NodeIndex>,
                    key: &ModuleKey|
     -> NodeIndex {
        *index
            .entry(key.clone())
            .or_insert_with(|| graph.add_node(key.clone()))
    };

    for root in roots {
        let (name, version_req) = match root.split_once("==") {
            Some((name, version)) => (name, format!("=={version}")),
            None => (root.as_str(), String::new()),
        };
        let admissible: Vec<&Candidate> = by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|c| root_version_matches(c, &version_req))
            .collect();
        let start = match choose(&admissible, choice) {
            Some(picked) => picked,
            None => {
                let mut of_that_name: Vec<String> = by_name
                    .get(name)
                    .into_iter()
                    .flatten()
                    .map(|c| c.version.clone())
                    .collect();
                of_that_name.sort_by(|a, b| cmp_version(b, a));
                of_that_name.dedup();
                // A package that exists but not at that version is a different
                // mistake from one that does not exist, and saying which is the
                // difference between a one-line fix and a search.
                return Err(if of_that_name.is_empty() {
                    OrderError::UnknownRoot {
                        requested: name.to_string(),
                        suggestions: near_names(name, candidates),
                    }
                } else {
                    OrderError::NoSuchVersion {
                        name: name.to_string(),
                        requirement: version_req.clone(),
                        available: of_that_name,
                    }
                });
            }
        };
        let key = ModuleKey::of(start);
        node_for(&mut graph, &mut index, &key);
        queue.push(key);
    }

    let mut seen: HashSet<ModuleKey> = HashSet::new();
    while let Some(key) = queue.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(candidate) = by_key.get(&key) else {
            continue;
        };
        let dependent = node_for(&mut graph, &mut index, &key);

        // The toolchain a recipe is built with has to exist first, and no
        // easyconfig lists it among its dependencies: EasyBuild reads it off
        // the `toolchain` line. Without this edge a gompi recipe can be
        // ordered ahead of the OpenMPI its toolchain is made of, which reads
        // as a valid order and is not one.
        if !crate::hierarchy::is_system_toolchain(&candidate.toolchain) {
            // EasyBuild's toolchain line is the unsuffixed module. A CUDA
            // GCC-12.3.0 sitting first in parse order is not GCC-12.3.0.
            let mut admissible: Vec<&Candidate> = by_name
                .get(candidate.toolchain.name.as_str())
                .into_iter()
                .flatten()
                .copied()
                .filter(|c| {
                    c.version == candidate.toolchain.version
                        && c.versionsuffix.as_deref().unwrap_or("").is_empty()
                })
                .collect();
            let system_defs: Vec<&Candidate> = admissible
                .iter()
                .copied()
                .filter(|c| crate::hierarchy::is_system_toolchain(&c.toolchain))
                .collect();
            if !system_defs.is_empty() {
                admissible = system_defs;
            }
            admissible.sort_by_key(|c| ModuleKey::of(c));
            match choose(&admissible, choice) {
                Some(tc) => {
                    let tc_key = ModuleKey::of(tc);
                    if tc_key != key {
                        let node = node_for(&mut graph, &mut index, &tc_key);
                        graph.add_edge(node, dependent, Edge::Toolchain);
                        queue.push(tc_key);
                    }
                }
                None => {
                    let mut available: Vec<String> = by_name
                        .get(candidate.toolchain.name.as_str())
                        .into_iter()
                        .flatten()
                        .map(|c| format!("{}-{}", c.version, ModuleKey::of(c)))
                        .collect();
                    available.sort();
                    available.dedup();
                    return Err(OrderError::Unsatisfied {
                        from: key.clone(),
                        requirement: format!(
                            "{} {}",
                            candidate.toolchain.name, candidate.toolchain.version
                        ),
                        available,
                    });
                }
            }
        }

        // Deps are walked in a stable order so the graph, and the order read
        // out of it, are properties of the tree and not of hash iteration.
        let mut deps: Vec<(&DepReq, Edge)> = candidate
            .dependencies
            .iter()
            .map(|d| (d, Edge::Runtime))
            .chain(candidate.builddependencies.iter().map(|d| (d, Edge::Build)))
            .collect();
        deps.sort_by(|(a, _), (b, _)| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version_req.cmp(&b.version_req))
        });

        let recipe_members =
            crate::hierarchy::hierarchy_for_with_tree(&candidate.toolchain, None, candidates)
                .map(|hierarchy| hierarchy.members)
                .unwrap_or_default();
        for (dep, kind) in deps {
            let mut scored: Vec<(&Candidate, usize)> =
                candidates_named(&by_name, &by_ext, &dep.name)
                    .into_iter()
                    .filter(|c| satisfies(c, dep, candidate))
                    .map(|c| (c, distance(c, candidate, &recipe_members)))
                    .collect();
            // Nearest generation first, then the choice function decides among
            // equals. A dependency that pins a toolchain was already narrowed
            // to that one build by `satisfies`. usize::MAX means "not in this
            // generation": keep those out so an unknown local-* toolchain
            // cannot walk to whichever GCCcore happens to hold the newest Lib.
            if let Some(best) = scored
                .iter()
                .map(|(_, distance)| *distance)
                .filter(|distance| *distance != usize::MAX)
                .min()
            {
                scored.retain(|(_, distance)| *distance == best);
            } else {
                scored.clear();
            }
            let admissible: Vec<&Candidate> = scored.into_iter().map(|(c, _)| c).collect();
            let Some(picked) = choose(&admissible, choice) else {
                let mut available: Vec<String> = candidates_named(&by_name, &by_ext, &dep.name)
                    .into_iter()
                    .map(|c| format!("{}-{}", c.version, ModuleKey::of(c).toolchain))
                    .collect();
                available.sort();
                available.dedup();
                return Err(OrderError::Unsatisfied {
                    from: key.clone(),
                    requirement: format!("{} {}", dep.name, dep.version_req)
                        .trim()
                        .to_string(),
                    available,
                });
            };
            let dep_key = ModuleKey::of(picked);
            let node = node_for(&mut graph, &mut index, &dep_key);
            graph.add_edge(node, dependent, kind);
            queue.push(dep_key);
        }
    }
    Ok(graph)
}

/// What to build, in the order to build it.
///
/// Roots are package names, optionally `name==version`. Every dependency the
/// reachable recipes state is included, build-time and runtime alike, since
/// both have to exist before the build starts.
// The error carries what someone needs to fix the failure: the versions that
// exist, the names that were nearly right. That makes it a large Err variant on
// a path that returns at most once per run, which is a trade worth taking:
// boxing it would save a few bytes and cost every caller a dereference.
#[allow(clippy::result_large_err)]
pub fn build_order(
    candidates: &[Candidate],
    roots: &[String],
    choice: Choice,
) -> Result<Vec<Candidate>, OrderError> {
    Ok(build_order_with_graph(candidates, roots, choice)?.0)
}

/// As [`build_order`], also returning the graph used for the sort so hashes
/// do not have to walk the tree a second time.
pub fn build_order_with_graph(
    candidates: &[Candidate],
    roots: &[String],
    choice: Choice,
) -> Result<(Vec<Candidate>, BuildGraph), OrderError> {
    let graph = build_graph(candidates, roots, choice)?;
    let by_key = candidates_by_key(candidates)?;

    let sorted = petgraph::algo::toposort(&graph, None).map_err(|cycle| {
        // toposort names one node in a cycle; the useful answer is the whole
        // component, and which edges would break it.
        let components = petgraph::algo::tarjan_scc(&graph);
        let guilty = components
            .into_iter()
            .find(|component| component.contains(&cycle.node_id()))
            .unwrap_or_else(|| vec![cycle.node_id()]);
        OrderError::Cycle(guilty.into_iter().map(|n| graph[n].clone()).collect())
    })?;

    let order = sorted
        .into_iter()
        .filter_map(|node| by_key.get(&graph[node]).map(|c| (*c).clone()))
        .collect();
    Ok((order, graph))
}

/// Which edges would break the cycles in a graph, if any.
///
/// A bootstrap chain is a real cycle in the tree and someone has to decide
/// where to cut it, usually by taking one build from the previous generation.
/// This says where the cut is cheapest rather than leaving it to be guessed.
pub fn cycle_breaking_edges(graph: &BuildGraph) -> Vec<(ModuleKey, ModuleKey, Edge)> {
    petgraph::algo::greedy_feedback_arc_set(graph)
        .map(|edge| {
            (
                graph[edge.source()].clone(),
                graph[edge.target()].clone(),
                *edge.weight(),
            )
        })
        .collect()
}

/// The graph in Graphviz DOT, for looking at a generation rather than reading
/// six hundred lines of it.
pub fn to_dot(graph: &BuildGraph) -> String {
    use petgraph::dot::{Config, Dot};
    format!(
        "{:?}",
        Dot::with_attr_getters(
            graph,
            &[Config::EdgeNoLabel],
            &|_, edge| match edge.weight() {
                Edge::Runtime => "color=\"#004D40\"".to_string(),
                Edge::Build => "color=\"#004D40\",style=dashed".to_string(),
                Edge::Toolchain => "color=\"#FF655D\",penwidth=2".to_string(),
            },
            &|_, (_, key)| format!("label=\"{key}\",fontname=\"Jost\",shape=box"),
        )
    )
}

/// The order as easyconfig paths, one per line, ready for a build list.
pub fn format_order(order: &[Candidate]) -> String {
    let mut out = String::new();
    for c in order {
        if c.easyconfig_path.is_empty() || c.is_extension_provide() {
            continue;
        }
        out.push_str(&c.easyconfig_path);
        out.push('\n');
    }
    out
}

/// How many distinct builds of each name the order contains.
///
/// A name with more than one build is the case a stack solve cannot express,
/// so it is worth reporting rather than leaving for someone to notice.
pub fn multi_build_names(order: &[Candidate]) -> BTreeMap<String, Vec<String>> {
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in order {
        let key = ModuleKey::of(c);
        seen.entry(c.name.clone()).or_default().push(format!(
            "{}-{}{}",
            key.version, key.toolchain, key.versionsuffix
        ));
    }
    seen.retain(|_, builds| {
        builds.sort();
        builds.dedup();
        builds.len() > 1
    });
    seen
}

/// Index of runtime edges for callers that want the graph rather than the list.
pub fn runtime_edges(order: &[Candidate]) -> HashMap<String, Vec<String>> {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for c in order {
        edges.insert(
            ModuleKey::of(c).to_string(),
            c.dependencies.iter().map(|d| d.name.clone()).collect(),
        );
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Toolchain;

    fn tc(name: &str, version: &str) -> Toolchain {
        Toolchain {
            name: name.into(),
            version: version.into(),
        }
    }

    fn dep(name: &str, req: &str, toolchain: Option<Toolchain>) -> DepReq {
        DepReq {
            name: name.into(),
            version_req: req.into(),
            toolchain,
            versionsuffix: None,
        }
    }

    fn candidate(name: &str, version: &str, toolchain: Toolchain, deps: Vec<DepReq>) -> Candidate {
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain,
            versionsuffix: None,
            dependencies: deps,
            builddependencies: Vec::new(),
            easyconfig_path: format!("x/{name}/{name}-{version}.eb"),
            exts_list: Vec::new(),
            moduleclass: None,
        }
    }

    fn names(order: &[Candidate]) -> Vec<String> {
        order.iter().map(|c| ModuleKey::of(c).to_string()).collect()
    }

    fn tree(candidates: &[Candidate]) -> Vec<Candidate> {
        let mut extra = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for candidate in candidates {
            if crate::hierarchy::is_system_toolchain(&candidate.toolchain) {
                continue;
            }
            if seen.insert((
                candidate.toolchain.name.clone(),
                candidate.toolchain.version.clone(),
            )) {
                extra.push(self::candidate(
                    &candidate.toolchain.name,
                    &candidate.toolchain.version,
                    tc("system", "system"),
                    vec![],
                ));
            }
        }
        extra.extend(candidates.iter().cloned());
        extra
    }

    #[test]
    fn dependencies_come_before_what_needs_them() {
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("Lib", ">=2.0", None)],
            ),
            candidate(
                "Lib",
                "2.1",
                tc("foss", "2026.1"),
                vec![dep("Base", "", None)],
            ),
            candidate("Base", "0.9", tc("GCCcore", "15.2.0"), vec![]),
        ];
        let order = build_order(&tree(&all), &["App".into()], Choice::Newest).expect("order");
        let seq = names(&order);
        let at = |n: &str| seq.iter().position(|s| s.starts_with(n)).unwrap();
        assert!(at("Base") < at("Lib"), "{seq:?}");
        assert!(at("Lib") < at("App"), "{seq:?}");
    }

    /// The case a stack solve cannot express: two builds of one name, both
    /// required, both installed side by side as different modules.
    #[test]
    fn two_builds_of_one_package_both_appear() {
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("GCC", "15.2.0"),
                vec![
                    dep("Perl", "==5.42.0", Some(tc("GCCcore", "15.2.0"))),
                    dep("zlib", "", None),
                ],
            ),
            candidate(
                "zlib",
                "2.3.2",
                tc("GCCcore", "15.2.0"),
                vec![dep("Perl", "==5.38.0", Some(tc("system", "system")))],
            ),
            candidate("Perl", "5.42.0", tc("GCCcore", "15.2.0"), vec![]),
            candidate("Perl", "5.38.0", tc("system", "system"), vec![]),
        ];
        let order = build_order(&tree(&all), &["App".into()], Choice::Newest).expect("order");
        let seq = names(&order);
        assert!(
            seq.iter().any(|s| s == "Perl-5.42.0-GCCcore-15.2.0"),
            "{seq:?}"
        );
        assert!(seq.iter().any(|s| s == "Perl-5.38.0-system"), "{seq:?}");
        let multi = multi_build_names(&order);
        assert_eq!(multi.get("Perl").map(Vec::len), Some(2), "{multi:?}");
    }

    #[test]
    fn an_unsuffixed_pin_does_not_take_a_newer_suffixed_build() {
        let mut bare = candidate("Python", "3.12.0", tc("foss", "2026.1"), vec![]);
        bare.versionsuffix = Some("-bare".into());
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("Python", "==3.11.3", None)],
            ),
            candidate("Python", "3.11.3", tc("foss", "2026.1"), vec![]),
            bare,
        ];
        let order = build_order(&tree(&all), &["App".into()], Choice::Newest).expect("order");
        let seq = names(&order);
        assert!(
            seq.iter().any(|s| s == "Python-3.11.3-foss-2026.1"),
            "{seq:?}"
        );
        assert!(
            !seq.iter().any(|s| s.contains("-bare")),
            "unsuffixed pin must not pick -bare: {seq:?}"
        );
    }

    #[test]
    fn multi_build_names_keeps_a_cuda_variant() {
        let mut cuda = candidate("App", "1.0", tc("foss", "2026.1"), vec![]);
        cuda.versionsuffix = Some("-CUDA-12.8.0".into());
        let all = vec![candidate("App", "1.0", tc("foss", "2026.1"), vec![]), cuda];
        let multi = multi_build_names(&all);
        assert_eq!(
            multi.get("App").map(Vec::len),
            Some(2),
            "CUDA and plain must stay distinct: {multi:?}"
        );
    }

    #[test]
    fn an_implicit_pin_does_not_take_a_system_build() {
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("Python", "==3.11.3", None)],
            ),
            candidate("Python", "3.11.3", tc("system", "system"), vec![]),
        ];
        let err = build_order(&tree(&all), &["App".into()], Choice::Newest).unwrap_err();
        assert!(
            matches!(err, OrderError::Unsatisfied { .. }),
            "implicit pin must not take SYSTEM: {err}"
        );
    }

    #[test]
    fn an_explicit_system_tuple_still_takes_the_system_build() {
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("Python", "==3.11.3", Some(tc("system", "system")))],
            ),
            candidate("Python", "3.11.3", tc("system", "system"), vec![]),
        ];
        let order = build_order(&tree(&all), &["App".into()], Choice::Newest).expect("order");
        assert!(
            names(&order).iter().any(|s| s == "Python-3.11.3-system"),
            "{:?}",
            names(&order)
        );
    }

    #[test]
    fn a_bundle_provide_satisfies_an_extension_name() {
        let mut bundle = candidate("SciPy-bundle", "2025.06", tc("foss", "2026.1"), vec![]);
        bundle.exts_list = vec![crate::domain::ExtEntry {
            name: "numpy".into(),
            version: "2.3.1".into(),
        }];
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("numpy", "==2.3.1", None)],
            ),
            bundle,
        ];
        let order = build_order(&tree(&all), &["App".into()], Choice::Newest).expect("order");
        let seq = names(&order);
        assert!(seq.iter().any(|s| s.starts_with("SciPy-bundle")), "{seq:?}");
        assert!(
            !format_order(&order).contains("#ext:"),
            "{}",
            format_order(&order)
        );
    }

    #[test]
    fn colliding_module_keys_are_refused() {
        let mut first = candidate("Lib", "1.0", tc("foss", "2026.1"), vec![]);
        first.easyconfig_path = "upstream/Lib-1.0.eb".into();
        let mut second = candidate("Lib", "1.0", tc("foss", "2026.1"), vec![]);
        second.easyconfig_path = "overlay/Lib-1.0.eb".into();
        let err = build_order(&[first, second], &["Lib".into()], Choice::Newest).unwrap_err();
        assert!(matches!(err, OrderError::DuplicateModule { .. }), "{err}");
    }

    #[test]
    fn a_cycle_is_reported_with_the_path() {
        let all = vec![
            candidate("A", "1.0", tc("foss", "2026.1"), vec![dep("B", "", None)]),
            candidate("B", "1.0", tc("foss", "2026.1"), vec![dep("A", "", None)]),
        ];
        let err = build_order(&tree(&all), &["A".into()], Choice::Newest).unwrap_err();
        match err {
            OrderError::Cycle(component) => {
                let shown: Vec<String> = component.iter().map(ToString::to_string).collect();
                // The whole component, so both ends of the loop are named.
                assert!(shown.iter().any(|s| s.starts_with("A-")), "{shown:?}");
                assert!(shown.iter().any(|s| s.starts_with("B-")), "{shown:?}");
            }
            other => panic!("expected a cycle, got {other}"),
        }
    }

    /// A failure that only says no is a failure someone has to investigate by
    /// hand. These three say what to do next.
    #[test]
    fn a_misspelled_root_is_offered_the_name_it_missed() {
        let all = vec![candidate("GROMACS", "2026.3", tc("foss", "2026.1"), vec![])];
        let err = build_order(&tree(&all), &["GROMAC".into()], Choice::Newest).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("GROMACS"), "{shown}");
        assert!(shown.contains("Did you mean"), "{shown}");
    }

    #[test]
    fn a_wrong_capital_is_still_found() {
        let all = vec![candidate("pkgconf", "2.5.1", tc("foss", "2026.1"), vec![])];
        let err = build_order(&tree(&all), &["PkgConf".into()], Choice::Newest).unwrap_err();
        assert!(err.to_string().contains("pkgconf"), "{err}");
    }

    /// The package is there; the version is not. Saying so is the difference
    /// between a one-line fix and a search.
    #[test]
    fn a_version_that_does_not_exist_lists_the_ones_that_do() {
        let all = vec![
            candidate("GROMACS", "2026.3", tc("foss", "2026.1"), vec![]),
            candidate("GROMACS", "2025.0", tc("foss", "2026.1"), vec![]),
        ];
        let err = build_order(&tree(&all), &["GROMACS==99.0".into()], Choice::Newest).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("no version matching"), "{shown}");
        assert!(shown.contains("2026.3"), "{shown}");
        assert!(shown.contains("2025.0"), "{shown}");
        assert!(!shown.contains("no package named"), "{shown}");
    }

    #[test]
    fn an_unsatisfiable_dependency_lists_what_the_tree_has() {
        let all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("Lib", ">=9", None)],
            ),
            candidate("Lib", "2.0", tc("foss", "2026.1"), vec![]),
        ];
        let err = build_order(&tree(&all), &["App".into()], Choice::Newest).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("Lib >=9"), "{shown}");
        assert!(shown.contains("2.0-foss-2026.1"), "{shown}");
    }

    #[test]
    fn a_long_list_of_versions_is_summarized_rather_than_dumped() {
        let all: Vec<Candidate> = (1..=12)
            .map(|n| candidate("Many", &format!("{n}.0"), tc("foss", "2026.1"), vec![]))
            .collect();
        let err = build_order(&tree(&all), &["Many==99".into()], Choice::Newest).unwrap_err();
        assert!(err.to_string().contains("and 6 more"), "{err}");
    }

    #[test]
    fn an_unsatisfiable_requirement_names_who_asked() {
        let all = vec![candidate(
            "App",
            "1.0",
            tc("foss", "2026.1"),
            vec![dep("Missing", ">=9", None)],
        )];
        let err = build_order(&tree(&all), &["App".into()], Choice::Newest).unwrap_err();
        match err {
            OrderError::Unsatisfied {
                from, requirement, ..
            } => {
                assert_eq!(from.name, "App");
                assert!(requirement.contains("Missing"), "{requirement}");
            }
            other => panic!("expected an unsatisfied requirement, got {other}"),
        }
    }

    #[test]
    fn the_order_does_not_depend_on_how_the_tree_was_read() {
        let mut all = vec![
            candidate(
                "App",
                "1.0",
                tc("foss", "2026.1"),
                vec![dep("Lib", "", None)],
            ),
            candidate("Lib", "2.0", tc("foss", "2026.1"), vec![]),
            candidate("Lib", "2.1", tc("foss", "2026.1"), vec![]),
        ];
        let first = names(&build_order(&tree(&all), &["App".into()], Choice::Newest).unwrap());
        all.reverse();
        let second = names(&build_order(&tree(&all), &["App".into()], Choice::Newest).unwrap());
        assert_eq!(first, second);
        // Newest wins by default; oldest is available for reproducing a tree.
        assert!(first.iter().any(|s| s.starts_with("Lib-2.1")), "{first:?}");
        let oldest = names(&build_order(&tree(&all), &["App".into()], Choice::Oldest).unwrap());
        assert!(
            oldest.iter().any(|s| s.starts_with("Lib-2.0")),
            "{oldest:?}"
        );
    }

    #[test]
    fn a_root_can_pin_version_glued_to_versionsuffix() {
        let mut gcc = candidate("GCC", "8.2.0", tc("system", "system"), vec![]);
        gcc.versionsuffix = Some("-2.31.1".into());
        let order = build_order(&[gcc], &["GCC==8.2.0-2.31.1".into()], Choice::Newest)
            .expect("suffix-glued root");
        assert_eq!(names(&order), vec!["GCC-8.2.0-system-2.31.1".to_string()]);
    }

    #[test]
    fn a_root_can_pin_its_own_version() {
        let all = vec![
            candidate("Lib", "2.0", tc("foss", "2026.1"), vec![]),
            candidate("Lib", "2.1", tc("foss", "2026.1"), vec![]),
        ];
        let order = build_order(&tree(&all), &["Lib==2.0".into()], Choice::Newest).unwrap();
        assert_eq!(
            names(&order),
            vec![
                "foss-2026.1-system".to_string(),
                "Lib-2.0-foss-2026.1".to_string()
            ]
        );
    }

    #[test]
    fn toolchain_edge_prefers_the_unsuffixed_module() {
        let mut cuda = candidate("GCC", "12.3.0", tc("system", "system"), vec![]);
        cuda.versionsuffix = Some("-CUDA-12.8.0".into());
        let plain = candidate("GCC", "12.3.0", tc("system", "system"), vec![]);
        let app = candidate("App", "1.0", tc("GCC", "12.3.0"), vec![]);
        for all in [
            vec![cuda.clone(), plain.clone(), app.clone()],
            vec![app.clone(), plain.clone(), cuda.clone()],
        ] {
            let order = build_order(&tree(&all), &["App".into()], Choice::Newest).expect("order");
            let seq = names(&order);
            assert!(
                seq.iter().any(|name| name == "GCC-12.3.0-system"),
                "unsuffixed GCC must be the toolchain predecessor: {seq:?}"
            );
            assert!(
                !seq.iter().any(|name| name.contains("-CUDA-")),
                "CUDA GCC must not win the toolchain line: {seq:?}"
            );
        }
    }

    #[test]
    fn unknown_hierarchy_does_not_take_another_generation_newest() {
        let all = vec![
            candidate("local", "1.0", tc("system", "system"), vec![]),
            candidate(
                "App",
                "1.0",
                tc("local", "1.0"),
                vec![dep("Lib", ">=1", None)],
            ),
            candidate("Lib", "2.0", tc("local", "1.0"), vec![]),
            candidate("Lib", "9.0", tc("GCCcore", "11.3.0"), vec![]),
        ];
        let order = build_order(&all, &["App".into()], Choice::Newest).expect("order");
        let seq = names(&order);
        assert!(
            seq.iter().any(|name| name.starts_with("Lib-2.0")),
            "in-generation Lib must win: {seq:?}"
        );
        assert!(
            !seq.iter().any(|name| name.starts_with("Lib-9.0")),
            "unknown hierarchy must not walk to GCCcore-11.3.0: {seq:?}"
        );
    }

    #[test]
    fn unknown_hierarchy_does_not_satisfy_from_a_foreign_generation() {
        let all = vec![
            candidate("local", "1.0", tc("system", "system"), vec![]),
            candidate("GCCcore", "11.3.0", tc("system", "system"), vec![]),
            candidate(
                "App",
                "1.0",
                tc("local", "1.0"),
                vec![dep("Lib", ">=1", None)],
            ),
            candidate("Lib", "9.0", tc("GCCcore", "11.3.0"), vec![]),
        ];
        let err = build_order(&all, &["App".into()], Choice::Newest).unwrap_err();
        match err {
            OrderError::Unsatisfied { requirement, .. } => {
                assert!(
                    requirement.contains("Lib"),
                    "the hole is Lib, not a missing toolchain: {requirement}"
                );
            }
            other => panic!(
                "foreign-generation Lib must not satisfy an unknown local hierarchy: {other}"
            ),
        }
    }

    #[test]
    fn missing_toolchain_definition_is_unsatisfied() {
        let all = vec![candidate("App", "1.0", tc("foss", "2026.1"), vec![])];
        let err = build_order(&all, &["App".into()], Choice::Newest).unwrap_err();
        match err {
            OrderError::Unsatisfied {
                from, requirement, ..
            } => {
                assert_eq!(from.name, "App");
                assert!(
                    requirement.contains("foss"),
                    "missing toolchain must be named: {requirement}"
                );
            }
            other => panic!("expected Unsatisfied, got {other}"),
        }
    }
}
