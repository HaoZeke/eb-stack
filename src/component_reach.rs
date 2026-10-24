//! Easyconfigs and installed modules that carry a component in one generation.
//!
//! Names match with ASCII case folded, so `curl` hits `cURL`. A compat-layer
//! file is an input: those rows are not easyconfigs.

use crate::build_order::{module_name, ModuleKey};
use crate::dep_filter::FilterDeps;
use crate::domain::{Candidate, Toolchain};
use crate::hierarchy::{is_system_toolchain, known_hierarchy};
use crate::osv::ResolvedComponent;
use crate::rebuild_set::{rebuild_set, RebuildCounts, RebuildQuery, RebuildRow, StackEasystack};
use crate::registry::RegistryClient;
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
    ///
    /// Empty when the match is only [`Self::exact_versions`].
    pub version_req: &'a str,
    /// Further requirements. A version matches when any requirement matches.
    pub also_reqs: &'a [String],
    /// Exact versions that match even when no requirement does.
    pub exact_versions: &'a [String],
    /// Package URL that named this component, when there was one.
    pub purl: Option<&'a str>,
    /// Ecosystem from the purl or the OSV record.
    pub ecosystem: Option<&'a str>,
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
    /// `crate`, `wheel`, `source`, `patch`, `compat-layer`, or `installed`.
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
    ///
    /// Empty when the match is the exact [`Self::versions`] list.
    pub version_req: String,
    /// Every requirement, when more than one was OR-ed together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub version_reqs: Vec<String>,
    /// Exact versions from a CVE record.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
    /// Package URL, when the query was a purl or a resolved CVE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purl: Option<String>,
    /// Ecosystem, when the query named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
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
    let model = purl_match(query.purl);
    let mut carriers = Vec::new();
    let mut changed = BTreeSet::new();
    let mut notes = Vec::new();
    if let PurlMatch::Unsupported = model {
        if let Some(kind) = purl_kind(query.purl) {
            notes.push(format!("no carrier model for purl type {kind}"));
        }
    }
    if !matches!(model, PurlMatch::Unsupported) {
        for candidate in query.candidates {
            if !in_generation(&candidate.toolchain, query.generation) {
                continue;
            }
            let module = module_name(&ModuleKey::of(candidate));
            let file = basename(&candidate.easyconfig_path).to_string();
            let text = std::fs::read_to_string(&candidate.easyconfig_path).unwrap_or_default();
            let recipe_lang = recipe_language(&text, candidate.easyblock.as_deref());
            let mut found = parsed_hits(candidate, query);
            found.extend(list_hits(&text, query, &candidate.name, &candidate.version));
            found.retain(|(role, _version)| {
                let lang = if *role == "component" {
                    field_list(&text, "components")
                        .as_deref()
                        .and_then(|body| component_language(body, query.component, recipe_lang))
                } else {
                    recipe_lang
                };
                role_allowed(&model, role, lang)
            });
            for (role, version) in found {
                if role != "dependency" && role != "builddependency" {
                    changed.insert(file.clone());
                }
                carriers.push(carrier(role, version, &file, &module, query.installed));
            }
        }
    }
    if role_allowed(&model, "compat-layer", None) {
        for (name, version) in query.compat {
            if same(name, query.component) && version_selected(version, query) {
                carriers.push(Carrier {
                    role: "compat-layer".into(),
                    version: version.clone(),
                    easyconfig: String::new(),
                    module: String::new(),
                    installed: None,
                });
                notes.push(
                    "compat-layer input carries this component and has no rebuild edge".into(),
                );
            }
        }
    }
    if role_allowed(&model, "installed", None) {
        if let Some(installed) = query.installed {
            for spec in installed {
                let Some((name, rest)) = spec.split_once('/') else {
                    continue;
                };
                let version = rest.split('-').next().unwrap_or(rest);
                if same(name, query.component)
                    && version_selected(version, query)
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
    let (mut closure_counts, closure_rows, missing, extra) = close(query, &changed);
    if matches!(model, PurlMatch::Unsupported) {
        closure_counts.review = 1;
    }
    notes.extend(extra);
    let mut version_reqs = Vec::new();
    if !query.version_req.is_empty() {
        version_reqs.push(query.version_req.to_string());
    }
    version_reqs.extend(query.also_reqs.iter().cloned());
    ReachDocument {
        schema_version: COMPONENT_REACH_SCHEMA_VERSION,
        generated_at: query.generated_at.to_string(),
        component: query.component.to_string(),
        version_req: query.version_req.to_string(),
        version_reqs: if version_reqs.len() > 1 {
            version_reqs
        } else {
            Vec::new()
        },
        versions: query.exact_versions.to_vec(),
        purl: query.purl.map(str::to_string),
        ecosystem: query.ecosystem.map(str::to_string),
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
    let requirement = if !document.version_req.is_empty() {
        document.version_req.clone()
    } else if !document.versions.is_empty() {
        format!("{} exact versions", document.versions.len())
    } else {
        "*".to_string()
    };
    out.push_str(&format!(
        "Computed {} for `{}` `{requirement}` in generation `{}`.\n\n{} carriers. Closure: {} must-rebuild, {} review, {} no-rebuild.\n\n",
        document.generated_at,
        document.component,
        document.generation,
        document.carriers.len(),
        document.closure_counts.must_rebuild,
        document.closure_counts.review,
        document.closure_counts.no_rebuild
    ));
    if let Some(purl) = &document.purl {
        let ecosystem = document.ecosystem.as_deref().unwrap_or("");
        out.push_str(&format!("Package URL `{purl}`"));
        if !ecosystem.is_empty() {
            out.push_str(&format!(" ({ecosystem})"));
        }
        out.push_str(".\n\n");
    }
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

/// Shared inputs of one generation scan.
pub struct ReachScope<'a> {
    /// Easyconfigs, later trees already preferred.
    pub candidates: &'a [Candidate],
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

/// One CVE, and the reach of every package OSV attached to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CveReachDocument {
    /// [`COMPONENT_REACH_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// `cve`, so this file is not read as one component document.
    pub kind: String,
    /// Id that was resolved.
    pub cve: String,
    /// When it was computed.
    pub generated_at: String,
    /// Toolchain generation.
    pub generation: String,
    /// Packages the record named.
    pub resolved: Vec<ResolvedComponent>,
    /// One reach document per resolved package.
    pub results: Vec<ReachDocument>,
    /// Alias fetches that failed, and closure notes that are not per result.
    pub notes: Vec<String>,
    /// Ranges and events OSV named that this tool could not parse.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
}

/// Reach of every package in one OSV resolution.
pub fn cve_reach(
    cve: &str,
    components: &[ResolvedComponent],
    notes: &[String],
    unresolved: &[String],
    scope: &ReachScope<'_>,
) -> CveReachDocument {
    let mut results = Vec::new();
    let mut listed = unresolved.to_vec();
    for component in components {
        for item in &component.unresolved {
            listed.push(format!("{}: {item}", component.name));
        }
        let version_req = component
            .version_reqs
            .first()
            .map(String::as_str)
            .unwrap_or("");
        let also = if component.version_reqs.len() > 1 {
            component.version_reqs[1..].to_vec()
        } else {
            Vec::new()
        };
        let purl = (!component.purl.is_empty()).then_some(component.purl.as_str());
        let ecosystem = (!component.ecosystem.is_empty()).then_some(component.ecosystem.as_str());
        let mut result = component_reach(&ReachQuery {
            candidates: scope.candidates,
            component: &component.name,
            version_req,
            also_reqs: &also,
            exact_versions: &component.versions,
            purl,
            ecosystem,
            generation: scope.generation,
            installed: scope.installed,
            easystacks: scope.easystacks,
            compat: scope.compat,
            filter_deps: scope.filter_deps,
            generated_at: scope.generated_at,
        });
        if component.version_reqs.is_empty()
            && component.versions.is_empty()
            && !component.unresolved.is_empty()
        {
            result
                .notes
                .push("every OSV range was unparsed; counted as review".into());
            result.closure_counts.review = result.closure_counts.review.max(1);
        }
        results.push(result);
    }
    CveReachDocument {
        schema_version: COMPONENT_REACH_SCHEMA_VERSION,
        kind: "cve".into(),
        cve: cve.to_string(),
        generated_at: scope.generated_at.to_string(),
        generation: scope.generation.to_string(),
        resolved: components.to_vec(),
        results,
        notes: notes.to_vec(),
        unresolved: listed,
    }
}

/// Markdown for [`CveReachDocument`], one section per package.
pub fn render_cve_markdown(document: &CveReachDocument) -> String {
    let mut out = format!(
        "# CVE reach\n\nComputed {} for `{}` in generation `{}`. {} packages.\n\n",
        document.generated_at,
        document.cve,
        document.generation,
        document.results.len()
    );
    for note in &document.notes {
        out.push_str(&format!("- note: {note}\n"));
    }
    if !document.notes.is_empty() {
        out.push('\n');
    }
    if !document.unresolved.is_empty() {
        out.push_str("Unresolved ranges:\n\n");
        for item in &document.unresolved {
            out.push_str(&format!("- {item}\n"));
        }
        out.push('\n');
    }
    for result in &document.results {
        out.push_str(&render_markdown(result));
        out.push('\n');
    }
    out
}

/// TOML record for a CVE reach document.
pub fn cve_to_toml(document: &CveReachDocument) -> Result<String, toml::ser::Error> {
    Ok(format!("{HEADER}\n{}", toml::to_string(document)?))
}

/// Download easystack YAML. Anything other than `https://` is refused.
pub fn fetch_easystack_urls(
    urls: &[String],
    client: &dyn RegistryClient,
) -> Result<Vec<StackEasystack>, String> {
    let mut out = Vec::new();
    for url in urls {
        if !url.starts_with("https://") {
            return Err(format!("easystack URL must be https: {url}"));
        }
        let bytes = client
            .get(url)
            .map_err(|error| format!("easystack {url}: {error}"))?;
        let text = String::from_utf8(bytes).map_err(|_| format!("easystack {url} is not utf-8"))?;
        out.push(StackEasystack {
            path: url.clone(),
            text,
        });
    }
    Ok(out)
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

/// `generic` and `github` name easyconfigs. The language types name
/// extensions, bundled components, crates, and wheels of that language.
pub fn purl_carrier_known(kind: &str) -> bool {
    matches!(
        kind,
        "generic" | "github" | "pypi" | "cargo" | "npm" | "golang" | "cran"
    )
}

enum PurlMatch {
    /// No package URL: every carrier role is eligible.
    Any,
    /// Easyconfig names: the module, its dependencies, components, and files.
    Easyconfig,
    /// One language ecosystem.
    Language(&'static str),
    /// The purl type has no carrier model.
    Unsupported,
}

fn purl_kind(purl: Option<&str>) -> Option<&str> {
    let rest = purl?.strip_prefix("pkg:")?;
    let end = rest.find(['/', '@', '?', '#']).unwrap_or(rest.len());
    let kind = &rest[..end];
    (!kind.is_empty()).then_some(kind)
}

fn purl_match(purl: Option<&str>) -> PurlMatch {
    let Some(kind) = purl_kind(purl) else {
        return PurlMatch::Any;
    };
    match kind {
        "generic" | "github" => PurlMatch::Easyconfig,
        "pypi" => PurlMatch::Language("pypi"),
        "cargo" => PurlMatch::Language("cargo"),
        "npm" => PurlMatch::Language("npm"),
        "golang" => PurlMatch::Language("golang"),
        "cran" => PurlMatch::Language("cran"),
        _ => PurlMatch::Unsupported,
    }
}

fn role_allowed(model: &PurlMatch, role: &str, lang: Option<&str>) -> bool {
    match model {
        PurlMatch::Any => true,
        PurlMatch::Unsupported => false,
        PurlMatch::Easyconfig => matches!(
            role,
            "module"
                | "dependency"
                | "builddependency"
                | "component"
                | "source"
                | "patch"
                | "compat-layer"
                | "installed"
        ),
        PurlMatch::Language(want) => match role {
            "crate" => *want == "cargo",
            "wheel" => *want == "pypi",
            "extension" | "module" | "component" => lang == Some(*want),
            _ => false,
        },
    }
}

fn class_language(class: &str) -> Option<&'static str> {
    let class = class.trim().trim_matches(|c| c == '\'' || c == '"');
    if class.contains("Python") {
        Some("pypi")
    } else if class.contains("RPackage") {
        Some("cran")
    } else if class.contains("Npm") || class.contains("NodeJS") || class.contains("Nodejs") {
        Some("npm")
    } else if class.contains("GoPackage") {
        Some("golang")
    } else if class.contains("Cargo") || class.contains("Crate") {
        Some("cargo")
    } else {
        None
    }
}

fn recipe_language(text: &str, easyblock: Option<&str>) -> Option<&'static str> {
    if let Some(class) = assignment_string(text, "exts_defaultclass") {
        if let Some(lang) = class_language(&class) {
            return Some(lang);
        }
    }
    easyblock.and_then(class_language)
}

fn component_language(
    body: &str,
    name: &str,
    recipe: Option<&'static str>,
) -> Option<&'static str> {
    for tuple in top_tuples(body) {
        let Some(cap) = pair_re().captures(&tuple) else {
            continue;
        };
        if !same(&cap[1], name) {
            continue;
        }
        if let Some(class) = quoted_after_key(&tuple, "easyblock") {
            return class_language(&class);
        }
        return recipe;
    }
    recipe
}

fn quoted_after_key(text: &str, key: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let needle = key.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if bytes[i..].starts_with(needle) {
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if matches!(bytes.get(j), Some(b':' | b'=')) {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if matches!(bytes.get(j), Some(b'\'' | b'"')) {
                    let quote = bytes[j];
                    let start = j + 1;
                    let mut k = start;
                    while k < bytes.len() && bytes[k] != quote {
                        k += 1;
                    }
                    return Some(text[start..k].to_string());
                }
            }
        }
        i += 1;
    }
    None
}

fn parsed_hits<'a>(candidate: &'a Candidate, query: &ReachQuery<'_>) -> Vec<(&'a str, String)> {
    let mut out = Vec::new();
    if same(&candidate.name, query.component) && version_selected(&candidate.version, query) {
        out.push(("module", candidate.version.clone()));
    }
    for (role, deps) in [
        ("dependency", &candidate.dependencies),
        ("builddependency", &candidate.builddependencies),
    ] {
        for dep in deps {
            if same(&dep.name, query.component) && version_selected(&dep.version_req, query) {
                out.push((role, dep.version_req.trim_start_matches("==").to_string()));
            }
        }
    }
    for ext in &candidate.exts_list {
        if same(&ext.name, query.component) && version_selected(&ext.version, query) {
            out.push(("extension", ext.version.clone()));
        }
    }
    out
}

