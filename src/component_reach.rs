//! Easyconfigs and installed modules that carry a component in one generation.
//!
//! Names match with ASCII case folded, so `curl` hits `cURL`. A compat-layer
//! file is an input: those rows are not easyconfigs.

use crate::build_order::{module_name, ModuleKey};
use crate::dep_filter::FilterDeps;
use crate::domain::{Candidate, Toolchain};
use crate::hierarchy::{is_system_toolchain, known_hierarchy};
use crate::rebuild_set::{RebuildCounts, RebuildRow, StackEasystack};
use crate::version::matches_req;
use serde::Serialize;
use std::collections::BTreeSet;

/// Schema of [`ReachDocument`].
pub const COMPONENT_REACH_SCHEMA_VERSION: u32 = 1;

/// One run. `version_req` is an exact version, `*`, or a requirement.
pub struct ReachQuery<'a> {
    /// Easyconfigs, later trees already preferred.
    pub candidates: &'a [Candidate],
    /// Component name.
    pub component: &'a str,
    /// Version, `*`, or a requirement such as `>=8,<9`.
    pub version_req: &'a str,
    /// Toolchain generation, for example `2026.1`.
    pub generation: &'a str,
    /// Installed `Name/version-toolchain` lines.
    pub installed: Option<&'a BTreeSet<String>>,
    /// Deployed easystacks. Empty uses the generation as the stack.
    pub easystacks: &'a [StackEasystack],
    /// Compat-layer `name` / `version` rows.
    pub compat: &'a [(String, String)],
    /// `--filter-deps`, applied when the closure is filled in.
    pub filter_deps: &'a FilterDeps,
    /// RFC 3339 timestamp.
    pub generated_at: &'a str,
}

/// One easyconfig or installed module that carries the component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Carrier {
    /// `module`, `dependency`, `builddependency`, `extension`, `component`,
    /// `crate`, `source`, `patch`, `compat-layer`, or `installed`.
    pub role: String,
    /// Version that matched.
    pub version: String,
    /// Easyconfig filename, empty for compat-layer and installed-only rows.
    pub easyconfig: String,
    /// `Name/version-toolchain`.
    pub module: String,
    /// Set when an installed list was passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed: Option<bool>,
}

/// Carriers and, once filled, the rebuild closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReachDocument {
    /// [`COMPONENT_REACH_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// When it was computed.
    pub generated_at: String,
    /// Name that was asked for.
    pub component: String,
    /// Version requirement that was asked for.
    pub version_req: String,
    /// Toolchain generation.
    pub generation: String,
    /// Who carries it.
    pub carriers: Vec<Carrier>,
    /// Recipes that contain the component, fed to the closure.
    pub changed: Vec<String>,
    /// Closure counts. Zero until the closure is run.
    pub closure_counts: RebuildCounts,
    /// Closure rows.
    pub closure_rows: Vec<RebuildRow>,
    /// Changed filenames missing from the closure graph.
    pub missing: Vec<String>,
    /// Compat-layer hits and closure notes.
    pub notes: Vec<String>,
}

/// Find carriers in one toolchain generation.
pub fn component_reach(query: &ReachQuery<'_>) -> ReachDocument {
    let mut carriers = Vec::new();
    let mut changed = BTreeSet::new();
    for candidate in query.candidates {
        if !in_generation(&candidate.toolchain, query.generation) {
            continue;
        }
        let module = module_name(&ModuleKey::of(candidate));
        let file = basename(&candidate.easyconfig_path).to_string();
        for (role, version) in parsed_hits(candidate, query.component, query.version_req) {
            if role != "dependency" && role != "builddependency" {
                changed.insert(file.clone());
            }
            carriers.push(carrier(role, version, &file, &module, query.installed));
        }
    }
    let mut notes = Vec::new();
    for (name, version) in query.compat {
        if same(name, query.component) && version_hits(version, query.version_req) {
            carriers.push(Carrier {
                role: "compat-layer".into(),
                version: version.clone(),
                easyconfig: String::new(),
                module: String::new(),
                installed: None,
            });
            notes.push("compat-layer input carries this component and has no rebuild edge".into());
        }
    }
    carriers.sort_by(|a, b| {
        (&a.easyconfig, &a.role, &a.version, &a.module).cmp(&(
            &b.easyconfig,
            &b.role,
            &b.version,
            &b.module,
        ))
    });
    carriers.dedup();
    ReachDocument {
        schema_version: COMPONENT_REACH_SCHEMA_VERSION,
        generated_at: query.generated_at.to_string(),
        component: query.component.to_string(),
        version_req: query.version_req.to_string(),
        generation: query.generation.to_string(),
        carriers,
        changed: changed.into_iter().collect(),
        closure_counts: RebuildCounts {
            no_rebuild: 0,
            review: 0,
            must_rebuild: 0,
        },
        closure_rows: Vec::new(),
        missing: Vec::new(),
        notes,
    }
}

/// `name version` or `name==version`, one per line. `#` comments are skipped.
pub fn parse_compat_layer(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let cut = line.find([' ', '\t', '='])?;
            let version = line[cut + 1..].trim().trim_start_matches('=').trim();
            Some((line[..cut].trim().to_string(), version.to_string()))
        })
        .collect()
}

