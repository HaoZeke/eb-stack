//! What a module was built from, as one hash over its whole input closure.
//!
//! A version number says what a recipe is called, not what it was built from.
//! Two stacks can name the same versions and still differ, because a patch
//! changed, or a dependency two levels down moved. Functional deployment
//! answers this by identifying a build with a hash over its inputs rather than
//! with its version, so an identical hash means an identical build
//! (doi:10.1017/s0956796810000195), and it is the same property the
//! reproducible-builds work measures across a distribution
//! (doi:10.1109/ms.2021.3073045, doi:10.1109/msr66628.2025.00115).
//!
//! The hash here is over the easyconfig's own bytes and the hashes of
//! everything it needs. Recipe bytes are the honest input because everything a
//! build depends on that is not a dependency is stated in that file: sources,
//! checksums, patches, configure options, the toolchain line. Change any of
//! them and the hash changes. Change a dependency and every dependent's hash
//! changes with it, which is what makes the answer transitive rather than
//! local.
//!
//! What this is for: saying whether two plans are the same plan, and which
//! modules a change actually forces a rebuild of.

use crate::build_order::{BuildGraph, ModuleKey};
use petgraph::visit::EdgeRef;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// Version marker in every hash, so a change to what goes into a hash cannot
/// be mistaken for a change to the software.
const SCHEME: &str = "eb-stack-input-v1";

/// The input hash of one module, and what went into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputHash {
    /// Hex sha256 over the whole input closure.
    pub hash: String,
    /// Hex sha256 of the easyconfig file, or `None` when it could not be read.
    pub recipe: Option<String>,
    /// Whether every input was known. A recipe that could not be read leaves
    /// the hash defined but not trustworthy, and saying so is the difference
    /// between a plan that can be compared and one that only looks like it.
    pub complete: bool,
}

/// A digest as lowercase hex, the way every checksum in this crate is written.
fn hex(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut out, "{byte:02x}").expect("writing a digest to String cannot fail");
    }
    out
}

/// Hash the bytes of one easyconfig.
fn recipe_digest(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let bytes = std::fs::read(Path::new(path)).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(hex(hasher.finalize().as_slice()))
}

/// Input hashes for every module in a build graph.
///
/// The graph must be acyclic, which is what [`crate::build_order::build_order`]
/// has already established by the time it hands one over; `order` supplies the
/// topological sequence so a module is hashed after everything it needs.
pub fn input_hashes(
    graph: &BuildGraph,
    order: &[ModuleKey],
    recipe_paths: &BTreeMap<ModuleKey, String>,
) -> BTreeMap<ModuleKey, InputHash> {
    let mut out: BTreeMap<ModuleKey, InputHash> = BTreeMap::new();

    // node index by key, so edges can be walked without a second traversal
    let mut node_of = BTreeMap::new();
    for index in graph.node_indices() {
        node_of.insert(graph[index].clone(), index);
    }

    for key in order {
        let Some(&node) = node_of.get(key) else {
            continue;
        };
        // Incoming edges run from dependency to dependent, so these are the
        // inputs, and they are already hashed because the order says so.
        let mut inputs: Vec<String> = graph
            .edges_directed(node, petgraph::Direction::Incoming)
            .map(|edge| graph[edge.source()].clone())
            .map(|dep| {
                out.get(&dep)
                    .map(|h| format!("{dep} {}", h.hash))
                    // A dependency with no hash yet can only happen if the
                    // order is not a topological one, and pretending otherwise
                    // would produce a hash that silently means nothing.
                    .unwrap_or_else(|| format!("{dep} UNRESOLVED"))
            })
            .collect();
        inputs.sort();

        let recipe = recipe_paths.get(key).and_then(|p| recipe_digest(p));
        let deps_complete = inputs.iter().all(|i| !i.ends_with("UNRESOLVED"))
            && graph
                .edges_directed(node, petgraph::Direction::Incoming)
                .all(|edge| {
                    out.get(&graph[edge.source()])
                        .map(|h| h.complete)
                        .unwrap_or(false)
                });

        let mut hasher = Sha256::new();
        hasher.update(SCHEME.as_bytes());
        hasher.update(b"\nmodule\n");
        hasher.update(key.to_string().as_bytes());
        hasher.update(b"\nrecipe\n");
        hasher.update(recipe.as_deref().unwrap_or("UNREADABLE").as_bytes());
        for input in &inputs {
            hasher.update(b"\ninput\n");
            hasher.update(input.as_bytes());
        }
        out.insert(
            key.clone(),
            InputHash {
                hash: hex(hasher.finalize().as_slice()),
                complete: recipe.is_some() && deps_complete,
                recipe,
            },
        );
    }
    out
}

