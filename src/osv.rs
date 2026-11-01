//! A package URL, or a CVE id resolved through OSV.
//!
//! The CVE record often has only a git range. The package name and the
//! versions EasyBuild actually pins are on the GHSA or RUSTSEC alias, or in
//! `name-version` tags such as `libXpm-3.5.17`. A GIT range is mapped onto
//! release versions from `affected[].versions` when that list is present, and
//! otherwise from tags in the upstream repository. An offline tag lookup
//! leaves the range unresolved.

use crate::osv_git::map_git_events;
use crate::registry::{MapClient, RegistryClient};
use crate::version::{cmp_version, parse_requirement};

pub use crate::osv_git::{CachingGitRanges, GitRangeSource, NoGit};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const OSV_VULN_URL: &str = "https://api.osv.dev/v1/vulns/";

/// One affected package OSV named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedComponent {
    /// OSV ecosystem, empty when the record only had `name-version` tags.
    pub ecosystem: String,
    /// Component name to search for.
    pub name: String,
    /// Package URL, filled in when OSV omitted it.
    pub purl: String,
    /// Requirements, any one of which matches. `>=0,<0.103.13`.
    pub version_reqs: Vec<String>,
    /// Exact versions, any one of which matches.
    pub versions: Vec<String>,
    /// Ranges and events that were not parsed into a requirement.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<String>,
}

/// What one OSV id resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsvResolution {
    /// Id that was asked for.
    pub id: String,
    /// Affected packages that name a version.
    pub components: Vec<ResolvedComponent>,
    /// Alias fetches that failed. The packages above are still usable.
    pub notes: Vec<String>,
    /// Ranges with no package name, and other drops that are not on a component.
    pub unresolved: Vec<String>,
}

/// `pkg:type/namespace/name@version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPurl {
    /// `cargo`, `pypi`, `generic`, …
    pub kind: String,
    /// OSV ecosystem for [`Self::kind`], when there is a usual one.
    pub ecosystem: String,
    /// Namespace, when the purl has one.
    pub namespace: Option<String>,
    /// Package name.
    pub name: String,
    /// Version, when the purl pins one.
    pub version: Option<String>,
}

/// Parse a package URL into the name and version a reach query uses.
pub fn parse_purl(raw: &str) -> Result<ParsedPurl, String> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix("pkg:")
        .ok_or_else(|| format!("purl must start with pkg: ({raw})"))?;
    let without_sub = rest.split_once('#').map(|(path, _)| path).unwrap_or(rest);
    let without_qual = without_sub
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(without_sub);
    let (path, version) = match without_qual.split_once('@') {
        Some((path, version)) => (path, Some(percent_decode(version)?)),
        None => (without_qual, None),
    };
    let mut parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(percent_decode)
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() < 2 {
        return Err(format!("purl needs a type and a name ({raw})"));
    }
    let kind = parts.remove(0);
    let name = parts.pop().unwrap_or_default();
    if name.is_empty() {
        return Err(format!("purl needs a name ({raw})"));
    }
    let namespace = if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    };
    Ok(ParsedPurl {
        ecosystem: ecosystem_of_purl(&kind),
        kind,
        namespace,
        name,
        version,
    })
}

/// Fetch `id` and the advisory aliases that carry a package.
///
/// GIT ranges are not resolved against upstream tags. Use
/// [`resolve_vulnerability_with`] when a [`GitRangeSource`] is available.
pub fn resolve_vulnerability(
    id: &str,
    client: &dyn RegistryClient,
) -> Result<OsvResolution, String> {
    resolve_vulnerability_with(id, client, &NoGit)
}

