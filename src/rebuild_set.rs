//! What an in-place easyconfig patch forces EESSI to rebuild.
//!
//! The patch keeps the filename and the version. The deployed stack is the
//! closure of its easystacks, resolved the same way as [`crate::pr_overlap`]:
//! the release easyconfigs, a `from-commit` tree ahead of them, filtered
//! dependencies dropped, installed modules read with
//! [`crate::pr_overlap::parse_installed`]. The reverse index is
//! [`crate::build_order::reverse_index`].
//!
//! Classification uses only that graph.
//!
//! * `no-rebuild`. Some path from the patched module to this one is made of
//!   runtime dependencies. The dependent links the patched package as a
//!   shared library. Replacing that library does not require a rebuild.
//! * `review`. Every path uses a builddependency, so the dependent may have
//!   linked the package statically or embedded it. The graph cannot say
//!   which.
//! * `must-rebuild`. The row is the patched easyconfig itself, or a bundle
//!   whose `exts_list` names the patched package and version. The extension
//!   is installed as part of the bundle.
//!
//! A toolchain edge is not one of these uses. Patching a compiler is outside
//! these three classes. A later `--check-static <prefix>` could read
//! installed binaries. This module does not.

use crate::build_order::{
    module_name, reachable_graph, reverse_index, Choice, ModuleKey, ReverseUse, UseKind,
};
use crate::dep_filter::FilterDeps;
use crate::domain::Candidate;
use crate::easystack::parse_easystack_entries;
use crate::eb_easyblock::encode_class_name;
use crate::eb_parse::merge_candidates_with_precedence;
use crate::pr_overlap::{eb_version_of, EasyblockCatalog};
use crate::version::cmp_version;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;

/// Schema of [`RebuildDocument`]. Bumped when a field changes meaning.
pub const REBUILD_SET_SCHEMA_VERSION: u32 = 1;

/// Class for a runtime dependency, direct or through other runtime dependencies.
pub const CLASS_NO_REBUILD: &str = "no-rebuild";
/// Class for a dependent reached only through builddependencies.
pub const CLASS_REVIEW: &str = "review";
/// Class for the patched easyconfig and for a bundle that ships it.
pub const CLASS_MUST_REBUILD: &str = "must-rebuild";

const EDGE_CHANGED: &str = "changed";

/// Why a deployed easystack could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RebuildError {
    /// The file is not an easystack document.
    #[error("{path}: {message}")]
    Easystack {
        /// Path that was passed in.
        path: String,
        /// Parser error.
        message: String,
    },
}

/// One deployed easystack file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackEasystack {
    /// Repository path or filesystem path, used for the EasyBuild version
    /// in the filename and for the report.
    pub path: String,
    /// File text.
    pub text: String,
}

/// Inputs for one `rebuild-set` run.
#[derive(Debug, Clone)]
pub struct RebuildQuery {
    /// Easyconfigs of the release the stack uses.
    pub base: Vec<Candidate>,
    /// Easyconfigs at a `from-commit`, keyed by commit. Each tree is put
    /// ahead of `base` for entries pinned to that commit.
    pub commits: BTreeMap<String, Vec<Candidate>>,
    /// Deployed easystacks. Every entry is a root of the graph.
    pub easystacks: Vec<StackEasystack>,
    /// Easyconfig filenames to treat as patched in place.
    pub changed: Vec<String>,
    /// Installed `Name/version` modules.
    pub installed: BTreeSet<String>,
    /// Dependencies dropped before the walk, EasyBuild `--filter-deps`.
    pub filter_deps: FilterDeps,
    /// When set, a missing easyblock class is recorded. It does not remove
    /// an installed module from the set.
    pub easyblocks: Option<EasyblockCatalog>,
    /// Label for the installed list, for example `x86_64/amd/zen4`.
    pub installed_source: String,
    /// RFC 3339 timestamp recorded in the report.
    pub generated_at: String,
}

