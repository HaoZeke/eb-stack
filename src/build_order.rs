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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
            toolchain: if crate::hierarchy::is_system_toolchain(&candidate.toolchain) {
                "system".to_string()
            } else {
                format!(
                    "{}-{}",
                    candidate.toolchain.name, candidate.toolchain.version
                )
            },
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

/// How one module uses another.
///
/// A module listed in both `dependencies` and `builddependencies` is
/// [`UseKind::Runtime`]. The runtime edge is the shared-library link, so
/// "builddependency only" means the runtime list does not name it.
/// [`UseKind::Toolchain`] is the `toolchain` line. The resolver records it
/// because the build needs the toolchain first. It is not a link to a
/// patched library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UseKind {
    /// Listed in `dependencies`.
    Runtime,
    /// Listed in `builddependencies` and not in `dependencies`.
    BuildOnly,
    /// This module's name and version appear in the bundle's `exts_list`.
    Extension,
    /// The dependent is built with this module as its toolchain.
    Toolchain,
}

impl std::fmt::Display for UseKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime => write!(f, "runtime"),
            Self::BuildOnly => write!(f, "builddependency"),
            Self::Extension => write!(f, "extension"),
            Self::Toolchain => write!(f, "toolchain"),
        }
    }
}

/// One module that uses another, and the kind of use.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReverseUse {
    /// The module that does the using.
    pub dependent: ModuleKey,
    /// How it uses the dependency.
    pub kind: UseKind,
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
fn satisfies(candidate: &Candidate, dep: &DepReq) -> bool {
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
            return true;
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
    match dep.versionsuffix.as_deref() {
        Some(suffix) => candidate.versionsuffix.as_deref().unwrap_or("") == suffix,
        None => true,
    }
}

/// How far a candidate sits from the recipe that needs it.
///
/// EasyBuild resolves an unpinned dependency inside the recipe's own
/// generation, taking the lowest level that has it, and only a tuple that
/// names a toolchain reaches outside. Without that discipline a closure walks
/// into whatever generation happens to hold the newest matching version, which
/// is how a 2026 root ends up pulling a GCCcore-11.3.0 bootstrap chain and
/// closing a cycle that does not exist within either generation.
fn distance(candidate: &Candidate, recipe: &Candidate, all: &[Candidate]) -> usize {
    if crate::hierarchy::toolchains_match(&candidate.toolchain, &recipe.toolchain) {
        return 0;
    }
    let members = crate::hierarchy::hierarchy_for_with_tree(&recipe.toolchain, None, all)
        .map(|h| h.members)
        .unwrap_or_default();
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

/// Keep the candidates EasyBuild would actually pick a dependency from.
///
/// With minimal toolchains off, the robot searches the recipe's own generation
/// from the parent toolchain downward. A candidate outside that hierarchy is
/// not a fallback: taking it is how a 2026 root pulls a `gompi-2022a` build
/// and then the whole older generation. A dependency that names a toolchain
/// was already limited to that toolchain by [`satisfies`], and that pin is
/// allowed to point outside the generation.
fn within_generation<'a>(
    mut admissible: Vec<&'a Candidate>,
    recipe: &Candidate,
    all: &[Candidate],
    dep: &DepReq,
) -> Vec<&'a Candidate> {
    let Some(best) = admissible.iter().map(|c| distance(c, recipe, all)).min() else {
        return admissible;
    };
    if best == usize::MAX && dep.toolchain.is_none() {
        return Vec::new();
    }
    admissible.retain(|c| distance(c, recipe, all) == best);
    admissible
}

fn index_by_name(candidates: &[Candidate]) -> HashMap<&str, Vec<&Candidate>> {
    let mut by_name: HashMap<&str, Vec<&Candidate>> = HashMap::new();
    for candidate in candidates {
        by_name
            .entry(candidate.name.as_str())
            .or_default()
            .push(candidate);
    }
    by_name
}

