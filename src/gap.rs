//! What a site provides that a stack does not, and the recipe that would close it.
//!
//! A site's module tree names the software its users load. A stack such as an
//! EESSI version names what it installs. For every package the site has and
//! the stack lacks, the gap says which route closes it, in this order:
//! site-only (`excluded`), filtered to the compatibility layer, already in an
//! open software-layer pull request, a recipe on the generation in the
//! easyconfig trees, an open easybuild-easyconfigs pull request at that
//! generation, or a bump (`no-recipe`). Recipes on the generation are the
//! roots `stack batch` plans pull requests from.

use crate::domain::Candidate;
use crate::easystack::filename_toolchain;
use crate::hierarchy::is_system_toolchain;
use crate::jenkins::package_name;
use crate::pr_overlap::{self, added_entries, FileChange, PrInput};
use crate::version::cmp_version;
use anyhow::{Context, Result};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One site package and what closing it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GapRow {
    /// EasyBuild name, as the site's module tree spells it.
    pub name: String,
    /// `in-stack`, `excluded`, `filtered`, `open-layer`, `recipe`,
    /// `open-easyconfigs`, or `no-recipe`.
    pub route: String,
    /// The easyconfig on the generation, for `recipe`, or the filename an
    /// open pull request adds, for `open-layer` and `open-easyconfigs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// The newest easyconfig of the name on any toolchain, for `no-recipe`:
    /// the source a bump starts from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_elsewhere: Option<String>,
    /// Why a name is `excluded` or `filtered`, or the pull requests that
    /// already carry an `open-layer` or `open-easyconfigs` name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A filename an open pull request adds, and the pull requests that add it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCarry {
    /// Easyconfig filename, without a directory.
    pub file: String,
    /// `software-layer#1585` or `easyconfigs#24388`. Several are joined
    /// with `", "` in the order the pull requests were read.
    pub label: String,
}

/// Module names and the versions the live listing exposes.
///
/// A name with an empty set came from a bare module name, so its version is
/// unknown rather than absent from the inventory.
pub type ModuleInventory = BTreeMap<String, BTreeSet<String>>;

/// The lists [`gap`] reads.
pub struct GapInput<'a> {
    /// Site package names.
    pub site: &'a BTreeSet<String>,
    /// Names the stack already installs.
    pub stack: &'a BTreeSet<String>,
    /// Easyconfigs the trees parsed.
    pub candidates: &'a [Candidate],
    /// Toolchain generation, `2026.1`. A toolchain of this version is on
    /// the generation even when `members` does not list it (`lfoss-2026.1`).
    pub generation: &'a str,
    /// Toolchain labels of the generation (`GCCcore-15.2.0`, `foss-2026.1`,
    /// `llvm-compilers-21.1.8`).
    pub members: &'a [String],
    /// Names the stack does not carry (licensed, vendor, conda, portal),
    /// mapped to the reason.
    pub excluded: &'a BTreeMap<String, String>,
    /// Names filtered to the compatibility layer, matched without regard
    /// to case.
    pub filtered: &'a BTreeSet<String>,
    /// Names an open software-layer pull request adds, keyed by the
    /// lower-case package name.
    pub open_layer: &'a BTreeMap<String, OpenCarry>,
    /// Names an open easyconfigs pull request adds at this generation,
    /// keyed by the lower-case package name.
    pub open_easyconfigs: &'a BTreeMap<String, OpenCarry>,
}