/// One row of the rebuild table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebuildRow {
    /// Patched easyconfig filename.
    pub changed: String,
    /// Dependent module, `name/version[-toolchain]versionsuffix`.
    pub dependent: String,
    /// Easyconfig filename of `dependent`.
    pub dependent_easyconfig: String,
    /// `changed`, `runtime`, `builddependency`, or `extension`.
    pub edge_kind: String,
    /// `no-rebuild`, `review`, or `must-rebuild`.
    pub class: String,
    /// Why this class was chosen.
    pub reason: String,
    /// Newest EasyBuild version among the easystacks whose closure contains
    /// the dependent. Omitted when no easystack filename carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eb_version: Option<String>,
}

/// How many rows fell in each class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebuildCounts {
    /// Runtime dependencies.
    #[serde(rename = "no-rebuild")]
    pub no_rebuild: usize,
    /// Builddependency-only paths.
    pub review: usize,
    /// Patched easyconfigs and bundles that ship them.
    #[serde(rename = "must-rebuild")]
    pub must_rebuild: usize,
}

/// The rebuild set for one installed stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RebuildDocument {
    /// [`REBUILD_SET_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// When it was computed.
    pub generated_at: String,
    /// Where the installed list came from.
    pub installed_source: String,
    /// Easystack paths that were read.
    pub easystacks: Vec<String>,
    /// Patched easyconfig filenames, sorted.
    pub changed: Vec<String>,
    /// Changed filenames that are not in the deployed graph.
    pub missing: Vec<String>,
    /// Requirements no easyconfig satisfies, and easystack entries the robot
    /// path does not have.
    pub unresolved: Vec<String>,
    /// Missing easyblock classes, and `from-commit` pins whose tree was not
    /// given. Empty when there is nothing to say.
    pub notes: Vec<String>,
    /// Row counts.
    pub counts: RebuildCounts,
    /// One row per patched easyconfig and installed reverse dependent.
    pub rows: Vec<RebuildRow>,
}

/// A class `must-rebuild` easystack, one EasyBuild version per file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildEasystack {
    /// EasyBuild version taken from the deployed easystack filenames.
    pub eb_version: String,
    /// `YYYYMMDD-eb-<version>-cve-patches.yml`.
    pub filename: String,
    /// File body.
    pub body: String,
}

#[derive(Default)]
struct ModuleInfo {
    filename: String,
    easyblock: String,
    eb_versions: BTreeSet<String>,
}

struct Deployed {
    reverse: BTreeMap<ModuleKey, Vec<ReverseUse>>,
    modules: BTreeMap<ModuleKey, ModuleInfo>,
    unresolved: BTreeSet<String>,
    notes: BTreeSet<String>,
}

struct Root {
    file: String,
}

/// Close the deployed easystacks and classify `query.changed`.
pub fn rebuild_set(query: &RebuildQuery) -> Result<RebuildDocument, RebuildError> {
    let mut changed: Vec<String> = query
        .changed
        .iter()
        .map(|name| basename(name).to_string())
        .collect();
    changed.sort();
    changed.dedup();

    let Deployed {
        reverse,
        modules,
        unresolved,
        mut notes,
    } = deployed_graph(query)?;
    if let Some(catalog) = &query.easyblocks {
        if catalog.check {
            for info in modules.values() {
                if !easyblock_known(catalog, &info.easyblock) {
                    notes.insert(format!("easyblock {} missing", info.easyblock));
                }
            }
        }
    }

    let mut by_filename: BTreeMap<String, Vec<ModuleKey>> = BTreeMap::new();
    for (key, info) in &modules {
        by_filename
            .entry(info.filename.clone())
            .or_default()
            .push(key.clone());
    }

    let mut missing = Vec::new();
    let mut rows = Vec::new();
    for filename in &changed {
        let Some(keys) = by_filename.get(filename) else {
            missing.push(filename.clone());
            continue;
        };
        for key in keys {
            rows.extend(classify_one(
                filename,
                key,
                &reverse,
                &modules,
                &query.installed,
            ));
        }
    }
    rows.sort_by(|left, right| {
        left.changed
            .cmp(&right.changed)
            .then_with(|| left.dependent.cmp(&right.dependent))
            .then_with(|| left.edge_kind.cmp(&right.edge_kind))
    });
    rows.dedup();

    let counts = RebuildCounts {
        no_rebuild: rows
            .iter()
            .filter(|row| row.class == CLASS_NO_REBUILD)
            .count(),
        review: rows.iter().filter(|row| row.class == CLASS_REVIEW).count(),
        must_rebuild: rows
            .iter()
            .filter(|row| row.class == CLASS_MUST_REBUILD)
            .count(),
    };
    let mut easystacks: Vec<String> = query
        .easystacks
        .iter()
        .map(|item| item.path.clone())
        .collect();
    easystacks.sort();
    easystacks.dedup();
    Ok(RebuildDocument {
        schema_version: REBUILD_SET_SCHEMA_VERSION,
        generated_at: query.generated_at.clone(),
        installed_source: query.installed_source.clone(),
        easystacks,
        changed,
        missing,
        unresolved: unresolved.into_iter().collect(),
        notes: notes.into_iter().collect(),
        counts,
        rows,
    })
}

