//! Backport list for one stack lock.
//!
//! A locked module is on the list when an advisory matches its version and
//! its reach class is `cross-user` or `network-facing`. `local-only`,
//! `not-reachable`, and `unclassified` are recorded and left off the list.
//! Blast radius is the SBOM closure: the module and every locked module that
//! depends on it through a runtime or build edge.

use crate::cli_parse::usage;
use crate::domain::{LockPackage, StackLock};
use crate::findings::{
    justifies_backport, reach_key, ReachTable, REACH_CROSS_USER, REACH_LOCAL, REACH_NETWORK,
    REACH_NOT_REACHABLE,
};
use crate::osv::{load_recorded_osv, resolve_vulnerability, ResolvedComponent};
use crate::registry::{RegistryClient, UreqClient};
use crate::sbom::{closure_edges_for_lock, package_identity};
use crate::version::matches_req;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::Path;

/// Schema of [`BackportDocument`].
pub const BACKPORT_LIST_SCHEMA_VERSION: u32 = 1;

/// What `stack sbom --backport-list` reads.
pub struct BackportRequest<'a> {
    /// Stack lock.
    pub lock: &'a StackLock,
    /// Path the lock was read from. Relative recipe paths resolve beside it.
    pub lock_path: &'a Path,
    /// Advisory ids. Empty means every id in [`Self::osv_cache`].
    pub cves: &'a [String],
    /// Recorded OSV responses. Absent means a live fetch of [`Self::cves`].
    pub osv_cache: Option<&'a Path>,
    /// Site file of `name class` lines. Absent uses the built-in names.
    pub reachability: Option<&'a Path>,
}

/// One locked module an advisory matched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackportRow {
    /// `Name/version-toolchain`, plus the versionsuffix when the lock has one.
    pub module: String,
    /// Package name.
    pub name: String,
    /// Locked version.
    pub version: String,
    /// Reach class.
    pub reach: String,
    /// Advisory id.
    pub advisory: String,
    /// Upper bounds from a less-than clause. A less-than-or-equal bound is last-affected, not a fix.
    pub fixed_in: Vec<String>,
    /// The module plus every locked module that depends on it.
    pub blast_radius: usize,
    /// Those module labels, sorted.
    pub blast: Vec<String>,
    /// `true` when [`justifies_backport`] is true for [`Self::reach`].
    pub on_list: bool,
}

/// The backport list for one lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackportDocument {
    /// [`BACKPORT_LIST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Lock path, as it was passed.
    pub lock: String,
    /// Toolchain label of the lock.
    pub generation: String,
    /// Advisory ids that were resolved, sorted.
    pub advisories: Vec<String>,
    /// Packages in the lock.
    pub stack_modules: usize,
    /// Rows with [`BackportRow::on_list`].
    pub listed: usize,
    /// Rows left off the list.
    pub recorded: usize,
    /// Matching modules. On-list rows come first.
    pub rows: Vec<BackportRow>,
    /// Advisories that name a package the lock lacks, a version outside the
    /// range, or a range that did not parse.
    pub notes: Vec<String>,
}

/// Resolve the advisories and classify every matching locked module.
pub fn backport_list(request: &BackportRequest<'_>) -> Result<BackportDocument> {
    let ids = advisory_ids(request.cves, request.osv_cache)?;
    let resolutions = resolve_ids(&ids, request.osv_cache)?;
    let reach = load_reach(request.reachability)?;
    let blasts = blast_sets(request.lock, request.lock_path);
    let mut rows = Vec::new();
    let mut notes = BTreeSet::new();
    for resolution in &resolutions {
        for component in &resolution.components {
            classify_component(
                request.lock,
                &resolution.id,
                component,
                &reach,
                &blasts,
                &mut rows,
                &mut notes,
            );
        }
        for item in &resolution.unresolved {
            notes.insert(format!("{}: {item}", resolution.id));
        }
    }
    rows.sort_by(|left, right| {
        right
            .on_list
            .cmp(&left.on_list)
            .then(rank(&left.reach).cmp(&rank(&right.reach)))
            .then(right.blast_radius.cmp(&left.blast_radius))
            .then(left.module.cmp(&right.module))
            .then(left.advisory.cmp(&right.advisory))
    });
    let listed = rows.iter().filter(|row| row.on_list).count();
    Ok(BackportDocument {
        schema_version: BACKPORT_LIST_SCHEMA_VERSION,
        lock: request.lock_path.display().to_string(),
        generation: generation_of(request.lock),
        advisories: ids,
        stack_modules: request.lock.packages.len(),
        listed,
        recorded: rows.len() - listed,
        rows,
        notes: notes.into_iter().collect(),
    })
}