/// Package names from a module listing: `Name/Version` lines, bare names, or
/// `Name` directories. Blank lines and `#` comments are skipped.
pub fn names_from_listing(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split('/').next())
        .map(|n| n.trim_end_matches(".lua").to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Package names from a listing file or an Lmod module tree.
///
/// A directory is walked: `Name/version.lua` under `modules/all`,
/// `Compiler`, and `MPI` all count, and `.modulerc.lua` and `default` do
/// not. A file is `module -t avail`, `module spider -t`, or the same
/// `Name/version` lines as [`names_from_listing`].
pub fn names_from_module_source(path: &Path) -> Result<BTreeSet<String>> {
    Ok(inventory_from_module_source(path)?.into_keys().collect())
}

/// Module names and versions from a listing file or an Lmod module tree.
///
/// Versions are kept as Lmod spells them, including any toolchain suffix.
/// Bare names have an empty version set.
pub fn inventory_from_module_source(path: &Path) -> Result<ModuleInventory> {
    if path.is_dir() {
        inventory_from_module_tree(path).with_context(|| path.display().to_string())
    } else {
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        Ok(inventory_from_live_listing(&text))
    }
}

/// Names in `module -t avail` or `module spider -t` output.
///
/// Path headers, `(default)`, and a `Name: Name/version, ...` spider line
/// are read. A plain `Name/version` or bare name is unchanged.
pub fn names_from_live_listing(text: &str) -> BTreeSet<String> {
    inventory_from_live_listing(text).into_keys().collect()
}

/// Module names and versions in `module -t avail` or `module spider -t`
/// output.
pub fn inventory_from_live_listing(text: &str) -> ModuleInventory {
    let mut inventory = ModuleInventory::new();
    for line in text.lines() {
        for module in live_module_tokens(line) {
            let mut parts = module.splitn(2, '/');
            let Some(name) = parts.next() else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let versions = inventory.entry(name.to_string()).or_default();
            if let Some(version) = parts.next().filter(|version| !version.is_empty()) {
                versions.insert(version.to_string());
            }
        }
    }
    inventory
}

fn inventory_from_module_tree(root: &Path) -> std::io::Result<ModuleInventory> {
    let mut inventory = ModuleInventory::new();
    walk_module_tree(root, &mut inventory)?;
    Ok(inventory)
}

/// One tab-separated file per route `stack gap` writes, besides `gap.tsv`.
///
/// `excluded` is the site-only set and `no-recipe` is the bump set. The
/// file names are those words.
pub const ROUTE_FILES: &[(&str, &str)] = &[
    ("in-stack", "in-stack.tsv"),
    ("excluded", "site-only.tsv"),
    ("filtered", "filtered.tsv"),
    ("open-layer", "open-layer.tsv"),
    ("recipe", "recipe.tsv"),
    ("open-easyconfigs", "open-easyconfigs.tsv"),
    ("no-recipe", "bump.tsv"),
];

fn file_of(candidate: &Candidate) -> String {
    basename(&candidate.easyconfig_path)
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn toolchain_label(candidate: &Candidate) -> String {
    format!(
        "{}-{}",
        candidate.toolchain.name, candidate.toolchain.version
    )
}

fn on_generation(candidate: &Candidate, generation: &str, members: &[String]) -> bool {
    is_system_toolchain(&candidate.toolchain)
        || members
            .iter()
            .any(|label| label == &toolchain_label(candidate))
        || candidate.toolchain.version == generation
}

/// Order candidates newest first: version, then a recipe without a
/// versionsuffix before one with, then by file name.
fn newest_first(a: &&Candidate, b: &&Candidate) -> Ordering {
    cmp_version(&b.version, &a.version)
        .then_with(|| {
            a.versionsuffix
                .as_deref()
                .unwrap_or("")
                .len()
                .cmp(&b.versionsuffix.as_deref().unwrap_or("").len())
        })
        .then_with(|| file_of(a).cmp(&file_of(b)))
}

fn reason_for<'a>(map: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    if let Some(reason) = map.get(name) {
        return Some(reason.as_str());
    }
    map.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, reason)| reason.as_str())
}

fn set_has(set: &BTreeSet<String>, name: &str) -> bool {
    set.contains(name) || set.iter().any(|key| key.eq_ignore_ascii_case(name))
}

fn candidates_named<'a>(
    by_name: &BTreeMap<&str, Vec<&'a Candidate>>,
    name: &str,
) -> Vec<&'a Candidate> {
    if let Some(found) = by_name.get(name) {
        return found.clone();
    }
    by_name
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, found)| found.clone())
        .unwrap_or_default()
}

fn row(
    name: &str,
    route: &str,
    recipe: Option<String>,
    elsewhere: Option<String>,
    reason: Option<String>,
) -> GapRow {
    GapRow {
        name: name.to_string(),
        route: route.to_string(),
        recipe,
        newest_elsewhere: elsewhere,
        reason,
    }
}

