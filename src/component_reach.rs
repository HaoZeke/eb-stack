//! Easyconfigs and installed modules that carry a component in one generation.
//!
//! Names match with ASCII case folded, so `curl` hits `cURL`. A compat-layer
//! file is an input: those rows are not easyconfigs.

use crate::build_order::{module_name, ModuleKey};
use crate::dep_filter::FilterDeps;
use crate::domain::{Candidate, Toolchain};
use crate::hierarchy::{is_system_toolchain, known_hierarchy};
use crate::rebuild_set::{rebuild_set, RebuildCounts, RebuildQuery, RebuildRow, StackEasystack};
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
            &candidate.name,
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
    let changed: Vec<String> = changed.into_iter().collect();
    let (closure_counts, closure_rows, missing, extra) = close(query, &changed);
    notes.extend(extra);
    ReachDocument {
        schema_version: COMPONENT_REACH_SCHEMA_VERSION,
        generated_at: query.generated_at.to_string(),
        component: query.component.to_string(),
        version_req: query.version_req.to_string(),
        generation: query.generation.to_string(),
        carriers,
        changed,
        closure_counts,
        closure_rows,
        missing,
        notes,
    }
}

const HEADER: &str = "# Generated by `eb-stack component-reach`.\n";

/// Markdown tables of the carriers and the closure, in the pr-overlap shape.
pub fn render_markdown(document: &ReachDocument) -> String {
    let mut out = String::from("# Component reach\n\n");
    out.push_str(&format!(
        "Computed {} for `{}` `{}` in generation `{}`.\n\n{} carriers. Closure: {} must-rebuild, {} review, {} no-rebuild.\n\n",
        document.generated_at,
        document.component,
        document.version_req,
        document.generation,
        document.carriers.len(),
        document.closure_counts.must_rebuild,
        document.closure_counts.review,
        document.closure_counts.no_rebuild
    ));
    for note in &document.notes {
        out.push_str(&format!("- note: {note}\n"));
    }
    if !document.notes.is_empty() {
        out.push('\n');
    }
    out.push_str("| Easyconfig | Module | Role | Version | Installed |\n|---|---|---|---|---|\n");
    for row in &document.carriers {
        let installed = row
            .installed
            .map(|on| if on { "yes" } else { "no" })
            .unwrap_or("-");
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {installed} |\n",
            row.easyconfig, row.module, row.role, row.version
        ));
    }
    out.push_str("\n| Changed easyconfig | Dependent | Edge kind | Class | Reason |\n|---|---|---|---|---|\n");
    for row in &document.closure_rows {
        out.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            row.changed,
            row.dependent,
            row.edge_kind,
            row.class,
            row.reason.replace('|', "\\|")
        ));
    }
    out
}

/// TOML record with the same generated header as `pr-overlap`.
pub fn to_toml(document: &ReachDocument) -> Result<String, toml::ser::Error> {
    Ok(format!("{HEADER}\n{}", toml::to_string(document)?))
}