/// Markdown for [`BackportDocument`].
pub fn render_markdown(document: &BackportDocument) -> String {
    let mut out = String::from("# Backport list\n\n");
    out.push_str(&format!(
        "Lock `{}`. Generation `{}`. {} modules.\n\n",
        document.lock, document.generation, document.stack_modules
    ));
    out.push_str(
        "A module is on this list when its reach class is `cross-user` or \
         `network-facing`. Those two classes justify a backport. Blast radius \
         is the SBOM closure: the module and every locked module that depends \
         on it through a runtime or build edge.\n\n",
    );
    if !document.advisories.is_empty() {
        let ids = document
            .advisories
            .iter()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("Advisories: {ids}.\n\n"));
    }
    out.push_str("## Backport\n\n");
    write_rows(
        &mut out,
        document.rows.iter().filter(|row| row.on_list),
        "No module on this lock is `cross-user` or `network-facing`.\n",
    );
    out.push_str("\n## Recorded\n\n");
    out.push_str(
        "These modules stay off the backport list. A backport is cross-user or network-facing.\n\n",
    );
    write_rows(
        &mut out,
        document.rows.iter().filter(|row| !row.on_list),
        "No module was recorded off the list.\n",
    );
    if !document.notes.is_empty() {
        out.push_str("\n## Notes\n\n");
        for note in &document.notes {
            out.push_str(&format!("- {note}\n"));
        }
    }
    out
}

/// Write the markdown. A path ending in `.md` also gets a sibling `.json`.
pub fn write_backport(path: &Path, document: &BackportDocument) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
    }
    std::fs::write(path, render_markdown(document))
        .with_context(|| format!("write {}", path.display()))?;
    if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
        crate::write_json_pretty(&path.with_extension("json"), document)?;
    }
    Ok(())
}

fn write_rows<'a>(out: &mut String, rows: impl Iterator<Item = &'a BackportRow>, empty: &str) {
    let rows: Vec<&BackportRow> = rows.collect();
    if rows.is_empty() {
        out.push_str(empty);
        return;
    }
    out.push_str("| Module | Reach | Advisory | Fixed in | Blast radius |\n");
    out.push_str("|---|---|---|---|---|\n");
    for row in rows {
        let fixed = if row.fixed_in.is_empty() {
            "-".to_string()
        } else {
            row.fixed_in.join(", ")
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            row.module, row.reach, row.advisory, fixed, row.blast_radius
        ));
    }
}

fn classify_component(
    lock: &StackLock,
    id: &str,
    component: &ResolvedComponent,
    reach: &ReachTable,
    blasts: &HashMap<String, BTreeSet<String>>,
    rows: &mut Vec<BackportRow>,
    notes: &mut BTreeSet<String>,
) {
    let named: Vec<&LockPackage> = lock
        .packages
        .iter()
        .filter(|package| reach_key(&package.name) == reach_key(&component.name))
        .collect();
    if named.is_empty() {
        notes.insert(format!(
            "{id} names {}, which this lock does not have",
            component.name
        ));
        return;
    }
    if component.version_reqs.is_empty() && component.versions.is_empty() {
        notes.insert(format!(
            "{id} names {}, but its ranges were not parsed",
            component.name
        ));
        return;
    }
    let hits: Vec<&LockPackage> = named
        .iter()
        .copied()
        .filter(|package| version_matches(package, component))
        .collect();
    if hits.is_empty() {
        let present = named
            .iter()
            .map(|package| module_label(package))
            .collect::<Vec<_>>()
            .join(", ");
        let verb = if named.len() == 1 { "is" } else { "are" };
        notes.insert(format!(
            "{present} {verb} not in range for {id} ({})",
            range_detail(component)
        ));
        return;
    }
    let fixed_in = fixed_bounds(&component.version_reqs);
    for package in hits {
        let identity = package_identity(package);
        let blast = blasts.get(&identity).cloned().unwrap_or_default();
        let class = reach.class_of(&package.name, &[&component.name, &package.name]);
        rows.push(BackportRow {
            module: module_label(package),
            name: package.name.clone(),
            version: package.version.clone(),
            reach: class.clone(),
            advisory: id.to_string(),
            fixed_in: fixed_in.clone(),
            blast_radius: blast.len().max(1),
            blast: blast.into_iter().collect(),
            on_list: justifies_backport(&class),
        });
    }
}

