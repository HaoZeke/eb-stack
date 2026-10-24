//! A package URL, or a CVE id resolved through OSV.
//!
//! The CVE record often has only a git range. The package name and the
//! versions EasyBuild actually pins are on the GHSA or RUSTSEC alias, or in
//! `name-version` tags such as `libXpm-3.5.17`.

use crate::registry::RegistryClient;
use crate::version::parse_requirement;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

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
pub fn resolve_vulnerability(
    id: &str,
    client: &dyn RegistryClient,
) -> Result<OsvResolution, String> {
    let id = id.trim();
    if id.is_empty() || id.chars().any(|c| c.is_whitespace() || c == '/') {
        return Err(format!("not an OSV id: {id}"));
    }
    let primary = fetch_vuln(client, id)?;
    let mut notes = Vec::new();
    let mut map: BTreeMap<(String, String), ResolvedComponent> = BTreeMap::new();
    let mut unresolved = absorb_document(&primary, &mut map);
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
            Ok(document) => unresolved.extend(absorb_document(&document, &mut map)),
            Err(error) => notes.push(format!("{alias}: {error}")),
        }
    }
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
) -> Vec<String> {
    let Some(affected) = document.get("affected").and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut unresolved = Vec::new();
    for entry in affected {
        unresolved.extend(absorb_affected(entry, map));
    }
    unresolved
}

fn absorb_affected(
    entry: &serde_json::Value,
    map: &mut BTreeMap<(String, String), ResolvedComponent>,
) -> Vec<String> {
    let package = entry
        .get("package")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    let ecosystem = text_field(&package, "ecosystem");
    let name = text_field(&package, "name");
    let purl = text_field(&package, "purl");
    let (reqs, dropped) = range_reqs(entry);
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
        slot.versions.extend(plain_versions(entry, &name));
        slot.unresolved.extend(dropped);
        return Vec::new();
    }
    // A converted CVE often has no package. `libXpm-3.5.17` is still a name
    // and a version. Git tags such as `XORG-6_8_0` are not.
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in string_list(entry, "versions") {
        if let Some((tagged, version)) = split_named_version(&raw) {
            by_name.entry(tagged).or_default().push(version);
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
        slot.versions.extend(versions);
    }
    dropped
}

fn range_reqs(entry: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let Some(ranges) = entry.get("ranges").and_then(|value| value.as_array()) else {
        return (Vec::new(), Vec::new());
    };
    let mut reqs = Vec::new();
    let mut unresolved = Vec::new();
    for range in ranges {
        let kind = range
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let label = if kind.is_empty() { "range" } else { kind };
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

fn plain_versions(entry: &serde_json::Value, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in string_list(entry, "versions") {
        if let Some(rest) = raw.strip_prefix("v/").or_else(|| raw.strip_prefix('v')) {
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                out.push(rest.to_string());
                continue;
            }
        }
        if raw.starts_with(|c: char| c.is_ascii_digit()) {
            out.push(raw);
            continue;
        }
        if let Some((tagged, version)) = split_named_version(&raw) {
            if tagged.eq_ignore_ascii_case(name) {
                out.push(version);
            }
        }
    }
    out
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
        assert!(libxpm.components[0].version_reqs.is_empty());
        assert_eq!(libxpm.components[0].purl, "pkg:generic/libXpm");
        assert!(
            libxpm
                .unresolved
                .iter()
                .any(|item| item.contains("GIT range not parsed")),
            "{:?}",
            libxpm.unresolved
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
}