fn version_selected(found: &str, query: &ReachQuery<'_>) -> bool {
    let mut saw_req = false;
    let mut hit = false;
    for req in std::iter::once(query.version_req).chain(query.also_reqs.iter().map(String::as_str))
    {
        if req.is_empty() {
            continue;
        }
        saw_req = true;
        if version_hits(found, req) {
            hit = true;
        }
    }
    let listed = query.exact_versions.iter().any(|version| version == found);
    if saw_req {
        hit || listed
    } else {
        listed
    }
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
    query: &ReachQuery<'_>,
    own_name: &str,
    own: &str,
) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Some(body) = field_list(text, "components") {
        out.extend(pair_hits(&body, "component", query, own));
    }
    if let Some(body) = field_list(text, "crates") {
        out.extend(pair_hits(&body, "crate", query, own));
        out.extend(file_hits(&body, "crate", query));
    }
    // A Cargo recipe often names a vendored crate only as a checksum key
    // (`rustls-webpki-0.102.8.tar.gz`). The recipe's own tarball is not a
    // crate it carries. A `.whl` key is a wheel in any recipe.
    if let Some(body) = field_list(text, "checksums") {
        let cargo = field_list(text, "crates").is_some();
        for (role, version) in file_hits(&body, "crate", query) {
            if role == "wheel" || (cargo && !(same(query.component, own_name) && version == own)) {
                out.push((role, version));
            }
        }
    }
    if let Some(body) = field_list(text, "exts_list") {
        out.extend(wheel_ext_hits(&body, query));
    }
    for (field, role) in [("sources", "source"), ("patches", "patch")] {
        if let Some(body) = field_list(text, field) {
            out.extend(file_hits(&body, role, query));
        }
    }
    if let Some(value) = assignment_string(text, "source_tmpl") {
        if value.contains(".whl") && same(own_name, query.component) && version_selected(own, query)
        {
            out.push(("wheel", own.to_string()));
        }
    }
    out
}