/// The gap between the site and the stack.
///
/// Stack names are matched without regard to case, as Lmod does. A name the
/// stack already has is `in-stack`. Each other name takes the first route
/// that applies: `excluded`, `filtered`, `open-layer`, `recipe`,
/// `open-easyconfigs`, then `no-recipe`.
pub fn gap(input: &GapInput<'_>) -> Vec<GapRow> {
    let stack_lower: BTreeSet<String> = input.stack.iter().map(|n| n.to_lowercase()).collect();
    let mut by_name: BTreeMap<&str, Vec<&Candidate>> = BTreeMap::new();
    for candidate in input.candidates {
        by_name
            .entry(candidate.name.as_str())
            .or_default()
            .push(candidate);
    }
    let mut rows = Vec::new();
    for name in input.site {
        if stack_lower.contains(&name.to_lowercase()) {
            rows.push(row(name, "in-stack", None, None, None));
            continue;
        }
        if let Some(reason) = reason_for(input.excluded, name) {
            rows.push(row(name, "excluded", None, None, Some(reason.to_string())));
            continue;
        }
        if set_has(input.filtered, name) {
            rows.push(row(
                name,
                "filtered",
                None,
                None,
                Some("filtered to the compatibility layer".to_string()),
            ));
            continue;
        }
        if let Some(carry) = input.open_layer.get(&name.to_lowercase()) {
            rows.push(row(
                name,
                "open-layer",
                Some(carry.file.clone()),
                None,
                Some(carry.label.clone()),
            ));
            continue;
        }
        let mut all = candidates_named(&by_name, name);
        all.sort_by(newest_first);
        if let Some(candidate) = all
            .iter()
            .find(|candidate| on_generation(candidate, input.generation, input.members))
        {
            rows.push(row(name, "recipe", Some(file_of(candidate)), None, None));
            continue;
        }
        if let Some(carry) = input.open_easyconfigs.get(&name.to_lowercase()) {
            rows.push(row(
                name,
                "open-easyconfigs",
                Some(carry.file.clone()),
                None,
                Some(carry.label.clone()),
            ));
            continue;
        }
        rows.push(row(
            name,
            "no-recipe",
            None,
            all.first().map(|candidate| file_of(candidate)),
            None,
        ));
    }
    rows
}

/// The gap as a tab-separated table with a header line.
pub fn render_tsv(rows: &[GapRow]) -> String {
    let mut out = String::from("name\troute\trecipe\tnewest_elsewhere\treason\n");
    for r in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            r.name,
            r.route,
            r.recipe.as_deref().unwrap_or("-"),
            r.newest_elsewhere.as_deref().unwrap_or("-"),
            r.reason.as_deref().unwrap_or("-")
        ));
    }
    out
}

/// One previous-stack module version and where users find it in the new
/// stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionChangeRow {
    /// EasyBuild software name.
    pub name: String,
    /// Version in the site or older-EESSI inventory. `None` for a bare name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_version: Option<String>,
    /// Version in the target EESSI inventory. Multiple target versions are
    /// comma-separated. Set only for `moved`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eessi_version: Option<String>,
    /// `moved`, `gone`, or `site-only`.
    pub status: String,
}

fn inventory_versions<'a>(
    inventory: &'a ModuleInventory,
    name: &str,
) -> Option<&'a BTreeSet<String>> {
    inventory.get(name).or_else(|| {
        inventory
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, versions)| versions)
    })
}