/// Fetch `id`, and map GIT ranges with `git` when the advisory has no versions.
pub fn resolve_vulnerability_with(
    id: &str,
    client: &dyn RegistryClient,
    git: &dyn GitRangeSource,
) -> Result<OsvResolution, String> {
    let id = id.trim();
    if id.is_empty() || id.chars().any(|c| c.is_whitespace() || c == '/') {
        return Err(format!("not an OSV id: {id}"));
    }
    let primary = fetch_vuln(client, id)?;
    let mut notes = Vec::new();
    let mut map: BTreeMap<(String, String), ResolvedComponent> = BTreeMap::new();
    let mut unresolved = absorb_document(&primary, &mut map, git);
    let mut seen = BTreeSet::from([id.to_string()]);
    let mut fetched = 0usize;
    for alias in aliases_of(&primary) {
        if !seen.insert(alias.clone()) || !advisory_alias(&alias) {
            continue;
        }
        if fetched == 8 {
            notes.push(format!("stopped after 8 advisory aliases of {id}"));
            break;
        }
        fetched += 1;
        match fetch_vuln(client, &alias) {
            Ok(document) => unresolved.extend(absorb_document(&document, &mut map, git)),
            Err(error) => notes.push(format!("{alias}: {error}")),
        }
    }
    // A GIT range with no package name is the repository's last path segment.
    // When an alias already named that package in a language ecosystem, the
    // tag bounds belong on that package. The language purl is what matches an
    // extension; a generic purl would not.
    fold_repo_name_into_ecosystem(&mut map);
    let mut components: Vec<ResolvedComponent> = map.into_values().collect();
    for component in &mut components {
        component.version_reqs = dedup(std::mem::take(&mut component.version_reqs));
        component.versions = dedup(std::mem::take(&mut component.versions));
        if component.purl.is_empty() {
            component.purl = synthetic_purl(&component.ecosystem, &component.name);
        }
    }
    components.retain(|component| {
        !component.name.is_empty()
            && (!component.version_reqs.is_empty()
                || !component.versions.is_empty()
                || !component.unresolved.is_empty())
    });
    components
        .sort_by(|left, right| (&left.ecosystem, &left.name).cmp(&(&right.ecosystem, &right.name)));
    if components.is_empty() && unresolved.is_empty() {
        return Err(format!(
            "OSV record {id} names no package version a recipe can match"
        ));
    }
    Ok(OsvResolution {
        id: id.to_string(),
        components,
        notes,
        unresolved,
    })
}

/// Read OSV responses saved under `dir`.
///
/// `queries.json` records the date and each request URL. The named files are
/// the bodies returned for those URLs, so a later run does not ask the network.
pub fn load_recorded_osv(dir: &Path) -> Result<MapClient, String> {
    let manifest_path = dir.join("queries.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("parse {}: {error}", manifest_path.display()))?;
    let queries = manifest
        .get("queries")
        .and_then(|value| value.as_array())
        .ok_or_else(|| format!("{} has no queries", manifest_path.display()))?;
    let mut pages = BTreeMap::new();
    for query in queries {
        let url = query
            .get("url")
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("{} query has no url", manifest_path.display()))?;
        let file = query
            .get("file")
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("{} query has no file", manifest_path.display()))?;
        let path = dir.join(file);
        let body =
            std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        pages.insert(url.to_string(), body);
    }
    Ok(MapClient { pages })
}