/// An `exts_list` entry installed from a wheel. The tuple names the
/// distribution; the wheel template often does not repeat that name.
fn wheel_ext_hits(body: &str, query: &ReachQuery<'_>) -> Vec<(&'static str, String)> {
    top_tuples(body)
        .into_iter()
        .filter(|tuple| tuple.contains(".whl"))
        .filter_map(|tuple| {
            let cap = pair_re().captures(&tuple)?;
            if !same(&cap[1], query.component) {
                return None;
            }
            let version = cap.get(2).map(|m| m.as_str()).unwrap_or("");
            (!version.is_empty() && version_selected(version, query))
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
    query: &ReachQuery<'_>,
    own: &str,
) -> Vec<(&'static str, String)> {
    pair_re()
        .captures_iter(body)
        .filter_map(|cap| {
            if !same(&cap[1], query.component) {
                return None;
            }
            let version = cap.get(2).map(|m| m.as_str()).unwrap_or(own);
            version_selected(version, query).then(|| (role, version.to_string()))
        })
        .collect()
}

fn file_hits(
    body: &str,
    role: &'static str,
    query: &ReachQuery<'_>,
) -> Vec<(&'static str, String)> {
    quoted(body)
        .into_iter()
        .filter_map(|name| {
            let version = version_in_filename(&name, query.component)?;
            let role = if name.ends_with(".whl") {
                "wheel"
            } else {
                role
            };
            version_selected(&version, query).then_some((role, version))
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
                also_reqs: &[],
                exact_versions: &[],
                purl: None,
                ecosystem: None,
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
                also_reqs: &[],
                exact_versions: &[],
                purl: None,
                ecosystem: None,
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

    #[test]
    fn an_unparsed_osv_range_counts_as_review() {
        use crate::osv::ResolvedComponent;
        let filter = FilterDeps::empty();
        let component = ResolvedComponent {
            ecosystem: "Debian".into(),
            name: "curl".into(),
            purl: "pkg:deb/debian/curl".into(),
            version_reqs: Vec::new(),
            versions: Vec::new(),
            unresolved: vec![
                "GIT range not parsed".into(),
                "requirement not parsed: >=0,<1.2.3,ubuntu".into(),
            ],
        };
        let document = cve_reach(
            "CVE-2099-0001",
            std::slice::from_ref(&component),
            &[],
            &[],
            &ReachScope {
                candidates: &[],
                generation: "2026.1",
                installed: None,
                easystacks: &[],
                compat: &[],
                filter_deps: &filter,
                generated_at: "2026-10-24T00:00:00Z",
            },
        );
        assert!(document
            .unresolved
            .iter()
            .any(|item| item.contains("GIT range not parsed")));
        assert!(document.results[0].carriers.is_empty());
        assert_eq!(document.results[0].closure_counts.review, 1);
        let markdown = render_cve_markdown(&document);
        assert!(markdown.contains("Unresolved ranges"));
        assert!(markdown.contains("GIT range not parsed"));
    }
}