/// Markdown table of the rebuild set.
pub fn render_markdown(document: &RebuildDocument) -> String {
    let mut out = String::new();
    out.push_str("# Rebuild set\n\n");
    out.push_str(&format!(
        "Computed {} against the modules installed for `{}`.\n\n",
        document.generated_at, document.installed_source
    ));
    out.push_str(
        "Classes follow the easyconfig graph only. A runtime dependency needs no rebuild: \
         it links the patched package as a shared library. A builddependency only needs \
         review: it may have linked the package statically or embedded it. The patched \
         easyconfig, and a bundle that ships it as an extension, must be rebuilt.\n\n",
    );
    out.push_str(&format!(
        "{} must-rebuild, {} review, {} no-rebuild.\n\n",
        document.counts.must_rebuild, document.counts.review, document.counts.no_rebuild
    ));
    if !document.missing.is_empty() {
        out.push_str("Changed easyconfigs not in the deployed graph:\n\n");
        for name in &document.missing {
            out.push_str(&format!("- `{name}`\n"));
        }
        out.push('\n');
    }
    if !document.notes.is_empty() {
        for note in &document.notes {
            out.push_str(&format!("- note: {note}\n"));
        }
        out.push('\n');
    }
    out.push_str("| Changed easyconfig | Dependent | Edge kind | Class | Reason |\n");
    out.push_str("|---|---|---|---|---|\n");
    for row in &document.rows {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            row.changed,
            row.dependent,
            row.edge_kind,
            row.class,
            escape_cell(&row.reason),
        ));
    }
    out.push_str(
        "\nA later `--check-static <prefix>` could inspect installed binaries. This report does not.\n",
    );
    out
}