fn carrier(
    role: &str,
    version: String,
    file: &str,
    module: &str,
    installed: Option<&BTreeSet<String>>,
) -> Carrier {
    Carrier {
        role: role.to_string(),
        version,
        easyconfig: file.to_string(),
        module: module.to_string(),
        installed: installed.map(|set| set.contains(module)),
    }
}

fn parsed_hits<'a>(candidate: &'a Candidate, component: &str, req: &str) -> Vec<(&'a str, String)> {
    let mut out = Vec::new();
    if same(&candidate.name, component) && version_hits(&candidate.version, req) {
        out.push(("module", candidate.version.clone()));
    }
    for (role, deps) in [
        ("dependency", &candidate.dependencies),
        ("builddependency", &candidate.builddependencies),
    ] {
        for dep in deps {
            if same(&dep.name, component) && version_hits(&dep.version_req, req) {
                out.push((role, dep.version_req.trim_start_matches("==").to_string()));
            }
        }
    }
    for ext in &candidate.exts_list {
        if same(&ext.name, component) && version_hits(&ext.version, req) {
            out.push(("extension", ext.version.clone()));
        }
    }
    out
}

fn version_hits(found: &str, req: &str) -> bool {
    if req == "*" {
        return !found.is_empty();
    }
    let found = found.trim();
    if let Some(exact) = found.strip_prefix("==") {
        return matches_req(exact, req);
    }
    if found.starts_with(['>', '<', '!', '~', '^']) {
        let query = req.trim().strip_prefix("==").unwrap_or(req.trim());
        return query.chars().next().is_some_and(|c| c.is_ascii_digit())
            && !req.starts_with(['>', '<', '!', '~', '^', '*'])
            && !req.contains(',')
            && matches_req(query, found);
    }
    matches_req(found, req)
}

fn in_generation(tc: &Toolchain, generation: &str) -> bool {
    if is_system_toolchain(tc) {
        return false;
    }
    tc.version == generation
        || known_hierarchy(&Toolchain {
            name: "foss".into(),
            version: generation.into(),
        })
        .is_some_and(|hierarchy| hierarchy.contains(tc))
}

fn same(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eb_parse::parse_easyconfig_tree;

    #[test]
    fn parsed_fields_carry_a_component_inside_its_generation() {
        let dir = tempfile::tempdir().unwrap();
        let eb = |name: &str, body: &str| std::fs::write(dir.path().join(name), body).unwrap();
        eb("cURL-8.17.0-GCCcore-15.2.0.eb", "name = 'cURL'\nversion = '8.17.0'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n");
        eb("git-2-GCCcore-15.2.0.eb", "name = 'git'\nversion = '2'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\ndependencies = [('cURL', '8.17.0')]\nbuilddependencies = [('cURL', '8.17.0')]\n");
        eb("Old-1.0-foss-2025b.eb", "name = 'Old'\nversion = '1.0'\ntoolchain = {'name': 'foss', 'version': '2025b'}\ndependencies = [('cURL', '7.0')]\n");
        eb("Python-bundle-PyPI-2026.04-GCCcore-15.2.0.eb", "name = 'Python-bundle-PyPI'\nversion = '2026.04'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\nexts_list = [('urllib3', '2.6.3', {})]\n");
        eb("JupyterLab-4.5.8-GCCcore-15.2.0.eb", "name = 'JupyterLab'\nversion = '4.5.8'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\nexts_list = [('jupyterlab', version, {})]\n");
        let parsed = parse_easyconfig_tree(dir.path()).unwrap();
        assert!(parsed.skipped.is_empty(), "{:?}", parsed.skipped);
        let filter = FilterDeps::empty();
        let compat = parse_compat_layer("# layer\ncurl 8.17.0\n");
        let installed = BTreeSet::from(["cURL/8.17.0-GCCcore-15.2.0".into()]);
        let run = |name: &str| {
            component_reach(&ReachQuery {
                candidates: &parsed.candidates,
                component: name,
                version_req: "*",
                generation: "2026.1",
                installed: Some(&installed),
                easystacks: &[],
                compat: &compat,
                filter_deps: &filter,
                generated_at: "2026-10-22T12:00:00Z",
            })
        };
        let curl = run("curl");
        let roles: Vec<_> = curl.carriers.iter().map(|row| row.role.as_str()).collect();
        assert!(roles.contains(&"module") && roles.contains(&"dependency"));
        assert!(roles.contains(&"builddependency") && roles.contains(&"compat-layer"));
        assert!(curl
            .carriers
            .iter()
            .any(|row| row.role == "module" && row.installed == Some(true)));
        assert!(!curl
            .carriers
            .iter()
            .any(|row| row.easyconfig.starts_with("Old-")));
        assert!(curl.changed.iter().any(|name| name.starts_with("cURL-")));
        assert!(!curl.changed.iter().any(|name| name.starts_with("git-")));
        assert!(run("urllib3")
            .carriers
            .iter()
            .any(|row| row.role == "extension" && row.version == "2.6.3"));
        assert!(run("jupyterlab")
            .carriers
            .iter()
            .any(|row| row.role == "extension" && row.version == "4.5.8"));
        assert!(run("jupyterlab")
            .carriers
            .iter()
            .any(|row| row.role == "module"));
    }
}