/// What changed between two sets of input hashes, and therefore what has to be
/// rebuilt.
///
/// A module is listed when its own hash differs, which by construction covers
/// everything downstream of a change as well.
pub fn changed(
    before: &BTreeMap<ModuleKey, InputHash>,
    after: &BTreeMap<ModuleKey, InputHash>,
) -> Vec<ModuleKey> {
    let mut out: Vec<ModuleKey> = Vec::new();
    for (key, now) in after {
        match before.get(key) {
            Some(then) if then.hash == now.hash => {}
            _ => out.push(key.clone()),
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_order::{build_graph, build_order, Choice};
    use crate::domain::{Candidate, DepReq, Toolchain};
    use std::fs;

    fn tc(name: &str, version: &str) -> Toolchain {
        Toolchain {
            name: name.into(),
            version: version.into(),
        }
    }

    /// Two recipes on disk, so the hash has real bytes to read.
    fn tree(dir: &Path, lib_body: &str) -> Vec<Candidate> {
        fs::create_dir_all(dir.join("l/Lib")).unwrap();
        fs::create_dir_all(dir.join("a/App")).unwrap();
        let lib = dir.join("l/Lib/Lib-1.0-foss-2026.1.eb");
        let app = dir.join("a/App/App-2.0-foss-2026.1.eb");
        fs::write(&lib, lib_body).unwrap();
        fs::write(&app, "name = 'App'\n").unwrap();
        vec![
            Candidate {
                name: "Lib".into(),
                version: "1.0".into(),
                toolchain: tc("foss", "2026.1"),
                versionsuffix: None,
                dependencies: Vec::new(),
                builddependencies: Vec::new(),
                easyconfig_path: lib.to_string_lossy().into_owned(),
                exts_list: Vec::new(),
                moduleclass: None,
            },
            Candidate {
                name: "App".into(),
                version: "2.0".into(),
                toolchain: tc("foss", "2026.1"),
                versionsuffix: None,
                dependencies: vec![DepReq {
                    name: "Lib".into(),
                    version_req: String::new(),
                    toolchain: None,
                    versionsuffix: None,
                }],
                builddependencies: Vec::new(),
                easyconfig_path: app.to_string_lossy().into_owned(),
                exts_list: Vec::new(),
                moduleclass: None,
            },
        ]
    }

    fn hashes_for(candidates: &[Candidate]) -> BTreeMap<ModuleKey, InputHash> {
        let graph = build_graph(candidates, &["App".into()], Choice::Newest).unwrap();
        let order: Vec<ModuleKey> = build_order(candidates, &["App".into()], Choice::Newest)
            .unwrap()
            .iter()
            .map(ModuleKey::of)
            .collect();
        let paths: BTreeMap<ModuleKey, String> = candidates
            .iter()
            .map(|c| (ModuleKey::of(c), c.easyconfig_path.clone()))
            .collect();
        input_hashes(&graph, &order, &paths)
    }

    #[test]
    fn the_same_inputs_give_the_same_hash() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let a = hashes_for(&tree(first.path(), "name = 'Lib'\nversion = '1.0'\n"));
        let b = hashes_for(&tree(second.path(), "name = 'Lib'\nversion = '1.0'\n"));
        let key = |m: &BTreeMap<ModuleKey, InputHash>| {
            m.iter()
                .find(|(k, _)| k.name == "App")
                .map(|(_, h)| h.hash.clone())
                .unwrap()
        };
        assert_eq!(key(&a), key(&b), "identical inputs, different directories");
    }

    /// The property that makes the hash worth having: a change deep in the
    /// graph reaches everything built on top of it.
    #[test]
    fn changing_a_dependency_changes_what_depends_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let before = hashes_for(&tree(dir.path(), "name = 'Lib'\nversion = '1.0'\n"));
        // Same version, different recipe: a patch added, a checksum corrected.
        let after = hashes_for(&tree(
            dir.path(),
            "name = 'Lib'\nversion = '1.0'\npatches = ['fix.patch']\n",
        ));

        let app = |m: &BTreeMap<ModuleKey, InputHash>| {
            m.iter()
                .find(|(k, _)| k.name == "App")
                .map(|(_, h)| h.hash.clone())
                .unwrap()
        };
        assert_ne!(
            app(&before),
            app(&after),
            "a changed dependency must reach its dependents"
        );
        let moved = changed(&before, &after);
        let names: Vec<String> = moved.iter().map(|k| k.name.clone()).collect();
        assert!(names.contains(&"Lib".to_string()), "{names:?}");
        assert!(names.contains(&"App".to_string()), "{names:?}");
    }

    #[test]
    fn nothing_changes_when_nothing_changes() {
        let dir = tempfile::tempdir().unwrap();
        let body = "name = 'Lib'\nversion = '1.0'\n";
        let before = hashes_for(&tree(dir.path(), body));
        let after = hashes_for(&tree(dir.path(), body));
        assert!(changed(&before, &after).is_empty());
    }

    #[test]
    fn a_recipe_that_cannot_be_read_is_hashed_but_not_called_complete() {
        let dir = tempfile::tempdir().unwrap();
        let mut candidates = tree(dir.path(), "name = 'Lib'\nversion = '1.0'\n");
        candidates[0].easyconfig_path = "/nowhere/Lib-1.0.eb".into();
        let hashes = hashes_for(&candidates);
        let lib = hashes.iter().find(|(k, _)| k.name == "Lib").unwrap().1;
        let app = hashes.iter().find(|(k, _)| k.name == "App").unwrap().1;
        assert!(!lib.complete, "an unreadable recipe is not a known input");
        assert!(
            !app.complete,
            "incompleteness has to travel with the closure"
        );
        assert!(!lib.hash.is_empty());
    }
}
