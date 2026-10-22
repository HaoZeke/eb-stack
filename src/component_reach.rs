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
        let text = std::fs::read_to_string(&candidate.easyconfig_path).unwrap_or_default();
        let mut found = parsed_hits(candidate, query.component, query.version_req);
        found.extend(list_hits(
            &text,
            query.component,
            query.version_req,
            &candidate.version,
        ));
        for (role, version) in found {
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
    if let Some(installed) = query.installed {
        for spec in installed {
            let Some((name, rest)) = spec.split_once('/') else {
                continue;
            };
            let version = rest.split('-').next().unwrap_or(rest);
            if same(name, query.component)
                && version_hits(version, query.version_req)
                && spec_in_generation(spec, query.generation)
                && carriers.iter().all(|row| row.module != *spec)
            {
                carriers.push(Carrier {
                    role: "installed".into(),
                    version: version.into(),
                    easyconfig: String::new(),
                    module: spec.clone(),
                    installed: Some(true),
                });
            }
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

fn list_hits(text: &str, component: &str, req: &str, own: &str) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(body) = field_list(text, "components") {
        out.extend(pair_hits(&body, "component", component, req, own));
    }
    if let Some(body) = field_list(text, "crates") {
        out.extend(pair_hits(&body, "crate", component, req, own));
        out.extend(file_hits(&body, "crate", component, req));
    }
    for (field, role) in [("sources", "source"), ("patches", "patch")] {
        if let Some(body) = field_list(text, field) {
            out.extend(file_hits(&body, role, component, req));
        }
    }
    out
}

fn pair_hits(
    body: &str,
    role: &'static str,
    component: &str,
    req: &str,
    own: &str,
) -> Vec<(&'static str, String)> {
    pair_re()
        .captures_iter(body)
        .filter_map(|cap| {
            if !same(&cap[1], component) {
                return None;
            }
            let version = cap.get(2).map(|m| m.as_str()).unwrap_or(own);
            version_hits(version, req).then(|| (role, version.to_string()))
        })
        .collect()
}

fn file_hits(
    body: &str,
    role: &'static str,
    component: &str,
    req: &str,
) -> Vec<(&'static str, String)> {
    quoted(body)
        .into_iter()
        .filter_map(|name| {
            let version = version_in_filename(&name, component)?;
            version_hits(&version, req).then_some((role, version))
        })
        .collect()
}

fn pair_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r#"(?s)\(\s*['"]([^'"]+)['"]\s*,\s*(?:['"]([^'"]+)['"]|version\b)"#)
            .expect("pair pattern")
    })
}

fn field_list(text: &str, field: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let needle = field.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if bytes[i..].starts_with(needle) && at_line_start(bytes, i) {
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'=') {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if matches!(bytes.get(j), Some(b'[' | b'(')) {
                    return Some(take_balanced(&text[j..]));
                }
            }
        }
        i += 1;
    }
    None
}

fn at_line_start(bytes: &[u8], index: usize) -> bool {
    let mut k = index;
    while k > 0 && matches!(bytes[k - 1], b' ' | b'\t') {
        k -= 1;
    }
    k == 0 || bytes[k - 1] == b'\n'
}

fn take_balanced(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut depth = 0i32;
    let mut i = 0;
    let mut quote = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            out.push(c as char);
            if c == b'\\' && i + 1 < bytes.len() {
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
            out.push(c as char);
            i += 1;
            continue;
        }
        if matches!(c, b'[' | b'(' | b'{') {
            depth += 1;
        }
        out.push(c as char);
        if matches!(c, b']' | b')' | b'}') {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        i += 1;
    }
    out
}

fn quoted(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            let q = bytes[i];
            let start = i + 1;
            i = start;
            while i < bytes.len() && bytes[i] != q {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            out.push(body[start..i.min(bytes.len())].to_string());
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

fn version_in_filename(filename: &str, component: &str) -> Option<String> {
    let file = filename.to_ascii_lowercase();
    let name = component.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = file[from..].find(&name) {
        let idx = from + rel;
        let end = idx + name.len();
        let before = idx == 0 || !file.as_bytes()[idx - 1].is_ascii_alphanumeric();
        if before {
            let rest = filename[end..].trim_start_matches(['-', '_', '.']);
            let version: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let version = version.trim_matches('.').to_string();
            if version.chars().any(|c| c.is_ascii_digit()) {
                return Some(version);
            }
        }
        from = end.max(from + 1);
    }
    None
}

fn spec_in_generation(spec: &str, generation: &str) -> bool {
    if spec.contains(&format!("-{generation}")) {
        return true;
    }
    known_hierarchy(&Toolchain {
        name: "foss".into(),
        version: generation.into(),
    })
    .is_some_and(|hierarchy| {
        hierarchy.members.iter().any(|member| {
            !member.version.is_empty()
                && !is_system_toolchain(member)
                && spec.contains(&format!("-{}-{}", member.name, member.version))
        })
    })
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
        eb("cargo-c.eb", "name = 'cargo-c'\nversion = '0.10'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\ncrates = [('libz-sys', '1.1.29'), ('zlib-rs', '0.6.4'), {'libz-sys-1.1.29.tar.gz': 'ab'}]\n");
        eb("X11.eb", "name = 'X11'\nversion = '1'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\ncomponents = [('libX11', '1.8.12', {'easyblock': 'ConfigureMake'})]\nsources = ['zlib-1.3.1.tar.gz']\npatches = ['curl-8.4.0-fix.patch']\n");
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
        assert!(run("libz-sys")
            .carriers
            .iter()
            .any(|row| row.role == "crate" && row.version == "1.1.29"));
        assert!(run("zlib-rs")
            .carriers
            .iter()
            .any(|row| row.role == "crate"));
        assert!(run("libX11")
            .carriers
            .iter()
            .any(|row| row.role == "component" && row.version == "1.8.12"));
        assert!(run("zlib")
            .carriers
            .iter()
            .any(|row| row.role == "source" && row.version == "1.3.1"));
        assert!(run("curl")
            .carriers
            .iter()
            .any(|row| row.role == "patch" && row.version == "8.4.0"));
    }
}