fn fetch_vuln(client: &dyn RegistryClient, id: &str) -> Result<serde_json::Value, String> {
    let url = format!("{OSV_VULN_URL}{id}");
    let bytes = client
        .get(&url)
        .map_err(|error| format!("OSV {id}: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("OSV {id}: {error}"))
}

fn absorb_document(
    document: &serde_json::Value,
    map: &mut BTreeMap<(String, String), ResolvedComponent>,
    git: &dyn GitRangeSource,
) -> Vec<String> {
    let Some(affected) = document.get("affected").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut unresolved = Vec::new();
    for entry in affected {
        unresolved.extend(absorb_affected(entry, map, git));
    }
    unresolved
}

fn absorb_affected(
    entry: &serde_json::Value,
    map: &mut BTreeMap<(String, String), ResolvedComponent>,
    git: &dyn GitRangeSource,
) -> Vec<String> {
    let package = entry
        .get("package")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let ecosystem = text_field(&package, "ecosystem");
    let name = text_field(&package, "name");
    let purl = text_field(&package, "purl");
    let (reqs, dropped) = range_reqs(entry, git);
    let listed = string_list(entry, "versions");
    if !name.is_empty() {
        let slot = map
            .entry((ecosystem.clone(), name.clone()))
            .or_insert_with(|| ResolvedComponent {
                ecosystem: ecosystem.clone(),
                name: name.clone(),
                purl: purl.clone(),
                version_reqs: Vec::new(),
                versions: Vec::new(),
                unresolved: Vec::new(),
            });
        if slot.purl.is_empty() {
            slot.purl = purl;
        }
        slot.version_reqs.extend(reqs);
        slot.versions.extend(release_versions(&listed, &name));
        slot.unresolved.extend(dropped);
        return Vec::new();
    }
    // A converted CVE often has no package. `libXpm-3.5.17` is still a name
    // and a version. Git tags such as `XORG-6_8_0` are not.
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in &listed {
        if let Some((tagged, version)) = split_named_version(raw) {
            by_name.entry(tagged).or_default().push(version);
        }
    }
    let covered = !by_name.is_empty() && entry_has_git(entry);
    if by_name.is_empty() && !reqs.is_empty() {
        if let Some(inferred) = repo_name(entry) {
            let slot = map
                .entry((String::new(), inferred.clone()))
                .or_insert_with(|| ResolvedComponent {
                    ecosystem: String::new(),
                    name: inferred,
                    purl: String::new(),
                    version_reqs: Vec::new(),
                    versions: Vec::new(),
                    unresolved: Vec::new(),
                });
            slot.version_reqs.extend(reqs);
            slot.unresolved.extend(dropped);
            return Vec::new();
        }
    }
    for (tagged, versions) in by_name {
        let slot = map
            .entry((String::new(), tagged.clone()))
            .or_insert_with(|| ResolvedComponent {
                ecosystem: String::new(),
                name: tagged.clone(),
                purl: String::new(),
                version_reqs: Vec::new(),
                versions: Vec::new(),
                unresolved: Vec::new(),
            });
        if covered {
            if let Some(span) = span_req(&versions) {
                slot.version_reqs.push(span);
            }
        }
        slot.versions.extend(versions);
    }
    // Named tags already carry the releases the GIT commits map onto, so the
    // unparsed-GIT note is not returned with them. A failed tag lookup still is.
    if covered {
        return dropped
            .into_iter()
            .filter(|item| item != "GIT range not parsed")
            .collect();
    }
    dropped
}

fn range_reqs(entry: &serde_json::Value, git: &dyn GitRangeSource) -> (Vec<String>, Vec<String>) {
    let Some(ranges) = entry.get("ranges").and_then(|value| value.as_array()) else {
        return (Vec::new(), Vec::new());
    };
    let package_name = text_field(
        &entry
            .get("package")
            .cloned()
            .unwrap_or(serde_json::json!({})),
        "name",
    );
    let listed = string_list(entry, "versions");
    let mut reqs = Vec::new();
    let mut unresolved = Vec::new();
    for range in ranges {
        let kind = range
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let label = if kind.is_empty() { "range" } else { kind };
        if kind == "GIT" {
            if versions_cover_git(&package_name, &listed) {
                // The advisory already enumerated the releases those commits
                // affect. The span is that set; exact versions are stored too.
                if !package_name.is_empty() {
                    let releases = release_versions(&listed, &package_name);
                    if let Some(span) = span_req(&releases) {
                        push_req(&mut reqs, &mut unresolved, &span);
                    }
                }
                continue;
            }
            let repo = range
                .get("repo")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let Some(events) = range.get("events").and_then(|value| value.as_array()) else {
                unresolved.push("GIT range has no events".to_string());
                continue;
            };
            match map_git_events(repo, events, git) {
                Ok(parsed) => {
                    for req in parsed {
                        push_req(&mut reqs, &mut unresolved, &req);
                    }
                }
                Err(_) => unresolved.push("GIT range not parsed".to_string()),
            }
            continue;
        }
        if kind != "SEMVER" && kind != "ECOSYSTEM" {
            unresolved.push(format!("{label} range not parsed"));
            continue;
        }
        let Some(events) = range.get("events").and_then(|value| value.as_array()) else {
            unresolved.push(format!("{label} range has no events"));
            continue;
        };
        let (parsed, dropped) = events_to_reqs(events);
        unresolved.extend(dropped);
        reqs.extend(parsed);
    }
    (reqs, unresolved)
}

fn events_to_reqs(events: &[serde_json::Value]) -> (Vec<String>, Vec<String>) {
    let mut reqs = Vec::new();
    let mut dropped = Vec::new();
    let mut open: Option<String> = None;
    for event in events {
        let Some(object) = event.as_object() else {
            dropped.push("event not parsed".to_string());
            continue;
        };
        let Some((key, value)) = object.iter().next() else {
            dropped.push("event not parsed".to_string());
            continue;
        };
        let Some(bound) = value.as_str() else {
            dropped.push(format!("event {key} not parsed"));
            continue;
        };
        let bound = normalize_bound(bound);
        match key.as_str() {
            "introduced" => {
                if let Some(start) = open.take() {
                    push_req(&mut reqs, &mut dropped, &format!(">={start}"));
                }
                open = Some(bound);
            }
            "fixed" => {
                let start = open.take().unwrap_or_else(|| "0".to_string());
                push_req(&mut reqs, &mut dropped, &format!(">={start},<{bound}"));
            }
            "last_affected" => {
                let start = open.take().unwrap_or_else(|| "0".to_string());
                push_req(&mut reqs, &mut dropped, &format!(">={start},<={bound}"));
            }
            _ => dropped.push(format!("event {key} not parsed")),
        }
    }
    if let Some(start) = open {
        push_req(&mut reqs, &mut dropped, &format!(">={start}"));
    }
    (reqs, dropped)
}

fn push_req(reqs: &mut Vec<String>, dropped: &mut Vec<String>, req: &str) {
    if parse_requirement(req).is_ok() {
        reqs.push(req.to_string());
    } else {
        dropped.push(format!("requirement not parsed: {req}"));
    }
}

fn release_versions(raws: &[String], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in raws {
        if let Some(rest) = raw.strip_prefix("v/").or_else(|| raw.strip_prefix('v')) {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                out.push(rest.to_string());
                continue;
            }
        }
        if raw.starts_with(|c: char| c.is_ascii_digit()) {
            out.push(raw.clone());
            continue;
        }
        if let Some((tagged, version)) = split_named_version(raw) {
            if tagged.eq_ignore_ascii_case(name) {
                out.push(version);
            }
        }
    }
    out
}

/// `versions` names the releases a GIT range affects when the package is
/// known, or when nameless entries use `name-version` tags.
fn versions_cover_git(package_name: &str, listed: &[String]) -> bool {
    if !package_name.is_empty() {
        return !release_versions(listed, package_name).is_empty();
    }
    listed.iter().any(|raw| split_named_version(raw).is_some())
}

fn entry_has_git(entry: &serde_json::Value) -> bool {
    entry
        .get("ranges")
        .and_then(|value| value.as_array())
        .is_some_and(|ranges| {
            ranges
                .iter()
                .any(|range| range.get("type").and_then(|value| value.as_str()) == Some("GIT"))
        })
}

fn span_req(versions: &[String]) -> Option<String> {
    let mut ordered: Vec<&str> = versions.iter().map(String::as_str).collect();
    if ordered.is_empty() {
        return None;
    }
    ordered.sort_by(|left, right| cmp_version(left, right).then(left.cmp(right)));
    ordered.dedup();
    let first = ordered[0];
    let last = ordered[ordered.len() - 1];
    Some(format!(">={first},<={last}"))
}

fn repo_name(entry: &serde_json::Value) -> Option<String> {
    let ranges = entry.get("ranges")?.as_array()?;
    for range in ranges {
        if range.get("type").and_then(|value| value.as_str()) != Some("GIT") {
            continue;
        }
        let repo = range
            .get("repo")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        if let Some(name) = name_from_repo(repo) {
            return Some(name);
        }
    }
    None
}

fn name_from_repo(repo: &str) -> Option<String> {
    let trimmed = repo.trim().trim_end_matches('/').trim_end_matches(".git");
    let name = trimmed.rsplit('/').next()?;
    let mut chars = name.chars();
    if !chars.next()?.is_ascii_alphabetic() {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return None;
    }
    Some(name.to_string())
}

/// Move a repository-named component onto the language package of the same name.
fn fold_repo_name_into_ecosystem(map: &mut BTreeMap<(String, String), ResolvedComponent>) {
    let bare_keys: Vec<(String, String)> = map
        .keys()
        .filter(|(ecosystem, _)| ecosystem.is_empty())
        .cloned()
        .collect();
    for key in bare_keys {
        let name = key.1.clone();
        let targets: Vec<(String, String)> = map
            .keys()
            .filter(|(ecosystem, other)| {
                language_ecosystem(ecosystem) && other.eq_ignore_ascii_case(&name)
            })
            .cloned()
            .collect();
        if targets.is_empty() {
            continue;
        }
        let Some(bare) = map.remove(&key) else {
            continue;
        };
        for target in targets {
            if let Some(slot) = map.get_mut(&target) {
                slot.version_reqs.extend(bare.version_reqs.iter().cloned());
                slot.versions.extend(bare.versions.iter().cloned());
                slot.unresolved.extend(bare.unresolved.iter().cloned());
                if slot.purl.is_empty() {
                    slot.purl.clone_from(&bare.purl);
                }
            }
        }
    }
}

fn language_ecosystem(ecosystem: &str) -> bool {
    matches!(
        ecosystem,
        "PyPI" | "crates.io" | "Go" | "npm" | "RubyGems" | "Maven"
    )
}

fn split_named_version(raw: &str) -> Option<(String, String)> {
    let (name, version) = raw.rsplit_once('-')?;
    if !name.chars().next()?.is_ascii_alphabetic() || !dotted_version(version) {
        return None;
    }
    Some((name.to_string(), version.to_string()))
}

fn dotted_version(version: &str) -> bool {
    let mut parts = version.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    if first.is_empty() || !first.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let mut more = false;
    for part in parts {
        more = true;
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    more
}

fn normalize_bound(raw: &str) -> String {
    match raw.trim() {
        "0.0.0-0" | "0.0.0.0" => "0".to_string(),
        other => other.to_string(),
    }
}

fn aliases_of(document: &serde_json::Value) -> Vec<String> {
    string_list(document, "aliases")
}

fn advisory_alias(id: &str) -> bool {
    id.starts_with("GHSA-")
        || id.starts_with("RUSTSEC-")
        || id.starts_with("PYSEC-")
        || id.starts_with("GO-")
}

fn string_list(value: &serde_json::Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn text_field(value: &serde_json::Value, field: &str) -> String {
    value
        .get(field)
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string()
}

fn dedup(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn synthetic_purl(ecosystem: &str, name: &str) -> String {
    let kind = match ecosystem {
        "crates.io" => "cargo",
        "PyPI" => "pypi",
        "Go" => "golang",
        "npm" => "npm",
        "RubyGems" => "gem",
        "Maven" => "maven",
        _ => "generic",
    };
    format!("pkg:{kind}/{name}")
}

fn ecosystem_of_purl(kind: &str) -> String {
    match kind {
        "cargo" => "crates.io".to_string(),
        "pypi" => "PyPI".to_string(),
        "golang" => "Go".to_string(),
        "npm" => "npm".to_string(),
        "gem" => "RubyGems".to_string(),
        "maven" => "Maven".to_string(),
        other => other.to_string(),
    }
}

fn percent_decode(raw: &str) -> Result<String, String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(format!("bad percent escape in {raw}"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| format!("bad percent escape in {raw}"))?;
            let value =
                u8::from_str_radix(hex, 16).map_err(|_| format!("bad percent escape in {raw}"))?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("purl is not utf-8: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MapClient;
    use std::collections::BTreeMap;

    #[test]
    fn a_purl_names_the_component_and_its_version() {
        let purl = parse_purl("pkg:cargo/rustls-webpki@0.102.8").unwrap();
        assert_eq!(purl.kind, "cargo");
        assert_eq!(purl.ecosystem, "crates.io");
        assert_eq!(purl.name, "rustls-webpki");
        assert_eq!(purl.version.as_deref(), Some("0.102.8"));
        let pypi = parse_purl("pkg:pypi/ruamel.yaml@0.18.6").unwrap();
        assert_eq!(pypi.name, "ruamel.yaml");
        let namespaced = parse_purl("pkg:github/rustls/webpki@v0.102.8").unwrap();
        assert_eq!(namespaced.namespace.as_deref(), Some("rustls"));
        assert_eq!(namespaced.name, "webpki");
        assert!(parse_purl("rustls-webpki@0.102.8").is_err());
    }

    #[test]
    fn a_cve_follows_the_advisory_and_keeps_named_tags() {
        let mut pages = BTreeMap::new();
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2026-93599".into(),
            br#"{"id":"CVE-2026-93599","aliases":["GHSA-82j2","RUSTSEC-2026-0104","SUSE-SU-1"],"affected":[{"ranges":[{"type":"GIT","events":[{"introduced":"0"},{"fixed":"abc"}]}],"versions":["v/0.103.12","0.17.0"]}]}"#.to_vec(),
        );
        pages.insert(
            "https://api.osv.dev/v1/vulns/RUSTSEC-2026-0104".into(),
            br#"{"id":"RUSTSEC-2026-0104","affected":[{"package":{"ecosystem":"crates.io","name":"rustls-webpki","purl":"pkg:cargo/rustls-webpki"},"ranges":[{"type":"SEMVER","events":[{"introduced":"0.0.0-0"},{"fixed":"0.103.13"},{"introduced":"0.104.0-alpha.1"},{"fixed":"0.104.0-alpha.7"}]}]}]}"#.to_vec(),
        );
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2026-94287".into(),
            br#"{"id":"CVE-2026-94287","affected":[{"ranges":[{"type":"GIT","events":[{"introduced":"0"}]}],"versions":["libXpm-3.5.17","libXpm-3.5.18","XORG-6_8_0","libXpm-3_5_5"]}]}"#.to_vec(),
        );
        let client = MapClient { pages };
        let webpki = resolve_vulnerability("CVE-2026-93599", &client).unwrap();
        assert!(
            webpki.notes.iter().any(|note| note.contains("GHSA-82j2")),
            "{:?}",
            webpki.notes
        );
        assert_eq!(webpki.components.len(), 1, "{:?}", webpki.components);
        let component = &webpki.components[0];
        assert_eq!(component.name, "rustls-webpki");
        assert_eq!(component.ecosystem, "crates.io");
        assert_eq!(component.purl, "pkg:cargo/rustls-webpki");
        assert!(component
            .version_reqs
            .iter()
            .any(|req| req == ">=0,<0.103.13"));
        assert!(component
            .version_reqs
            .iter()
            .any(|req| req == ">=0.104.0-alpha.1,<0.104.0-alpha.7"));
        let libxpm = resolve_vulnerability("CVE-2026-94287", &client).unwrap();
        assert_eq!(libxpm.components.len(), 1);
        assert_eq!(libxpm.components[0].name, "libXpm");
        assert_eq!(
            libxpm.components[0].versions,
            vec!["3.5.17".to_string(), "3.5.18".to_string()]
        );
        assert_eq!(
            libxpm.components[0].version_reqs,
            vec![">=3.5.17,<=3.5.18".to_string()]
        );
        assert_eq!(libxpm.components[0].purl, "pkg:generic/libXpm");
        assert!(
            libxpm
                .unresolved
                .iter()
                .all(|item| !item.contains("GIT range not parsed")),
            "{:?}",
            libxpm.unresolved
        );
    }

    #[test]
    fn the_recorded_eessi_advisories_resolve_offline() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/component_reach_eessi/osv");
        let manifest = std::fs::read_to_string(dir.join("queries.json")).expect("queries");
        assert!(manifest.contains("\"fetched\": \"2026-10-24\""));
        let client = load_recorded_osv(&dir).expect("load recorded osv");
        let webpki = resolve_vulnerability("CVE-2026-93599", &client).unwrap();
        assert!(
            webpki
                .components
                .iter()
                .any(|item| item.name == "rustls-webpki" && item.ecosystem == "crates.io"),
            "{:?}",
            webpki.components
        );
        let urllib = resolve_vulnerability("CVE-2025-66471", &client).unwrap();
        assert!(
            urllib
                .components
                .iter()
                .any(|item| item.name == "urllib3" && item.ecosystem == "PyPI"),
            "{:?}",
            urllib.components
        );
        let libxpm = resolve_vulnerability("CVE-2026-94287", &client).unwrap();
        assert!(
            libxpm.components.iter().any(|item| {
                item.name == "libXpm" && item.versions.iter().any(|version| version == "3.5.17")
            }),
            "{:?}",
            libxpm.components
        );
    }

    #[test]
    fn an_unparsed_range_is_kept() {
        let mut pages = BTreeMap::new();
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2099-0001".into(),
            br#"{"id":"CVE-2099-0001","affected":[{"package":{"ecosystem":"Debian","name":"curl","purl":"pkg:deb/debian/curl"},"ranges":[{"type":"GIT","events":[{"introduced":"0"},{"fixed":"abc"}]},{"type":"ECOSYSTEM","events":[{"introduced":"0"},{"fixed":"1.2.3,ubuntu"}]}]}]}"#.to_vec(),
        );
        let resolution = resolve_vulnerability("CVE-2099-0001", &MapClient { pages }).unwrap();
        assert_eq!(
            resolution.components.len(),
            1,
            "{:?}",
            resolution.components
        );
        let component = &resolution.components[0];
        assert!(
            component.version_reqs.is_empty(),
            "{:?}",
            component.version_reqs
        );
        assert!(component.versions.is_empty());
        let text = component.unresolved.join("\n");
        assert!(text.contains("GIT range not parsed"), "{text}");
        assert!(
            text.contains("requirement not parsed") && text.contains("1.2.3,ubuntu"),
            "{text}"
        );
    }

    #[test]
    fn a_git_range_with_a_versions_list_maps_onto_those_releases() {
        let mut pages = BTreeMap::new();
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2099-1000".into(),
            br#"{"id":"CVE-2099-1000","affected":[{"package":{"ecosystem":"PyPI","name":"urllib3","purl":"pkg:pypi/urllib3"},"ranges":[{"type":"GIT","repo":"https://github.com/urllib3/urllib3","events":[{"introduced":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"fixed":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}],"versions":["2.5.0","1.0.0"]}]}"#.to_vec(),
        );
        // NoGit would fail a tag lookup. The versions list has to be enough.
        let resolution =
            resolve_vulnerability_with("CVE-2099-1000", &MapClient { pages }, &NoGit).unwrap();
        assert!(
            resolution.unresolved.is_empty(),
            "{:?}",
            resolution.unresolved
        );
        assert_eq!(
            resolution.components.len(),
            1,
            "{:?}",
            resolution.components
        );
        let component = &resolution.components[0];
        assert_eq!(component.name, "urllib3");
        assert_eq!(
            component.versions,
            vec!["1.0.0".to_string(), "2.5.0".to_string()]
        );
        assert_eq!(component.version_reqs, vec![">=1.0.0,<=2.5.0".to_string()]);
        assert!(
            component.unresolved.is_empty(),
            "{:?}",
            component.unresolved
        );
        let counts = reach_urllib(&resolution);
        assert_eq!(counts.0, 1, "affected 2.5.0 is a carrier");
        assert_eq!(counts.1, 0);
    }

    #[test]
    fn a_git_only_range_stays_unparsed_when_offline() {
        let mut pages = BTreeMap::new();
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2099-1001".into(),
            br#"{"id":"CVE-2099-1001","affected":[{"package":{"ecosystem":"PyPI","name":"urllib3","purl":"pkg:pypi/urllib3"},"ranges":[{"type":"GIT","repo":"https://github.com/urllib3/urllib3","events":[{"introduced":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"last_affected":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}]}]}"#.to_vec(),
        );
        let resolution =
            resolve_vulnerability_with("CVE-2099-1001", &MapClient { pages }, &NoGit).unwrap();
        assert_eq!(
            resolution.components.len(),
            1,
            "{:?}",
            resolution.components
        );
        let component = &resolution.components[0];
        assert!(
            component.version_reqs.is_empty(),
            "{:?}",
            component.version_reqs
        );
        assert!(component.versions.is_empty());
        assert!(
            component
                .unresolved
                .iter()
                .any(|item| item.contains("GIT range not parsed")),
            "{:?}",
            component.unresolved
        );
        let counts = reach_urllib(&resolution);
        assert_eq!(counts.0, 0);
        assert_eq!(counts.1, 1, "unparsed GIT range counts as review");
    }

    #[test]
    fn a_mixed_semver_and_git_range_keeps_both() {
        let mut pages = BTreeMap::new();
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2099-1002".into(),
            br#"{"id":"CVE-2099-1002","affected":[{"package":{"ecosystem":"PyPI","name":"urllib3","purl":"pkg:pypi/urllib3"},"ranges":[{"type":"SEMVER","events":[{"introduced":"1.0.0"},{"fixed":"2.6.0"}]},{"type":"GIT","repo":"https://github.com/urllib3/urllib3","events":[{"introduced":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},{"fixed":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}],"versions":["1.0.0","2.5.0"]}]}"#.to_vec(),
        );
        let resolution =
            resolve_vulnerability_with("CVE-2099-1002", &MapClient { pages }, &NoGit).unwrap();
        let component = &resolution.components[0];
        assert!(
            component
                .version_reqs
                .iter()
                .any(|req| req == ">=1.0.0,<2.6.0"),
            "{:?}",
            component.version_reqs
        );
        assert!(
            component
                .version_reqs
                .iter()
                .any(|req| req == ">=1.0.0,<=2.5.0"),
            "{:?}",
            component.version_reqs
        );
        assert_eq!(
            component.versions,
            vec!["1.0.0".to_string(), "2.5.0".to_string()]
        );
        assert!(
            component
                .unresolved
                .iter()
                .all(|item| !item.contains("GIT range not parsed")),
            "{:?}",
            component.unresolved
        );
        let counts = reach_urllib(&resolution);
        assert_eq!(counts.0, 1);
        assert_eq!(counts.1, 0);
    }

    /// `(carriers, review)` for urllib3 2.5.0 and 2.7.0 in generation 2026.1.
    fn reach_urllib(resolution: &OsvResolution) -> (usize, usize) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("old.eb"),
            "\
easyblock = 'PythonBundle'\n\
name = 'old'\n\
version = '1'\n\
toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
exts_list = [('urllib3', '2.5.0', {})]\n\
",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("new.eb"),
            "\
easyblock = 'PythonBundle'\n\
name = 'new'\n\
version = '1'\n\
toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
exts_list = [('urllib3', '2.7.0', {})]\n\
",
        )
        .unwrap();
        let parsed = crate::eb_parse::parse_easyconfig_tree(dir.path()).unwrap();
        let filter = crate::dep_filter::FilterDeps::empty();
        let document = crate::component_reach::cve_reach(
            &resolution.id,
            &resolution.components,
            &resolution.notes,
            &resolution.unresolved,
            &crate::component_reach::ReachScope {
                candidates: &parsed.candidates,
                generation: "2026.1",
                installed: None,
                easystacks: &[],
                compat: &[],
                filter_deps: &filter,
                generated_at: "2026-11-01T00:00:00Z",
            },
        );
        let carriers = document
            .results
            .iter()
            .map(|item| item.carriers.len())
            .sum();
        let review = document
            .results
            .iter()
            .map(|item| item.closure_counts.review)
            .sum();
        if carriers == 1 {
            assert!(document.results[0].carriers.iter().any(|row| {
                row.version == "2.5.0" && row.role == "extension" && row.easyconfig == "old.eb"
            }));
            assert!(document.results[0]
                .carriers
                .iter()
                .all(|row| row.version != "2.7.0"));
        }
        (carriers, review)
    }
}