/// Turn gap routes into the user-facing version change table.
///
/// An `in-stack` module moved to EESSI, an `excluded` module remains
/// site-only, and every other route is gone from the opened stack until that
/// route lands. One row is written per old module version; a bare old name
/// still contributes one row with no version.
pub fn version_changes(
    rows: &[GapRow],
    old: &ModuleInventory,
    eessi: &ModuleInventory,
) -> Vec<VersionChangeRow> {
    let mut changes = Vec::new();
    for row in rows {
        let status = match row.route.as_str() {
            "in-stack" => "moved",
            "excluded" => "site-only",
            _ => "gone",
        };
        let eessi_version = (status == "moved")
            .then(|| inventory_versions(eessi, &row.name))
            .flatten()
            .filter(|versions| !versions.is_empty())
            .map(|versions| versions.iter().cloned().collect::<Vec<_>>().join(","));
        let old_versions = inventory_versions(old, &row.name);
        if let Some(versions) = old_versions.filter(|versions| !versions.is_empty()) {
            changes.extend(versions.iter().map(|version| VersionChangeRow {
                name: row.name.clone(),
                old_version: Some(version.clone()),
                eessi_version: eessi_version.clone(),
                status: status.to_string(),
            }));
        } else {
            changes.push(VersionChangeRow {
                name: row.name.clone(),
                old_version: None,
                eessi_version,
                status: status.to_string(),
            });
        }
    }
    changes
}

/// A tab-separated version change table with a header line.
pub fn render_version_changes(rows: &[VersionChangeRow]) -> String {
    let mut out = String::from("name\told_version\teessi_version\tstatus\n");
    for row in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            row.name,
            row.old_version.as_deref().unwrap_or("-"),
            row.eessi_version.as_deref().unwrap_or("-"),
            row.status
        ));
    }
    out
}

/// The package name an easyconfig filename names.
///
/// `hwloc-CUDA-2.11.2-GCCcore-15.2.0.eb` is `hwloc-CUDA`. A path contributes
/// only its last component.
pub fn software_name(file: &str) -> Option<String> {
    let base = basename(file);
    if let Some(parsed) = filename_toolchain(&base) {
        return Some(parsed.name);
    }
    package_name(&base).map(str::to_string)
}

fn remember(map: &mut BTreeMap<String, OpenCarry>, name: &str, file: &str, label: &str) {
    let key = name.to_lowercase();
    match map.get_mut(&key) {
        Some(hit) => {
            if !hit.label.split(", ").any(|part| part == label) {
                hit.label.push_str(", ");
                hit.label.push_str(label);
            }
        }
        None => {
            map.insert(
                key,
                OpenCarry {
                    file: basename(file),
                    label: label.to_string(),
                },
            );
        }
    }
}

/// Names the added easystack lines of `prs` carry.
///
/// The key is the lower-case package name. A pull request that only builds
/// the name as a dependency of some other line is not recorded: the route
/// is the line the pull request adds.
pub fn carries_from_layer(prs: &[PrInput]) -> BTreeMap<String, OpenCarry> {
    carries_from_layer_version(prs, None)
}

/// [`carries_from_layer`], keeping easystacks of `eessi_version` only.
///
/// `None` keeps every easystack, which is what [`carries_from_layer`] does.
/// `Some("2026.06")` leaves a 2025.06 pull request out of a 2026.06 gap.
/// The match is still the filename the pull request adds.
pub fn carries_from_layer_version(
    prs: &[PrInput],
    eessi_version: Option<&str>,
) -> BTreeMap<String, OpenCarry> {
    let mut out = BTreeMap::new();
    for pr in prs {
        let label = format!("software-layer#{}", pr.number);
        for change in &pr.easystacks {
            if let Some(version) = eessi_version {
                if pr_overlap::eessi_version_of(&change.path).as_deref() != Some(version) {
                    continue;
                }
            }
            for entry in added_entries(change) {
                if let Some(name) = software_name(&entry.file) {
                    remember(&mut out, &name, &entry.file, &label);
                }
            }
        }
    }
    out
}

fn walk_module_tree(dir: &Path, inventory: &mut ModuleInventory) -> std::io::Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|error| io_at(dir, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_at(dir, error))?;
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with('.') || file_name == "__pycache__" {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| io_at(&path, error))?;
        if file_type.is_symlink() {
            match std::fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => continue,
                Ok(_) => {}
                Err(_) => continue,
            }
        } else if file_type.is_dir() {
            walk_module_tree(&path, inventory)?;
            continue;
        }
        if !is_module_file(&file_name) {
            continue;
        }
        let Some(parent) = path.parent().and_then(|parent| parent.file_name()) else {
            continue;
        };
        let software = parent.to_string_lossy();
        if software.starts_with('.') {
            continue;
        }
        let stem = file_name.strip_suffix(".lua").unwrap_or(&file_name);
        if stem.is_empty() || stem == "default" {
            continue;
        }
        inventory
            .entry(software.to_string())
            .or_default()
            .insert(stem.to_string());
    }
    Ok(())
}