fn version_matches(package: &LockPackage, component: &ResolvedComponent) -> bool {
    component
        .versions
        .iter()
        .any(|version| version == &package.version)
        || component
            .version_reqs
            .iter()
            .any(|req| matches_req(&package.version, req))
}

fn range_detail(component: &ResolvedComponent) -> String {
    let mut parts = component.version_reqs.clone();
    for version in &component.versions {
        parts.push(format!("version {version}"));
    }
    parts.join(" or ")
}

/// Upper bounds introduced by a less-than clause. Less-than-or-equal is last-affected, not a fix.
fn fixed_bounds(reqs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for req in reqs {
        for clause in req.split(',') {
            let Some(bound) = clause.trim().strip_prefix('<') else {
                continue;
            };
            if bound.is_empty() || bound.starts_with('=') {
                continue;
            }
            out.push(bound.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn rank(class: &str) -> u8 {
    match class {
        REACH_CROSS_USER => 0,
        REACH_NETWORK => 1,
        REACH_LOCAL => 2,
        REACH_NOT_REACHABLE => 3,
        _ => 4,
    }
}

fn generation_of(lock: &StackLock) -> String {
    lock.generation_label
        .clone()
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| lock.toolchain.label())
}

fn module_label(package: &LockPackage) -> String {
    let mut label = format!(
        "{}/{}-{}",
        package.name,
        package.version,
        package.toolchain.label()
    );
    if let Some(suffix) = package
        .versionsuffix
        .as_deref()
        .filter(|suffix| !suffix.is_empty())
    {
        label.push_str(suffix);
    }
    label
}

fn advisory_ids(cves: &[String], cache: Option<&Path>) -> Result<Vec<String>> {
    if !cves.is_empty() {
        let mut ids = cves.to_vec();
        ids.sort();
        ids.dedup();
        return Ok(ids);
    }
    let Some(dir) = cache else {
        bail!("--backport-list needs --cve or --osv-cache");
    };
    let manifest_path = dir.join("queries.json");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parse {}", manifest_path.display()))?;
    let queries = manifest
        .get("queries")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow!("{} has no queries", manifest_path.display()))?;
    let mut ids = Vec::new();
    for query in queries {
        let url = query
            .get("url")
            .and_then(|value| value.as_str())
            .ok_or_else(|| anyhow!("{} query has no url", manifest_path.display()))?;
        let id = url
            .rsplit('/')
            .next()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("{url} has no advisory id"))?;
        ids.push(id.to_string());
    }
    if ids.is_empty() {
        bail!("{} names no advisory", manifest_path.display());
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn resolve_ids(ids: &[String], cache: Option<&Path>) -> Result<Vec<crate::osv::OsvResolution>> {
    let recorded = match cache {
        Some(dir) => Some(load_recorded_osv(dir).map_err(|error| anyhow!(error))?),
        None => None,
    };
    let live = UreqClient;
    let client: &dyn RegistryClient = match &recorded {
        Some(map) => map,
        None => &live,
    };
    let mut out = Vec::new();
    for id in ids {
        out.push(resolve_vulnerability(id, client).map_err(|error| anyhow!(error))?);
    }
    Ok(out)
}

fn load_reach(path: Option<&Path>) -> Result<ReachTable> {
    let Some(path) = path else {
        return Ok(ReachTable::builtin());
    };
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("--reachability {}: not readable", path.display()))?;
    ReachTable::builtin()
        .overlay(&text, path.display().to_string())
        .map_err(|error| usage(format!("--reachability {}: {error}", path.display())))
}

fn blast_sets(lock: &StackLock, lock_path: &Path) -> HashMap<String, BTreeSet<String>> {
    let edges = closure_edges_for_lock(lock, lock_path);
    let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, deps) in &edges {
        for dep in deps {
            reverse.entry(dep.as_str()).or_default().push(from.as_str());
        }
    }
    let labels: HashMap<String, String> = lock
        .packages
        .iter()
        .map(|package| (package_identity(package), module_label(package)))
        .collect();
    let mut out = HashMap::new();
    for package in &lock.packages {
        let root = package_identity(package);
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(root.as_str());
        seen.insert(root.clone());
        while let Some(node) = queue.pop_front() {
            if let Some(next) = reverse.get(node) {
                for dependent in next {
                    if seen.insert((*dependent).to_string()) {
                        queue.push_back(dependent);
                    }
                }
            }
        }
        let modules = seen
            .into_iter()
            .map(|id| labels.get(&id).cloned().unwrap_or(id))
            .collect();
        out.insert(root, modules);
    }
    out
}