/// The candidate [`graph_from_keys`] would pick for one dependency tuple.
fn resolve_dep<'a>(
    by_name: &HashMap<&str, Vec<&'a Candidate>>,
    recipe: &Candidate,
    dep: &DepReq,
    choice: Choice,
    all: &'a [Candidate],
) -> Option<&'a Candidate> {
    let named = by_name.get(dep.name.as_str())?;
    let admissible = within_generation(
        named
            .iter()
            .copied()
            .filter(|c| satisfies(c, dep))
            .collect(),
        recipe,
        all,
        dep,
    );
    choose(&admissible, choice)
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
        ordered.then_with(|| ModuleKey::of(b).cmp(&ModuleKey::of(a)))
    })
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
    let mut starts: Vec<ModuleKey> = Vec::new();
    for root in roots {
        let (name, version_req) = match root.split_once("==") {
            Some((name, version)) => (name, format!("=={version}")),
            None => (root.as_str(), String::new()),
        };
        let admissible: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| {
                c.name == name && (version_req.is_empty() || matches_req(&c.version, &version_req))
            })
            .collect();
        let start = match choose(&admissible, choice) {
            Some(picked) => picked,
            None => {
                let mut of_that_name: Vec<String> = candidates
                    .iter()
                    .filter(|c| c.name == name)
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
        starts.push(ModuleKey::of(start));
    }
    graph_from_keys(candidates, &starts, choice)
}