fn io_at(path: &Path, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn is_module_file(name: &str) -> bool {
    if name.starts_with('.') || name == "default" {
        return false;
    }
    if let Some(stem) = name.strip_suffix(".lua") {
        return !stem.is_empty() && stem != "default";
    }
    name.chars().any(|c| c.is_ascii_digit())
        && !name.ends_with(".txt")
        && !name.ends_with(".md")
        && !name.ends_with(".json")
        && !name.ends_with(".yml")
        && !name.ends_with(".eb")
}

fn live_module_tokens(line: &str) -> Vec<String> {
    let line = strip_ansi(line).replace("->", " ");
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.ends_with(':') {
        return Vec::new();
    }
    let body = if let Some((head, rest)) = line.split_once(':') {
        if rest.contains('/') {
            rest
        } else if live_module_token(head).is_some() {
            head
        } else {
            return Vec::new();
        }
    } else {
        line
    };
    body.split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(live_module_token)
        .collect()
}

fn live_module_token(raw: &str) -> Option<String> {
    let mut token = raw.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(index) = token.find('(') {
        token = token[..index].trim();
    }
    token = token.trim_matches(|c: char| matches!(c, '<' | '>' | '"' | '\'' | ','));
    token = token.trim_end_matches(':').trim().trim_matches('/');
    if token.is_empty() || token.starts_with('.') {
        return None;
    }
    let token = token.strip_suffix(".lua").unwrap_or(token);
    let parts: Vec<&str> = token.split('/').filter(|part| !part.is_empty()).collect();
    let (name, version) = match parts.as_slice() {
        [name] => (*name, None),
        [.., name, version] => (*name, Some(*version)),
        _ => return None,
    };
    if !is_software_name(name) {
        return None;
    }
    match version {
        None => Some(name.to_string()),
        Some(version) => {
            let version = version.strip_suffix(".lua").unwrap_or(version);
            if version.is_empty() || version.eq_ignore_ascii_case("default") {
                return None;
            }
            Some(format!("{name}/{version}"))
        }
    }
}

fn is_software_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '.'))
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn easyconfig_kept(change: &FileChange) -> bool {
    change.path.ends_with(".eb")
        && match (&change.base, &change.head) {
            (_, Some(_)) => true,
            (Some(_), None) => false,
            (None, None) => true,
        }
}

fn filename_on_generation(file: &str, generation: &str, members: &[String]) -> bool {
    let Some(parsed) = filename_toolchain(file) else {
        return false;
    };
    members.iter().any(|label| label == &parsed.toolchain)
        || parsed
            .toolchain
            .rsplit_once('-')
            .is_some_and(|(_, version)| version == generation)
}

