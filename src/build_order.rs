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

/// The build graph: an edge runs from a dependency to what needs it, so a
/// topological order is already the order to build in.
pub type BuildGraph = DiGraph<ModuleKey, Edge>;

/// Why an order could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    /// A named root matches no candidate in the tree.
    UnknownRoot(String),
    /// A requirement matches nothing, with the module that stated it.
    Unsatisfied {
        /// The module whose dependency could not be met.
        from: ModuleKey,
        /// The dependency as the recipe wrote it.
        requirement: String,
    },
    /// The graph has a cycle. Every module in the strongly connected
    /// component is named, not just the edge that happened to close it, since
    /// a bootstrap chain is broken by choosing where to cut the whole loop.
    Cycle(Vec<ModuleKey>),
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRoot(name) => write!(f, "no candidate named {name}"),
            Self::Unsatisfied { from, requirement } => {
                write!(
                    f,
                    "{from} requires {requirement}, which no candidate satisfies"
                )
            }
            Self::Cycle(component) => {
                let names: Vec<String> = component.iter().map(ToString::to_string).collect();
                write!(f, "dependency cycle among {}", names.join(", "))
            }
        }
    }
}

impl std::error::Error for OrderError {}

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

/// Whether a candidate satisfies one stated dependency.
///
/// The rules are EasyBuild's: a version requirement, an optional toolchain that
/// names one build and no other, and an optional versionsuffix.
fn satisfies(candidate: &Candidate, dep: &DepReq) -> bool {
    if candidate.name != dep.name {
        return false;
    }
    if !dep.version_req.is_empty() && !matches_req(&candidate.version, &dep.version_req) {
        return false;
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
pub fn build_graph(
    candidates: &[Candidate],
    roots: &[String],
    choice: Choice,
) -> Result<BuildGraph, OrderError> {
    let mut graph: BuildGraph = DiGraph::new();
    let mut index: HashMap<ModuleKey, NodeIndex> = HashMap::new();
    let mut queue: Vec<ModuleKey> = Vec::new();
    let by_key: BTreeMap<ModuleKey, &Candidate> =
        candidates.iter().map(|c| (ModuleKey::of(c), c)).collect();

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
        let admissible: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| {
                c.name == name && (version_req.is_empty() || matches_req(&c.version, &version_req))
            })
            .collect();
        let start =
            choose(&admissible, choice).ok_or_else(|| OrderError::UnknownRoot(root.clone()))?;
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
            let mut admissible: Vec<&Candidate> =
                candidates.iter().filter(|c| satisfies(c, dep)).collect();
            // Nearest generation first, then the choice function decides among
            // equals. A dependency that pins a toolchain was already narrowed
            // to that one build by `satisfies`.
            if let Some(best) = admissible
                .iter()
                .map(|c| distance(c, candidate, candidates))
                .min()
            {
                admissible.retain(|c| distance(c, candidate, candidates) == best);
            }
            let Some(picked) = choose(&admissible, choice) else {
                return Err(OrderError::Unsatisfied {
                    from: key.clone(),
                    requirement: format!("{} {}", dep.name, dep.version_req)
                        .trim()
                        .to_string(),
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
            OrderError::Unsatisfied { from, requirement } => {
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

    #[test]
    fn a_root_can_pin_its_own_version() {
        let all = vec![
            candidate("Lib", "2.0", tc("foss", "2026.1"), vec![]),
            candidate("Lib", "2.1", tc("foss", "2026.1"), vec![]),
        ];
        let order = build_order(&all, &["Lib==2.0".into()], Choice::Newest).unwrap();
        assert_eq!(names(&order), vec!["Lib-2.0-foss-2026.1".to_string()]);
    }
}