/// The build graph reachable from modules named by their full identity.
///
/// [`build_graph`] takes package names and picks one build of each; this takes
/// the builds themselves, for a caller that already knows which ones it means,
/// as an easystack does when it names easyconfig files. A key with no
/// candidate in the tree is a node with no dependencies.
#[allow(clippy::result_large_err)]
pub fn graph_from_keys(
    candidates: &[Candidate],
    starts: &[ModuleKey],
    choice: Choice,
) -> Result<BuildGraph, OrderError> {
    let mut graph: BuildGraph = DiGraph::new();
    let mut index: HashMap<ModuleKey, NodeIndex> = HashMap::new();
    let by_key: BTreeMap<ModuleKey, &Candidate> =
        candidates.iter().map(|c| (ModuleKey::of(c), c)).collect();
    let by_name = index_by_name(candidates);

    let node_for = |graph: &mut BuildGraph,
                    index: &mut HashMap<ModuleKey, NodeIndex>,
                    key: &ModuleKey|
     -> NodeIndex {
        *index
            .entry(key.clone())
            .or_insert_with(|| graph.add_node(key.clone()))
    };
    let mut queue: Vec<ModuleKey> = Vec::new();
    for key in starts {
        node_for(&mut graph, &mut index, key);
        queue.push(key.clone());
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
            if let Some(tc) = candidates.iter().find(|c| {
                c.name == candidate.toolchain.name && c.version == candidate.toolchain.version
            }) {
                let tc_key = ModuleKey::of(tc);
                if tc_key != key {
                    let node = node_for(&mut graph, &mut index, &tc_key);
                    graph.add_edge(node, dependent, Edge::Toolchain);
                    queue.push(tc_key);
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

        for (dep, kind) in deps {
            let Some(picked) = resolve_dep(&by_name, candidate, dep, choice, candidates) else {
                let mut available: Vec<String> = candidates
                    .iter()
                    .filter(|c| c.name == dep.name)
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

/// The build graph reachable from `starts`, including modules that are already
/// installed.
///
/// [`graph_from_keys`] returns on the first requirement nothing satisfies. A
/// deployed stack still has a graph when one recipe names a dependency the
/// tree cannot fill: that requirement is recorded and the walk continues.
/// Edge kinds are kept. Nothing here is treated as installed. The caller
/// decides which of these modules are on the machine.
pub fn reachable_graph(
    candidates: &[Candidate],
    starts: &[ModuleKey],
    choice: Choice,
) -> (BuildGraph, Vec<UnresolvedRequirement>) {
    let mut graph: BuildGraph = DiGraph::new();
    let mut index: HashMap<ModuleKey, NodeIndex> = HashMap::new();
    let by_key: BTreeMap<ModuleKey, &Candidate> =
        candidates.iter().map(|c| (ModuleKey::of(c), c)).collect();
    let by_name = index_by_name(candidates);
    let mut unresolved: BTreeSet<UnresolvedRequirement> = BTreeSet::new();

    let mut node_for = |graph: &mut BuildGraph, key: &ModuleKey| -> NodeIndex {
        *index
            .entry(key.clone())
            .or_insert_with(|| graph.add_node(key.clone()))
    };

    let mut queue: Vec<ModuleKey> = Vec::new();
    for key in starts {
        if by_key.contains_key(key) {
            node_for(&mut graph, key);
            queue.push(key.clone());
        } else {
            unresolved.insert(UnresolvedRequirement {
                from: key.clone(),
                requirement: key.to_string(),
            });
        }
    }

    let mut seen: HashSet<ModuleKey> = HashSet::new();
    while let Some(key) = queue.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(candidate) = by_key.get(&key) else {
            continue;
        };
        let dependent = node_for(&mut graph, &key);
        if !crate::hierarchy::is_system_toolchain(&candidate.toolchain) {
            if let Some(toolchain) =
                by_name
                    .get(candidate.toolchain.name.as_str())
                    .and_then(|named| {
                        named
                            .iter()
                            .copied()
                            .find(|item| item.version == candidate.toolchain.version)
                    })
            {
                let toolchain_key = ModuleKey::of(toolchain);
                if toolchain_key != key {
                    let node = node_for(&mut graph, &toolchain_key);
                    graph.add_edge(node, dependent, Edge::Toolchain);
                    queue.push(toolchain_key);
                }
            }
        }
        let mut deps: Vec<(&DepReq, Edge)> = candidate
            .dependencies
            .iter()
            .map(|dep| (dep, Edge::Runtime))
            .chain(
                candidate
                    .builddependencies
                    .iter()
                    .map(|dep| (dep, Edge::Build)),
            )
            .collect();
        deps.sort_by(|(left, _), (right, _)| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.version_req.cmp(&right.version_req))
        });
        for (dep, kind) in deps {
            match resolve_dep(&by_name, candidate, dep, choice, candidates) {
                Some(picked) => {
                    let dep_key = ModuleKey::of(picked);
                    let node = node_for(&mut graph, &dep_key);
                    graph.add_edge(node, dependent, kind);
                    queue.push(dep_key);
                }
                None => {
                    unresolved.insert(UnresolvedRequirement {
                        from: key.clone(),
                        requirement: format!("{} {}", dep.name, dep.version_req)
                            .trim()
                            .to_string(),
                    });
                }
            }
        }
    }
    (graph, unresolved.into_iter().collect())
}

/// Who uses each module in `graph`.
///
/// Edges in the graph run from a dependency to the module that needs it, so
/// the reverse of that edge is one entry here. A pair that has both a runtime
/// edge and a build edge is recorded once, as [`UseKind::Runtime`]. A
/// builddependency with no runtime edge is [`UseKind::BuildOnly`].
///
/// `exts_list` is not a dependency tuple, so it is not a graph edge. A bundle
/// whose extension name and version match a module in the graph gets an
/// [`UseKind::Extension`] entry from that module to the bundle. The toolchain
/// is the bundle's: an extension is built as part of the bundle.
pub fn reverse_index(
    graph: &BuildGraph,
    candidates: &[Candidate],
) -> BTreeMap<ModuleKey, Vec<ReverseUse>> {
    let mut flags: BTreeMap<(ModuleKey, ModuleKey), (bool, bool, bool)> = BTreeMap::new();
    for edge in graph.edge_references() {
        let dependency = graph[edge.source()].clone();
        let dependent = graph[edge.target()].clone();
        let slot = flags.entry((dependency, dependent)).or_default();
        match edge.weight() {
            Edge::Runtime => slot.0 = true,
            Edge::Build => slot.1 = true,
            Edge::Toolchain => slot.2 = true,
        }
    }

    let mut out: BTreeMap<ModuleKey, Vec<ReverseUse>> = BTreeMap::new();
    for node in graph.node_weights() {
        out.entry(node.clone()).or_default();
    }
    for ((dependency, dependent), (runtime, build, toolchain)) in flags {
        let slot = out.entry(dependency).or_default();
        if runtime {
            slot.push(ReverseUse {
                dependent: dependent.clone(),
                kind: UseKind::Runtime,
            });
        } else if build {
            slot.push(ReverseUse {
                dependent: dependent.clone(),
                kind: UseKind::BuildOnly,
            });
        }
        if toolchain {
            slot.push(ReverseUse {
                dependent,
                kind: UseKind::Toolchain,
            });
        }
    }

    let nodes: BTreeSet<ModuleKey> = graph.node_weights().cloned().collect();
    let mut by_name_version: BTreeMap<(&str, &str), Vec<ModuleKey>> = BTreeMap::new();
    for candidate in candidates {
        let key = ModuleKey::of(candidate);
        if nodes.contains(&key) {
            by_name_version
                .entry((candidate.name.as_str(), candidate.version.as_str()))
                .or_default()
                .push(key);
        }
    }
    for candidate in candidates {
        let bundle = ModuleKey::of(candidate);
        if !nodes.contains(&bundle) {
            continue;
        }
        for extension in &candidate.exts_list {
            if extension.name.is_empty() || extension.version.is_empty() {
                continue;
            }
            let Some(matches) =
                by_name_version.get(&(extension.name.as_str(), extension.version.as_str()))
            else {
                continue;
            };
            for key in matches {
                if key == &bundle {
                    continue;
                }
                out.entry(key.clone()).or_default().push(ReverseUse {
                    dependent: bundle.clone(),
                    kind: UseKind::Extension,
                });
            }
        }
    }
    for uses in out.values_mut() {
        uses.sort();
        uses.dedup();
    }
    out
}

/// The EasyBuild module name of a key: `name/version[-toolchain]versionsuffix`,
/// the way it appears under `modules/all` and `software/` in an install tree.
pub fn module_name(key: &ModuleKey) -> String {
    let toolchain = if key.toolchain == "system" {
        String::new()
    } else {
        format!("-{}", key.toolchain)
    };
    format!(
        "{}/{}{toolchain}{}",
        key.name, key.version, key.versionsuffix
    )
}

/// A requirement the closure walk found no easyconfig for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct UnresolvedRequirement {
    /// The module that asked for it.
    pub from: ModuleKey,
    /// The requirement as `name version`, or the whole key when the start
    /// itself has no easyconfig in the tree.
    pub requirement: String,
}

/// What a set of starts would make EasyBuild's robot build on a given stack.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledClosure {
    /// Modules not yet installed, in build order (dependencies first).
    pub to_build: Vec<ModuleKey>,
    /// Installed modules the walk reached and stopped at, sorted.
    pub installed: Vec<ModuleKey>,
    /// Requirements no easyconfig in the tree satisfies, sorted.
    pub unresolved: Vec<UnresolvedRequirement>,
}

/// Close `starts` over the tree the way `eb --robot` does against an
/// existing install: an installed module is a leaf, because its own
/// dependencies were installed with it, and everything reachable through
/// modules that are not installed has to be built.
///
/// Unlike [`graph_from_keys`] a requirement nothing satisfies does not abort
/// the walk. It is recorded and the walk goes on, so one missing recipe in one
/// pull request does not hide the rest of its build set.
pub fn closure_against_installed(
    candidates: &[Candidate],
    starts: &[ModuleKey],
    choice: Choice,
    is_installed: &dyn Fn(&ModuleKey) -> bool,
) -> InstalledClosure {
    let mut graph: BuildGraph = DiGraph::new();
    let mut index: HashMap<ModuleKey, NodeIndex> = HashMap::new();
    let by_key: BTreeMap<ModuleKey, &Candidate> =
        candidates.iter().map(|c| (ModuleKey::of(c), c)).collect();
    let by_name = index_by_name(candidates);
    let mut installed: BTreeSet<ModuleKey> = BTreeSet::new();
    let mut unresolved: BTreeSet<UnresolvedRequirement> = BTreeSet::new();

    let mut node_for = |graph: &mut BuildGraph, key: &ModuleKey| -> NodeIndex {
        *index
            .entry(key.clone())
            .or_insert_with(|| graph.add_node(key.clone()))
    };

    let mut queue: Vec<ModuleKey> = starts.to_vec();
    let mut seen: HashSet<ModuleKey> = HashSet::new();
    let mut edges: Vec<(ModuleKey, ModuleKey)> = Vec::new();
    while let Some(key) = queue.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if is_installed(&key) {
            installed.insert(key);
            continue;
        }
        node_for(&mut graph, &key);
        let Some(candidate) = by_key.get(&key) else {
            unresolved.insert(UnresolvedRequirement {
                from: key.clone(),
                requirement: key.to_string(),
            });
            continue;
        };
        if !crate::hierarchy::is_system_toolchain(&candidate.toolchain) {
            if let Some(tc) = candidates.iter().find(|c| {
                c.name == candidate.toolchain.name && c.version == candidate.toolchain.version
            }) {
                let tc_key = ModuleKey::of(tc);
                if tc_key != key {
                    edges.push((tc_key.clone(), key.clone()));
                    queue.push(tc_key);
                }
            }
        }
        let mut deps: Vec<&DepReq> = candidate
            .dependencies
            .iter()
            .chain(candidate.builddependencies.iter())
            .collect();
        deps.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.version_req.cmp(&b.version_req))
        });
        for dep in deps {
            match resolve_dep(&by_name, candidate, dep, choice, candidates) {
                Some(picked) => {
                    let dep_key = ModuleKey::of(picked);
                    edges.push((dep_key.clone(), key.clone()));
                    queue.push(dep_key);
                }
                None => {
                    unresolved.insert(UnresolvedRequirement {
                        from: key.clone(),
                        requirement: format!("{} {}", dep.name, dep.version_req)
                            .trim()
                            .to_string(),
                    });
                }
            }
        }
    }
    for (from, to) in edges {
        if installed.contains(&from) || installed.contains(&to) {
            continue;
        }
        let a = node_for(&mut graph, &from);
        let b = node_for(&mut graph, &to);
        graph.add_edge(a, b, Edge::Runtime);
    }
    // Build order when there is one; a cycle falls back to sorted keys so the
    // set is still reported, which is what an overlap check needs.
    let to_build = match petgraph::algo::toposort(&graph, None) {
        Ok(order) => order.into_iter().map(|n| graph[n].clone()).collect(),
        Err(_) => {
            let mut keys: Vec<ModuleKey> = graph.node_weights().cloned().collect();
            keys.sort();
            keys
        }
    };
    InstalledClosure {
        to_build,
        installed: installed.into_iter().collect(),
        unresolved: unresolved.into_iter().collect(),
    }
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
    let graph = build_graph(candidates, roots, choice)?;
    let by_key: BTreeMap<ModuleKey, &Candidate> =
        candidates.iter().map(|c| (ModuleKey::of(c), c)).collect();

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

    Ok(sorted
        .into_iter()
        .filter_map(|node| by_key.get(&graph[node]).map(|c| (*c).clone()))
        .collect())
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
        if c.easyconfig_path.is_empty() {
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
        seen.entry(c.name.clone())
            .or_default()
            .push(format!("{}-{}", key.version, key.toolchain));
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
    use crate::domain::{ExtEntry, Toolchain};

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
            easyblock: None,
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
        let order = build_order(&all, &["App".into()], Choice::Newest).expect("order");
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
        let order = build_order(&all, &["App".into()], Choice::Newest).expect("order");
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
    fn a_cycle_is_reported_with_the_path() {
        let all = vec![
            candidate("A", "1.0", tc("foss", "2026.1"), vec![dep("B", "", None)]),
            candidate("B", "1.0", tc("foss", "2026.1"), vec![dep("A", "", None)]),
        ];
        let err = build_order(&all, &["A".into()], Choice::Newest).unwrap_err();
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
        let err = build_order(&all, &["GROMAC".into()], Choice::Newest).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("GROMACS"), "{shown}");
        assert!(shown.contains("Did you mean"), "{shown}");
    }

    #[test]
    fn a_wrong_capital_is_still_found() {
        let all = vec![candidate("pkgconf", "2.5.1", tc("foss", "2026.1"), vec![])];
        let err = build_order(&all, &["PkgConf".into()], Choice::Newest).unwrap_err();
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
        let err = build_order(&all, &["GROMACS==99.0".into()], Choice::Newest).unwrap_err();
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
        let err = build_order(&all, &["App".into()], Choice::Newest).unwrap_err();
        let shown = err.to_string();
        assert!(shown.contains("Lib >=9"), "{shown}");
        assert!(shown.contains("2.0-foss-2026.1"), "{shown}");
    }

    #[test]
    fn a_long_list_of_versions_is_summarized_rather_than_dumped() {
        let all: Vec<Candidate> = (1..=12)
            .map(|n| candidate("Many", &format!("{n}.0"), tc("foss", "2026.1"), vec![]))
            .collect();
        let err = build_order(&all, &["Many==99".into()], Choice::Newest).unwrap_err();
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
        let err = build_order(&all, &["App".into()], Choice::Newest).unwrap_err();
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
        let first = names(&build_order(&all, &["App".into()], Choice::Newest).unwrap());
        all.reverse();
        let second = names(&build_order(&all, &["App".into()], Choice::Newest).unwrap());
        assert_eq!(first, second);
        // Newest wins by default; oldest is available for reproducing a tree.
        assert!(first.iter().any(|s| s.starts_with("Lib-2.1")), "{first:?}");
        let oldest = names(&build_order(&all, &["App".into()], Choice::Oldest).unwrap());
        assert!(
            oldest.iter().any(|s| s.starts_with("Lib-2.0")),
            "{oldest:?}"
        );
    }

    /// EasyBuild does not satisfy an unpinned dependency from an older
    /// generation. The only `ParMETIS` in a mixed tree is often `gompi-2022a`;
    /// walking it pulls that generation's GCCcore chain into a 2026 closure.
    #[test]
    fn an_unpinned_dependency_stays_inside_its_generation() {
        let old_chain = candidate("OldBase", "11.3.0", tc("GCCcore", "11.3.0"), vec![]);
        let old = candidate(
            "ParMETIS",
            "4.0.3",
            tc("gompi", "2022a"),
            vec![dep("OldBase", "", None)],
        );
        let cmake = candidate("CMake", "4.2.1", tc("GCCcore", "15.2.0"), vec![]);
        let app = candidate(
            "App",
            "1.0",
            tc("foss", "2026.1"),
            vec![
                dep("ParMETIS", "==4.0.3", None),
                dep("CMake", "==4.2.1", None),
            ],
        );
        let all = vec![app.clone(), old.clone(), old_chain, cmake];
        let closure =
            closure_against_installed(&all, &[ModuleKey::of(&app)], Choice::Newest, &|_| false);
        let built: Vec<String> = closure.to_build.iter().map(ToString::to_string).collect();
        assert!(
            built.iter().any(|s| s.starts_with("CMake-4.2.1")),
            "{built:?}"
        );
        assert!(
            built
                .iter()
                .all(|s| !s.contains("ParMETIS") && !s.contains("OldBase")),
            "{built:?}"
        );
        assert!(
            closure
                .unresolved
                .iter()
                .any(|u| u.requirement.contains("ParMETIS")),
            "{:?}",
            closure.unresolved
        );
        let err = build_order(&all, &["App".into()], Choice::Newest).unwrap_err();
        assert!(matches!(err, OrderError::Unsatisfied { .. }), "{err}");

        // A tuple that names the old toolchain is the one way out of the generation.
        let pinned = candidate(
            "Pinned",
            "1.0",
            tc("foss", "2026.1"),
            vec![dep("ParMETIS", "==4.0.3", Some(tc("gompi", "2022a")))],
        );
        let pinned_all = vec![pinned.clone(), old];
        let pinned_closure = closure_against_installed(
            &pinned_all,
            &[ModuleKey::of(&pinned)],
            Choice::Newest,
            &|_| false,
        );
        assert!(
            pinned_closure.to_build.iter().any(|k| k.name == "ParMETIS"),
            "{:?}",
            pinned_closure.to_build
        );
    }

    #[test]
    fn a_root_can_pin_its_own_version() {
        let all = vec![
            candidate("Lib", "2.0", tc("foss", "2026.1"), vec![]),
            candidate("Lib", "2.1", tc("foss", "2026.1"), vec![]),
        ];
        let order = build_order(&all, &["Lib==2.0".into()], Choice::Newest).unwrap();
        assert_eq!(names(&order), vec!["Lib-2.0-foss-2026.1".to_string()]);
    }

    #[test]
    fn the_reverse_index_keeps_runtime_build_extension_and_toolchain() {
        let gcc = candidate("GCCcore", "15.2.0", tc("system", "system"), vec![]);
        let zlib = candidate("zlib", "1.3.1", tc("GCCcore", "15.2.0"), vec![]);
        let mut linked = candidate(
            "Linked",
            "1.0",
            tc("GCCcore", "15.2.0"),
            vec![dep("zlib", "==1.3.1", None)],
        );
        linked.builddependencies.push(dep("zlib", "==1.3.1", None));
        let mut built = candidate("Built", "1.0", tc("GCCcore", "15.2.0"), vec![]);
        built.builddependencies.push(dep("zlib", "==1.3.1", None));
        let mut bundle = candidate("Bundle", "1.0", tc("GCCcore", "15.2.0"), vec![]);
        bundle.exts_list.push(ExtEntry {
            name: "zlib".into(),
            version: "1.3.1".into(),
        });
        let all = vec![gcc, zlib.clone(), linked, built, bundle];
        let (graph, unresolved) = reachable_graph(
            &all,
            &[
                ModuleKey::of(&all[2]),
                ModuleKey::of(&all[3]),
                ModuleKey::of(&all[4]),
            ],
            Choice::Newest,
        );
        assert!(unresolved.is_empty(), "{unresolved:?}");
        let reverse = reverse_index(&graph, &all);
        let uses = &reverse[&ModuleKey::of(&zlib)];
        let kinds: BTreeMap<String, UseKind> = uses
            .iter()
            .map(|use_| (use_.dependent.name.clone(), use_.kind))
            .collect();
        assert_eq!(
            kinds.get("Linked"),
            Some(&UseKind::Runtime),
            "a runtime edge wins over a build edge: {kinds:?}"
        );
        assert_eq!(kinds.get("Built"), Some(&UseKind::BuildOnly), "{kinds:?}");
        assert_eq!(kinds.get("Bundle"), Some(&UseKind::Extension), "{kinds:?}");
        assert!(
            !kinds.contains_key("GCCcore"),
            "the toolchain is not a user of zlib: {kinds:?}"
        );
        let toolchain_users = &reverse[&ModuleKey::of(&all[0])];
        assert!(
            toolchain_users
                .iter()
                .any(|use_| use_.dependent.name == "zlib" && use_.kind == UseKind::Toolchain),
            "{toolchain_users:?}"
        );
    }

    #[test]
    fn a_missing_dependency_does_not_stop_the_reachable_graph() {
        let app = candidate(
            "App",
            "1.0",
            tc("GCCcore", "15.2.0"),
            vec![dep("Missing", "==1", None), dep("Lib", "==1", None)],
        );
        let lib = candidate("Lib", "1", tc("GCCcore", "15.2.0"), vec![]);
        let (graph, unresolved) =
            reachable_graph(&[app.clone(), lib], &[ModuleKey::of(&app)], Choice::Newest);
        assert!(graph.node_weights().any(|key| key.name == "Lib"));
        assert!(unresolved
            .iter()
            .any(|item| item.requirement.contains("Missing")));
    }
}