/// Names the easyconfig files of `prs` add at `generation`.
///
/// A file whose toolchain version is `generation`, or whose toolchain label
/// is in `members`, counts. A file the pull request deletes does not. The
/// key is the lower-case package name.
pub fn carries_from_easyconfigs(
    prs: &[PrInput],
    generation: &str,
    members: &[String],
) -> BTreeMap<String, OpenCarry> {
    let mut out = BTreeMap::new();
    for pr in prs {
        let label = format!("easyconfigs#{}", pr.number);
        for change in &pr.easyconfigs {
            if !easyconfig_kept(change)
                || !filename_on_generation(&change.path, generation, members)
            {
                continue;
            }
            if let Some(name) = software_name(&change.path) {
                remember(&mut out, &name, &change.path, &label);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Toolchain;

    fn cand(name: &str, version: &str, tc: (&str, &str), suffix: Option<&str>) -> Candidate {
        let tcs = if tc.0 == "system" {
            String::new()
        } else {
            format!("-{}-{}", tc.0, tc.1)
        };
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: Toolchain {
                name: tc.0.into(),
                version: tc.1.into(),
            },
            versionsuffix: suffix.map(String::from),
            easyconfig_path: format!("/ec/{name}-{version}{tcs}{}.eb", suffix.unwrap_or("")),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            easyblock: None,
            moduleclass: None,
        }
    }

    fn members() -> Vec<String> {
        [
            "system",
            "GCCcore-15.2.0",
            "GCC-15.2.0",
            "gompi-2026.1",
            "foss-2026.1",
        ]
        .map(String::from)
        .to_vec()
    }

    fn classify(
        site: &BTreeSet<String>,
        stack: &BTreeSet<String>,
        candidates: &[Candidate],
        excluded: &BTreeMap<String, String>,
        filtered: &BTreeSet<String>,
        open_layer: &BTreeMap<String, OpenCarry>,
        open_easyconfigs: &BTreeMap<String, OpenCarry>,
    ) -> Vec<GapRow> {
        let members = members();
        gap(&GapInput {
            site,
            stack,
            candidates,
            generation: "2026.1",
            members: &members,
            excluded,
            filtered,
            open_layer,
            open_easyconfigs,
        })
    }

    #[test]
    fn routes_each_site_package() {
        let site = names_from_listing("BLAST+/2.16.0-gompi-2024a\nLAMMPS\nJulia/1.10.4\nCP2K/2025.2-foss-2025a\nMATLAB/2025b\nzlib/1.3.1\n");
        let stack = names_from_listing("FFTW/3.3.10-gompi-2026.1\n");
        let candidates = vec![
            cand("BLAST+", "2.17.0", ("gompi", "2026.1"), None),
            cand("BLAST+", "2.16.0", ("gompi", "2024a"), None),
            cand("Julia", "1.13.2", ("system", ""), None),
            cand("CP2K", "2025.2", ("foss", "2025b"), None),
            cand("CP2K", "2023.1", ("foss", "2023a"), None),
        ];
        let excluded = BTreeMap::from([("MATLAB".to_string(), "licensed".to_string())]);
        let filtered = BTreeSet::from(["zlib".to_string()]);
        let rows = classify(
            &site,
            &stack,
            &candidates,
            &excluded,
            &filtered,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        let by: BTreeMap<_, _> = rows.iter().map(|r| (r.name.as_str(), r)).collect();
        assert_eq!(by["BLAST+"].route, "recipe");
        assert_eq!(
            by["BLAST+"].recipe.as_deref(),
            Some("BLAST+-2.17.0-gompi-2026.1.eb")
        );
        assert_eq!(by["Julia"].recipe.as_deref(), Some("Julia-1.13.2.eb"));
        assert_eq!(by["CP2K"].route, "no-recipe");
        assert_eq!(
            by["CP2K"].newest_elsewhere.as_deref(),
            Some("CP2K-2025.2-foss-2025b.eb")
        );
        assert_eq!(by["LAMMPS"].route, "no-recipe");
        assert_eq!(by["LAMMPS"].newest_elsewhere, None);
        assert_eq!(by["MATLAB"].route, "excluded");
        assert_eq!(by["zlib"].route, "filtered");
    }

    #[test]
    fn the_plain_build_beats_a_suffixed_one_of_the_same_version() {
        let site = names_from_listing("NCCL\n");
        let candidates = vec![
            cand(
                "NCCL",
                "2.30.4",
                ("GCCcore", "15.2.0"),
                Some("-CUDA-13.3.0"),
            ),
            cand("NCCL", "2.30.4", ("GCCcore", "15.2.0"), None),
        ];
        let rows = classify(
            &site,
            &BTreeSet::new(),
            &candidates,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            rows[0].recipe.as_deref(),
            Some("NCCL-2.30.4-GCCcore-15.2.0.eb")
        );
    }

    #[test]
    fn open_pull_requests_outrank_a_develop_recipe_and_a_bump() {
        let site = names_from_listing("LAMMPS\nSciPy-bundle\nQuantumESPRESSO\nOldTool\n");
        let candidates = vec![
            cand("SciPy-bundle", "2026.05", ("gfbf", "2026.1"), None),
            cand("QuantumESPRESSO", "7.3", ("foss", "2025b"), None),
        ];
        let members = members();
        let layer = carries_from_layer(&[pr_layer(
            1787,
            "  - LAMMPS-2025.0-foss-2026.1.eb\n  - SciPy-bundle-2026.05-gfbf-2026.1.eb\n",
        )]);
        let easy = carries_from_easyconfigs(
            &[pr_easy(
                25001,
                &[
                    "q/QuantumESPRESSO/QuantumESPRESSO-7.4-foss-2026.1.eb",
                    "o/OldTool/OldTool-1.0-foss-2025b.eb",
                ],
            )],
            "2026.1",
            &members,
        );
        let rows = classify(
            &site,
            &BTreeSet::new(),
            &candidates,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &layer,
            &easy,
        );
        let by: BTreeMap<_, _> = rows.iter().map(|r| (r.name.as_str(), r)).collect();
        assert_eq!(by["LAMMPS"].route, "open-layer");
        assert_eq!(by["LAMMPS"].reason.as_deref(), Some("software-layer#1787"));
        assert_eq!(by["SciPy-bundle"].route, "open-layer");
        assert_eq!(by["QuantumESPRESSO"].route, "open-easyconfigs");
        assert_eq!(
            by["QuantumESPRESSO"].recipe.as_deref(),
            Some("QuantumESPRESSO-7.4-foss-2026.1.eb")
        );
        assert_eq!(by["OldTool"].route, "no-recipe");
    }

    #[test]
    fn an_lfoss_recipe_is_on_the_2026_1_generation() {
        let site = names_from_listing("OpenMPI\n");
        let candidates = vec![cand("OpenMPI", "5.0.10", ("lfoss", "2026.1"), None)];
        let rows = classify(
            &site,
            &BTreeSet::new(),
            &candidates,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(rows[0].route, "recipe");
        assert_eq!(
            rows[0].recipe.as_deref(),
            Some("OpenMPI-5.0.10-lfoss-2026.1.eb")
        );
    }

    #[test]
    fn site_only_wins_over_a_filtered_name() {
        let site = names_from_listing("zlib\n");
        let excluded =
            BTreeMap::from([("zlib".to_string(), "kept on the old pipeline".to_string())]);
        let filtered = BTreeSet::from(["zlib".to_string()]);
        let rows = classify(
            &site,
            &BTreeSet::new(),
            &[],
            &excluded,
            &filtered,
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(rows[0].route, "excluded");
        assert_eq!(rows[0].reason.as_deref(), Some("kept on the old pipeline"));
    }

    fn pr_layer(number: u64, added: &str) -> PrInput {
        PrInput {
            number,
            head_sha: format!("{number:040}"),
            title: None,
            easystacks: vec![FileChange {
                path: "easystacks/software.eessi.io/2026.06/eessi-2026.06-eb-5.4.2-2026.1.yml"
                    .into(),
                base: Some("easyconfigs:\n".into()),
                head: Some(format!("easyconfigs:\n{added}")),
                base_blob: None,
                head_blob: None,
            }],
            files: Vec::new(),
            easyconfigs: Vec::new(),
            file_list_truncated: false,
        }
    }

    fn pr_easy(number: u64, paths: &[&str]) -> PrInput {
        PrInput {
            number,
            head_sha: format!("{number:040}"),
            title: None,
            easystacks: Vec::new(),
            files: Vec::new(),
            easyconfigs: paths
                .iter()
                .map(|path| FileChange {
                    path: (*path).to_string(),
                    base: None,
                    head: Some("name = 'x'\n".into()),
                    base_blob: None,
                    head_blob: None,
                })
                .collect(),
            file_list_truncated: false,
        }
    }

    #[test]
    fn a_plain_listing_and_a_live_listing_name_the_same_packages() {
        let text = "\
BLAST+/2.16.0-gompi-2024a
LAMMPS
Julia/1.10.4
# comment
";
        assert_eq!(names_from_listing(text), names_from_live_listing(text));
    }

    #[test]
    fn avail_and_spider_text_drops_headers() {
        let text = "\
\u{1b}[1m/opt/modules/all:\u{1b}[0m
zlib/1.3.1(default)
GCCcore/14.2.0 (D)
LAMMPS
zlib: zlib/1.2.13, zlib/1.3.1
Where:
--------
";
        let names = names_from_live_listing(text);
        assert_eq!(
            names,
            BTreeSet::from([
                "zlib".to_string(),
                "GCCcore".to_string(),
                "LAMMPS".to_string(),
            ])
        );
        let inventory = inventory_from_live_listing(text);
        assert_eq!(
            inventory["zlib"],
            BTreeSet::from(["1.2.13".to_string(), "1.3.1".to_string()])
        );
        assert_eq!(inventory["GCCcore"], BTreeSet::from(["14.2.0".to_string()]));
        assert!(inventory["LAMMPS"].is_empty());
    }

    #[test]
    fn version_changes_name_moved_gone_and_site_only_rows() {
        let old = inventory_from_live_listing(
            "FFTW/3.3.10-gompi-2025b\nLAMMPS/2024.0-foss-2025b\nMATLAB/2024b\n",
        );
        let eessi = inventory_from_live_listing("FFTW/3.3.10-gompi-2026.1\n");
        let rows = vec![
            row("FFTW", "in-stack", None, None, None),
            row("LAMMPS", "open-layer", None, None, None),
            row("MATLAB", "excluded", None, None, None),
        ];
        assert_eq!(
            render_version_changes(&version_changes(&rows, &old, &eessi)),
            "\
name\told_version\teessi_version\tstatus
FFTW\t3.3.10-gompi-2025b\t3.3.10-gompi-2026.1\tmoved
LAMMPS\t2024.0-foss-2025b\t-\tgone
MATLAB\t2024b\t-\tsite-only
"
        );
    }

    #[test]
    fn a_module_tree_walk_skips_modulerc_and_reads_the_hierarchy() {
        let root = tempfile::tempdir().unwrap();
        let all = root.path().join("all");
        std::fs::create_dir_all(all.join("zlib")).unwrap();
        std::fs::create_dir_all(all.join("LAMMPS")).unwrap();
        std::fs::write(all.join("zlib/1.3.1.lua"), "").unwrap();
        std::fs::write(all.join("zlib/.modulerc.lua"), "").unwrap();
        std::fs::write(all.join(".modulerc.lua"), "").unwrap();
        std::fs::write(all.join("LAMMPS/default"), "").unwrap();
        std::fs::write(all.join("LAMMPS/2024.0.lua"), "").unwrap();
        std::fs::write(all.join("README"), "not a module").unwrap();
        let binutils = root.path().join("Compiler/GCCcore/13.2.0/binutils");
        std::fs::create_dir_all(&binutils).unwrap();
        std::fs::write(binutils.join("2.42.lua"), "").unwrap();
        let names: BTreeSet<String> = inventory_from_module_tree(root.path())
            .unwrap()
            .into_keys()
            .collect();
        assert_eq!(
            names,
            BTreeSet::from([
                "zlib".to_string(),
                "LAMMPS".to_string(),
                "binutils".to_string(),
            ])
        );
    }

    #[test]
    fn an_older_eessi_pull_request_is_not_the_target_version() {
        let current = pr_layer(1700, "  - LAMMPS-2025.0-foss-2026.1.eb\n");
        let mut older = pr_layer(1600, "  - QuantumESPRESSO-7.3-foss-2025a.eb\n");
        older.easystacks[0].path =
            "easystacks/software.eessi.io/2025.06/eessi-2025.06-eb-5.1.1-2025a.yml".into();
        let kept = carries_from_layer_version(&[current, older], Some("2026.06"));
        assert!(kept.contains_key("lammps"));
        assert!(!kept.contains_key("quantumespresso"));
        let all = carries_from_layer_version(
            &[pr_layer(1600, "  - QuantumESPRESSO-7.3-foss-2025a.eb\n")],
            None,
        );
        assert!(all.contains_key("quantumespresso"));
    }
}