fn close(
    query: &ReachQuery<'_>,
    changed: &[String],
) -> (RebuildCounts, Vec<RebuildRow>, Vec<String>, Vec<String>) {
    let empty = RebuildCounts {
        no_rebuild: 0,
        review: 0,
        must_rebuild: 0,
    };
    if changed.is_empty() {
        return (empty, Vec::new(), Vec::new(), Vec::new());
    }
    let owned: Vec<Candidate> = query
        .candidates
        .iter()
        .filter(|candidate| in_generation(&candidate.toolchain, query.generation))
        .cloned()
        .collect();
    let modules: BTreeSet<String> = owned
        .iter()
        .map(|c| module_name(&ModuleKey::of(c)))
        .collect();
    let mut names: BTreeSet<String> = owned
        .iter()
        .map(|c| basename(&c.easyconfig_path).to_string())
        .collect();
    names.extend(changed.iter().cloned());
    let synthetic = query.easystacks.is_empty();
    let generated;
    let easystacks: &[StackEasystack] = if synthetic {
        let mut text = String::from("easyconfigs:\n");
        for name in &names {
            text.push_str("  - \"");
            text.push_str(&name.replace('"', ""));
            text.push_str("\"\n");
        }
        generated = vec![StackEasystack {
            path: format!("generation-{}.yml", query.generation),
            text,
        }];
        &generated
    } else {
        query.easystacks
    };
    let installed = query.installed.cloned().unwrap_or(modules);
    match rebuild_set(&RebuildQuery {
        base: owned,
        commits: Default::default(),
        easystacks: easystacks.to_vec(),
        changed: changed.to_vec(),
        installed,
        filter_deps: query.filter_deps.clone(),
        easyblocks: None,
        installed_source: query.generation.into(),
        generated_at: query.generated_at.into(),
        prefix: None,
        check_static: false,
    }) {
        Ok(document) => (
            document.counts,
            document.rows,
            document.missing,
            document.notes,
        ),
        Err(error) => (empty, Vec::new(), Vec::new(), vec![error.to_string()]),
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

fn list_hits(
    text: &str,
    component: &str,
    req: &str,
    own_name: &str,
    own: &str,
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(body) = field_list(text, "components") {
        out.extend(pair_hits(&body, "component", component, req, own));
    }
    if let Some(body) = field_list(text, "crates") {
        out.extend(pair_hits(&body, "crate", component, req, own));
        out.extend(file_hits(&body, "crate", component, req));
    }
    // A Cargo recipe often names a vendored crate only as a checksum key
    // (`rustls-webpki-0.102.8.tar.gz`). The recipe's own tarball is not a
    // crate it carries. A `.whl` key is a wheel in any recipe.
    if let Some(body) = field_list(text, "checksums") {
        let cargo = field_list(text, "crates").is_some();
        for (role, version) in file_hits(&body, "crate", component, req) {
            if role == "wheel" || (cargo && !(same(component, own_name) && version == own)) {
                out.push((role, version));
            }
        }
    }
    if let Some(body) = field_list(text, "exts_list") {
        out.extend(wheel_ext_hits(&body, component, req));
    }
    for (field, role) in [("sources", "source"), ("patches", "patch")] {
        if let Some(body) = field_list(text, field) {
            out.extend(file_hits(&body, role, component, req));
        }
    }
    if let Some(value) = assignment_string(text, "source_tmpl") {
        if value.contains(".whl") && same(own_name, component) && version_hits(own, req) {
            out.push(("wheel", own.to_string()));
        }
    }
    out
}

/// An `exts_list` entry installed from a wheel. The tuple names the
/// distribution; the wheel template often does not repeat that name.
fn wheel_ext_hits(body: &str, component: &str, req: &str) -> Vec<(&'static str, String)> {
    top_tuples(body)
        .into_iter()
        .filter(|tuple| tuple.contains(".whl"))
        .filter_map(|tuple| {
            let cap = pair_re().captures(&tuple)?;
            if !same(&cap[1], component) {
                return None;
            }
            let version = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            (!version.is_empty() && version_hits(version, req))
                .then(|| ("wheel", version.to_string()))
        })
        .collect()
}

fn top_tuples(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let mut quote = None;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == b'\\' {
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
            i += 1;
            continue;
        }
        if c == b'(' && depth == 1 {
            let chunk = take_balanced(&body[i..]);
            let len = chunk.len().max(1);
            out.push(chunk);
            i += len;
            continue;
        }
        if matches!(c, b'[' | b'(' | b'{') {
            depth += 1;
        }
        if matches!(c, b']' | b')' | b'}') {
            depth -= 1;
        }
        i += 1;
    }
    out
}

fn assignment_string(text: &str, field: &str) -> Option<String> {
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
                if matches!(bytes.get(j), Some(b'\'' | b'"')) {
                    let q = bytes[j];
                    let start = j + 1;
                    let mut k = start;
                    while k < bytes.len() && bytes[k] != q {
                        k += if bytes[k] == b'\\' { 2 } else { 1 };
                    }
                    return Some(text[start..k.min(bytes.len())].to_string());
                }
            }
        }
        i += 1;
    }
    None
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
            let role = if name.ends_with(".whl") {
                "wheel"
            } else {
                role
            };
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
        let installed = BTreeSet::from([
            "cURL/8.17.0-GCCcore-15.2.0".into(),
            "git/2-GCCcore-15.2.0".into(),
        ]);
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
        assert!(curl
            .closure_rows
            .iter()
            .any(|row| row.dependent.starts_with("git/") && row.class == "no-rebuild"));
        let rendered = to_toml(&curl).unwrap();
        assert!(rendered.starts_with(HEADER));
        assert!(render_markdown(&curl).contains("no-rebuild"));
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

    #[test]
    fn wheels_and_checksum_crates_are_carriers() {
        let dir = tempfile::tempdir().unwrap();
        let body = "\
name = 'uv'\n\
version = '0.10.9'\n\
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
source_tmpl = '%(name)s-%(version)s.tar.gz'\n\
crates = [('libz-sys', '1.1.29')]\n\
checksums = [\n\
    'aaaaaaaa',\n\
    {'uv-0.10.9.tar.gz': 'bb'},\n\
    {'rustls-webpki-0.102.8.tar.gz': 'cc'},\n\
]\n\
exts_list = [\n\
    ('pastel', '0.2.1', {\n\
        'sources': ['%(name)s-%(version)s-py3-none-any.whl'],\n\
    }),\n\
    ('urllib3', '2.5.0', {\n\
        'checksums': ['dd'],\n\
    }),\n\
]\n\
sources = ['pillow-10.4.0-cp311-cp311-manylinux.whl']\n\
";
        std::fs::write(dir.path().join("uv.eb"), body).unwrap();
        let wheel = "\
name = 'cleo'\n\
version = '2.1.0'\n\
toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
source_tmpl = '%(name)s-%(version)s-py3-none-any.whl'\n\
";
        std::fs::write(dir.path().join("cleo.eb"), wheel).unwrap();
        let parsed = parse_easyconfig_tree(dir.path()).unwrap();
        let filter = FilterDeps::empty();
        let run = |name: &str| {
            component_reach(&ReachQuery {
                candidates: &parsed.candidates,
                component: name,
                version_req: "*",
                generation: "2025b",
                installed: None,
                easystacks: &[],
                compat: &[],
                filter_deps: &filter,
                generated_at: "2026-10-24T00:00:00Z",
            })
        };
        let webpki = run("rustls-webpki");
        assert!(
            webpki
                .carriers
                .iter()
                .any(|row| row.role == "crate" && row.version == "0.102.8"),
            "{:?}",
            webpki.carriers
        );
        assert!(webpki.changed.iter().any(|name| name == "uv.eb"));
        let uv = run("uv");
        assert!(uv.carriers.iter().any(|row| row.role == "module"));
        assert!(uv.carriers.iter().all(|row| row.role != "crate"));
        assert!(run("pastel")
            .carriers
            .iter()
            .any(|row| row.role == "wheel" && row.version == "0.2.1"));
        assert!(run("urllib3")
            .carriers
            .iter()
            .any(|row| row.role == "extension"));
        assert!(run("urllib3")
            .carriers
            .iter()
            .all(|row| row.role != "wheel"));
        assert!(run("pillow")
            .carriers
            .iter()
            .any(|row| row.role == "wheel" && row.version == "10.4.0"));
        assert!(run("cleo")
            .carriers
            .iter()
            .any(|row| row.role == "wheel" && row.version == "2.1.0"));
    }
}