/// One easystack per EasyBuild version, containing class `must-rebuild` only.
///
/// The shape matches the files under `easystacks/software.eessi.io/2026.06/rebuilds/`:
/// a short comment, then `easyconfigs` entries with `rebuild: true`. Versions
/// with no such row produce no file. Entries are sorted by filename.
pub fn rebuild_easystacks(document: &RebuildDocument) -> Vec<RebuildEasystack> {
    let date = report_date(&document.generated_at);
    let mut by_version: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in &document.rows {
        if row.class != CLASS_MUST_REBUILD {
            continue;
        }
        let version = row
            .eb_version
            .clone()
            .filter(|version| !version.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        by_version
            .entry(version)
            .or_default()
            .insert(row.dependent_easyconfig.clone());
    }
    by_version
        .into_iter()
        .map(|(version, files)| RebuildEasystack {
            filename: format!("{date}-eb-{version}-cve-patches.yml"),
            body: render_easystack(&files),
            eb_version: version,
        })
        .collect()
}

fn render_easystack(files: &BTreeSet<String>) -> String {
    let mut body = String::from(
        "# In-place patches, same filename and version.\n\
         # Class must-rebuild only: the patched easyconfig, and a bundle that ships it as an extension.\n\
         # Generated by `eb-stack rebuild-set`.\n\
         easyconfigs:\n",
    );
    for file in files {
        body.push_str("  - ");
        body.push_str(file);
        body.push_str(":\n      options:\n        rebuild: true\n");
    }
    body
}

fn report_date(generated_at: &str) -> String {
    let date: String = generated_at
        .chars()
        .take(10)
        .filter(|ch| ch.is_ascii_digit())
        .collect();
    if date.len() == 8 {
        date
    } else {
        "unknown".to_string()
    }
}

fn deployed_graph(query: &RebuildQuery) -> Result<Deployed, RebuildError> {
    let mut groups: BTreeMap<(Option<String>, Option<String>), Vec<Root>> = BTreeMap::new();
    let mut unresolved = BTreeSet::new();
    let mut notes = BTreeSet::new();
    for source in &query.easystacks {
        let entries =
            parse_easystack_entries(&source.text).map_err(|error| RebuildError::Easystack {
                path: source.path.clone(),
                message: error.to_string(),
            })?;
        let eb_version = eb_version_of(&source.path);
        for entry in entries {
            groups
                .entry((entry.from_commit.clone(), eb_version.clone()))
                .or_default()
                .push(Root { file: entry.file });
        }
    }

    let mut reverse: BTreeMap<ModuleKey, Vec<ReverseUse>> = BTreeMap::new();
    let mut modules: BTreeMap<ModuleKey, ModuleInfo> = BTreeMap::new();
    for ((commit, eb_version), roots) in &groups {
        let mut universe = match commit {
            Some(commit) => match query.commits.get(commit) {
                Some(overlay) => {
                    merge_candidates_with_precedence(&[query.base.clone(), overlay.clone()])
                }
                None => {
                    notes.insert(format!(
                        "from-commit {commit}: no easyconfigs given for this commit, release tree used"
                    ));
                    query.base.clone()
                }
            },
            None => query.base.clone(),
        };
        query.filter_deps.apply(&mut universe);
        let by_file = index_filenames(&universe);
        let mut starts = Vec::new();
        for root in roots {
            match by_file.get(root.file.as_str()) {
                Some(candidate) => starts.push(ModuleKey::of(candidate)),
                None => {
                    unresolved.insert(format!(
                        "{}: no such easyconfig on the robot path",
                        root.file
                    ));
                }
            }
        }
        let (graph, missing) = reachable_graph(&universe, &starts, Choice::Newest);
        for item in missing {
            unresolved.insert(format!(
                "{} needs {}",
                module_name(&item.from),
                item.requirement
            ));
        }
        let index = reverse_index(&graph, &universe);
        for (key, uses) in index {
            let slot = reverse.entry(key).or_default();
            slot.extend(uses);
            slot.sort();
            slot.dedup();
        }
        let by_key: BTreeMap<ModuleKey, &Candidate> = universe
            .iter()
            .map(|candidate| (ModuleKey::of(candidate), candidate))
            .collect();
        for key in graph.node_weights() {
            let Some(candidate) = by_key.get(key) else {
                continue;
            };
            let info = modules.entry(key.clone()).or_default();
            if info.filename.is_empty() {
                info.filename = basename(&candidate.easyconfig_path).to_string();
                info.easyblock = candidate
                    .easyblock
                    .clone()
                    .unwrap_or_else(|| encode_class_name(&candidate.name));
            }
            if let Some(version) = eb_version {
                info.eb_versions.insert(version.clone());
            }
        }
    }
    Ok(Deployed {
        reverse,
        modules,
        unresolved,
        notes,
    })
}

fn index_filenames(candidates: &[Candidate]) -> BTreeMap<&str, &Candidate> {
    let mut out = BTreeMap::new();
    for candidate in candidates {
        out.insert(basename(&candidate.easyconfig_path), candidate);
    }
    out
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn easyblock_known(catalog: &EasyblockCatalog, class: &str) -> bool {
    if catalog.base.contains_key(class) {
        return true;
    }
    catalog
        .commits
        .values()
        .any(|index| index.contains_key(class))
}

#[derive(Default)]
struct Reach {
    reached: bool,
    runtime_clean: bool,
    direct_extension: bool,
    runtime_via: Vec<String>,
    other_via: Vec<String>,
}

#[derive(Default)]
struct Expanded {
    dirty: bool,
    clean: bool,
}

fn classify_one(
    changed_file: &str,
    changed: &ModuleKey,
    reverse: &BTreeMap<ModuleKey, Vec<ReverseUse>>,
    modules: &BTreeMap<ModuleKey, ModuleInfo>,
    installed: &BTreeSet<String>,
) -> Vec<RebuildRow> {
    let mut states: BTreeMap<ModuleKey, Reach> = BTreeMap::new();
    let mut expanded: BTreeMap<ModuleKey, Expanded> = BTreeMap::new();
    let mut queue: VecDeque<(ModuleKey, bool, Vec<String>)> = VecDeque::new();
    queue.push_back((changed.clone(), true, Vec::new()));

    while let Some((node, pass_runtime, via)) = queue.pop_front() {
        let flag = expanded.entry(node.clone()).or_default();
        if pass_runtime {
            if flag.clean {
                continue;
            }
            flag.clean = true;
            flag.dirty = true;
        } else if flag.dirty {
            continue;
        } else {
            flag.dirty = true;
        }
        let Some(uses) = reverse.get(&node) else {
            continue;
        };
        for use_ in uses {
            if matches!(use_.kind, UseKind::Toolchain) {
                continue;
            }
            if matches!(use_.kind, UseKind::Extension) && node != *changed {
                continue;
            }
            if use_.dependent == *changed || !installed.contains(&module_name(&use_.dependent)) {
                continue;
            }
            let child_via = if node == *changed {
                Vec::new()
            } else {
                let mut path = via.clone();
                path.push(module_name(&node));
                path
            };
            let state = states.entry(use_.dependent.clone()).or_default();
            let child_pass = match use_.kind {
                UseKind::Extension => {
                    state.direct_extension = true;
                    state.reached = true;
                    true
                }
                UseKind::Runtime => {
                    state.reached = true;
                    if pass_runtime && !state.runtime_clean {
                        state.runtime_clean = true;
                        state.runtime_via = child_via.clone();
                    }
                    if !pass_runtime && state.other_via.is_empty() {
                        state.other_via = child_via.clone();
                    }
                    pass_runtime
                }
                UseKind::BuildOnly => {
                    state.reached = true;
                    if state.other_via.is_empty() {
                        state.other_via = child_via.clone();
                    }
                    false
                }
                UseKind::Toolchain => false,
            };
            queue.push_back((use_.dependent.clone(), child_pass, child_via));
        }
    }

    let eb_version = modules
        .get(changed)
        .and_then(|info| newest_version(&info.eb_versions));
    let mut rows = vec![RebuildRow {
        changed: changed_file.to_string(),
        dependent: module_name(changed),
        dependent_easyconfig: changed_file.to_string(),
        edge_kind: EDGE_CHANGED.to_string(),
        class: CLASS_MUST_REBUILD.to_string(),
        reason: "patched easyconfig".to_string(),
        eb_version: eb_version.clone(),
    }];
    let mut dependents: Vec<_> = states.into_iter().collect();
    dependents.sort_by(|left, right| left.0.cmp(&right.0));
    for (key, state) in dependents {
        if !state.reached {
            continue;
        }
        let filename = modules
            .get(&key)
            .map(|info| info.filename.clone())
            .unwrap_or_default();
        let (edge_kind, class, reason) = if state.direct_extension {
            (
                UseKind::Extension.to_string(),
                CLASS_MUST_REBUILD,
                "the patched package is an extension in this bundle".to_string(),
            )
        } else if state.runtime_clean {
            (
                UseKind::Runtime.to_string(),
                CLASS_NO_REBUILD,
                reason_runtime(&state.runtime_via),
            )
        } else {
            (
                UseKind::BuildOnly.to_string(),
                CLASS_REVIEW,
                reason_build(&state.other_via),
            )
        };
        let version = modules
            .get(&key)
            .and_then(|info| newest_version(&info.eb_versions))
            .or_else(|| eb_version.clone());
        rows.push(RebuildRow {
            changed: changed_file.to_string(),
            dependent: module_name(&key),
            dependent_easyconfig: filename,
            edge_kind,
            class: class.to_string(),
            reason,
            eb_version: version,
        });
    }
    rows
}

fn newest_version(versions: &BTreeSet<String>) -> Option<String> {
    versions
        .iter()
        .max_by(|left, right| cmp_version(left, right))
        .cloned()
}

fn reason_runtime(via: &[String]) -> String {
    if via.is_empty() {
        "runtime dependency; links the patched package as a shared library".to_string()
    } else {
        format!(
            "runtime dependency through {}; links the patched package as a shared library",
            via.join(", ")
        )
    }
}

fn reason_build(via: &[String]) -> String {
    if via.is_empty() {
        "builddependency only; it may have linked the patched package statically or embedded it"
            .to_string()
    } else {
        format!(
            "builddependency only, through {}; it may have linked the patched package statically or embedded it",
            via.join(", ")
        )
    }
}

fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DepReq, ExtEntry, Toolchain};

    fn tc(name: &str, version: &str) -> Toolchain {
        Toolchain {
            name: name.into(),
            version: version.into(),
        }
    }

    fn dep(name: &str, version: &str) -> DepReq {
        DepReq {
            name: name.into(),
            version_req: format!("=={version}"),
            versionsuffix: None,
            toolchain: None,
        }
    }

    fn cand(
        name: &str,
        version: &str,
        toolchain: Toolchain,
        deps: Vec<DepReq>,
        build: Vec<DepReq>,
        extensions: Vec<(&str, &str)>,
    ) -> Candidate {
        let suffix = if toolchain.name == "system" {
            version.to_string()
        } else {
            format!("{version}-{}-{}", toolchain.name, toolchain.version)
        };
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain,
            versionsuffix: None,
            easyconfig_path: format!("x/{name}/{name}-{suffix}.eb"),
            dependencies: deps,
            builddependencies: build,
            exts_list: extensions
                .into_iter()
                .map(|(ext_name, ext_version)| ExtEntry {
                    name: ext_name.into(),
                    version: ext_version.into(),
                })
                .collect(),
            easyblock: Some("ConfigureMake".into()),
            moduleclass: None,
        }
    }

    fn stack() -> Vec<Candidate> {
        let gcc = tc("GCCcore", "15.2.0");
        let mut both = cand(
            "Both",
            "1.0",
            gcc.clone(),
            vec![dep("zlib", "1.3.1")],
            vec![],
            vec![],
        );
        both.builddependencies.push(dep("zlib", "1.3.1"));
        vec![
            cand(
                "GCCcore",
                "15.2.0",
                tc("system", "system"),
                vec![],
                vec![],
                vec![],
            ),
            cand("zlib", "1.3.1", gcc.clone(), vec![], vec![], vec![]),
            cand(
                "libpng",
                "1.6.50",
                gcc.clone(),
                vec![dep("zlib", "1.3.1")],
                vec![dep("CMake", "4.1.2")],
                vec![],
            ),
            cand(
                "CMake",
                "4.1.2",
                gcc.clone(),
                vec![],
                vec![dep("zlib", "1.3.1")],
                vec![],
            ),
            cand(
                "Graphviz",
                "14.0.2",
                gcc.clone(),
                vec![dep("libpng", "1.6.50")],
                vec![],
                vec![],
            ),
            cand(
                "AppStatic",
                "1.0",
                gcc.clone(),
                vec![],
                vec![dep("libpng", "1.6.50")],
                vec![],
            ),
            cand(
                "Bundle",
                "1.0",
                gcc.clone(),
                vec![],
                vec![],
                vec![("zlib", "1.3.1")],
            ),
            cand(
                "PyApp",
                "1.0",
                gcc.clone(),
                vec![dep("Bundle", "1.0")],
                vec![],
                vec![],
            ),
            cand(
                "NotInstalled",
                "1.0",
                gcc.clone(),
                vec![dep("zlib", "1.3.1")],
                vec![],
                vec![],
            ),
            cand(
                "Hidden",
                "1.0",
                gcc.clone(),
                vec![dep("NotInstalled", "1.0")],
                vec![],
                vec![],
            ),
            both,
        ]
    }

    fn installed(all: &[Candidate]) -> BTreeSet<String> {
        all.iter()
            .map(|candidate| module_name(&ModuleKey::of(candidate)))
            .filter(|name| !name.starts_with("NotInstalled/") && !name.starts_with("Hidden/"))
            .collect()
    }

    fn easystack(all: &[Candidate]) -> StackEasystack {
        let mut text = String::from("easyconfigs:\n");
        for candidate in all {
            text.push_str("  - ");
            text.push_str(basename(&candidate.easyconfig_path));
            text.push('\n');
        }
        StackEasystack {
            path: "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.0-2026.1.yml".into(),
            text,
        }
    }

    fn query(changed: &[&str]) -> RebuildQuery {
        let all = stack();
        RebuildQuery {
            installed: installed(&all),
            easystacks: vec![easystack(&all)],
            base: all,
            commits: BTreeMap::new(),
            changed: changed.iter().map(|name| (*name).to_string()).collect(),
            filter_deps: FilterDeps::empty(),
            easyblocks: None,
            installed_source: "x86_64/amd/zen4".into(),
            generated_at: "2026-10-22T12:00:00Z".into(),
        }
    }

    fn row<'a>(document: &'a RebuildDocument, dependent: &str) -> &'a RebuildRow {
        document
            .rows
            .iter()
            .find(|row| row.dependent == dependent)
            .unwrap_or_else(|| panic!("no row for {dependent}: {:?}", document.rows))
    }

    #[test]
    fn a_patched_library_is_classified_from_the_graph() {
        let document = rebuild_set(&query(&["zlib-1.3.1-GCCcore-15.2.0.eb"])).expect("rebuild set");
        assert!(document.missing.is_empty(), "{:?}", document.missing);
        assert!(document.unresolved.is_empty(), "{:?}", document.unresolved);

        let zlib = row(&document, "zlib/1.3.1-GCCcore-15.2.0");
        assert_eq!(zlib.class, CLASS_MUST_REBUILD);
        assert_eq!(zlib.edge_kind, "changed");
        assert_eq!(zlib.reason, "patched easyconfig");
        assert_eq!(zlib.eb_version.as_deref(), Some("5.4.0"));

        let libpng = row(&document, "libpng/1.6.50-GCCcore-15.2.0");
        assert_eq!(libpng.class, CLASS_NO_REBUILD);
        assert_eq!(libpng.edge_kind, "runtime");
        assert_eq!(
            libpng.reason,
            "runtime dependency; links the patched package as a shared library"
        );

        let graphviz = row(&document, "Graphviz/14.0.2-GCCcore-15.2.0");
        assert_eq!(graphviz.class, CLASS_NO_REBUILD);
        assert_eq!(
            graphviz.reason,
            "runtime dependency through libpng/1.6.50-GCCcore-15.2.0; links the patched package as a shared library"
        );

        let cmake = row(&document, "CMake/4.1.2-GCCcore-15.2.0");
        assert_eq!(cmake.class, CLASS_REVIEW);
        assert_eq!(cmake.edge_kind, "builddependency");
        assert_eq!(
            cmake.reason,
            "builddependency only; it may have linked the patched package statically or embedded it"
        );

        let app = row(&document, "AppStatic/1.0-GCCcore-15.2.0");
        assert_eq!(app.class, CLASS_REVIEW);
        assert_eq!(
            app.reason,
            "builddependency only, through libpng/1.6.50-GCCcore-15.2.0; it may have linked the patched package statically or embedded it"
        );

        let bundle = row(&document, "Bundle/1.0-GCCcore-15.2.0");
        assert_eq!(bundle.class, CLASS_MUST_REBUILD);
        assert_eq!(bundle.edge_kind, "extension");
        assert_eq!(
            bundle.reason,
            "the patched package is an extension in this bundle"
        );

        let pyapp = row(&document, "PyApp/1.0-GCCcore-15.2.0");
        assert_eq!(pyapp.class, CLASS_NO_REBUILD);
        assert_eq!(
            pyapp.reason,
            "runtime dependency through Bundle/1.0-GCCcore-15.2.0; links the patched package as a shared library"
        );

        let both = row(&document, "Both/1.0-GCCcore-15.2.0");
        assert_eq!(both.class, CLASS_NO_REBUILD);
        assert_eq!(both.edge_kind, "runtime");

        let names: Vec<&str> = document
            .rows
            .iter()
            .map(|row| row.dependent.as_str())
            .collect();
        assert!(
            !names.iter().any(|name| name.starts_with("NotInstalled/")),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.starts_with("Hidden/")),
            "{names:?}"
        );
        assert!(!names.contains(&"GCCcore/15.2.0"), "{names:?}");
        assert_eq!(document.counts.must_rebuild, 2);
        assert_eq!(document.counts.review, 2);
        assert_eq!(document.counts.no_rebuild, 4);

        let stacks = rebuild_easystacks(&document);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].filename, "20261022-eb-5.4.0-cve-patches.yml");
        assert!(stacks[0].body.contains(
            "  - Bundle-1.0-GCCcore-15.2.0.eb:\n      options:\n        rebuild: true\n"
        ));
        assert!(stacks[0].body.contains(
            "  - zlib-1.3.1-GCCcore-15.2.0.eb:\n      options:\n        rebuild: true\n"
        ));
        assert!(!stacks[0].body.contains("libpng"));
        assert!(stacks[0].body.find("Bundle-").unwrap() < stacks[0].body.find("zlib-").unwrap());

        let markdown = render_markdown(&document);
        assert!(
            markdown.contains("| Changed easyconfig | Dependent | Edge kind | Class | Reason |")
        );
        assert!(markdown.contains("`zlib-1.3.1-GCCcore-15.2.0.eb`"));
        assert!(markdown.contains("2 must-rebuild, 2 review, 4 no-rebuild."));
        assert!(markdown.contains("--check-static"));
    }

    #[test]
    fn a_filtered_dependency_is_not_a_reverse_edge() {
        let mut query = query(&["zlib-1.3.1-GCCcore-15.2.0.eb"]);
        query.filter_deps = FilterDeps::parse(&["zlib".to_string()]).expect("filter");
        let document = rebuild_set(&query).expect("rebuild set");
        let names: Vec<&str> = document
            .rows
            .iter()
            .map(|row| row.dependent.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Bundle/1.0-GCCcore-15.2.0",
                "PyApp/1.0-GCCcore-15.2.0",
                "zlib/1.3.1-GCCcore-15.2.0",
            ],
            "filter-deps drops dependency tuples and leaves exts_list: {names:?}"
        );
        assert_eq!(
            row(&document, "PyApp/1.0-GCCcore-15.2.0").class,
            CLASS_NO_REBUILD
        );
    }

    #[test]
    fn a_changed_file_outside_the_deployed_graph_is_missing() {
        let document = rebuild_set(&query(&["Absent-1.0.eb"])).expect("rebuild set");
        assert_eq!(document.missing, ["Absent-1.0.eb"]);
        assert!(document.rows.is_empty());
        assert!(rebuild_easystacks(&document).is_empty());
    }

    #[test]
    fn versions_pick_the_newest() {
        assert_eq!(
            newest_version(&BTreeSet::from([
                "5.3.1".into(),
                "5.4.0".into(),
                "5.4.1".into()
            ])),
            Some("5.4.1".into())
        );
    }
}
