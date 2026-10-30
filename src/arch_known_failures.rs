//! Known per-architecture failures of entries that do build.
//!
//! An entry in `data/arch-unsupported.toml` cannot be built on an
//! architecture at all. The entries in `data/arch-known-failures.toml` can,
//! but a bot run on one target has already failed them: a fetch, a build
//! step, a test, a test timeout, or a sanity check. Each one names the
//! remedy the maintainers settled on (a `runtest` skip, a `preinstallopts`
//! line, an `ARCH` conditional, a pin) and the EESSI hook that applies it,
//! when there is one. A plan that includes such an entry, or depends on
//! one, gets a warning and that remedy before the bot runs.
//!
//! EESSI hooks that rewrite an easyconfig are in the same table. The CUDA
//! demotion in `inject_gpu_property` is one: an entry with
//! `requires_dependency = "CUDA"` applies only when the easyconfig depends
//! on CUDA, because only then does the hook move it to a build dependency.

use crate::arch_support::{basename, index_by_file, plan_requirers, split_easyconfig_filename};
use crate::build_order::{closure_against_installed, reverse_uses, Choice, ModuleKey};
use crate::cli_parse::usage;
use crate::domain::Candidate;
use crate::easystack::{parse_easystack_entries, EasystackEntry};
use crate::eb_parse::parse_easyconfig_trees;
use crate::pr_overlap::parse_installed;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The table shipped with eb-stack.
pub const BUNDLED_KNOWN_FAILURES: &str = include_str!("../data/arch-known-failures.toml");

/// Where in the bot run the entry fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// The source or wheel cannot be downloaded.
    Fetch,
    /// Configure, build, or install fails.
    Build,
    /// A test fails.
    Test,
    /// A test runs out of time.
    Timeout,
    /// The sanity check fails after the install.
    Sanity,
}

impl Phase {
    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "fetch" => Some(Self::Fetch),
            "build" => Some(Self::Build),
            "test" => Some(Self::Test),
            "timeout" => Some(Self::Timeout),
            "sanity" => Some(Self::Sanity),
            _ => None,
        }
    }

    /// The words the warning uses.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fetch => "fetch failure",
            Self::Build => "build failure",
            Self::Test => "test failure",
            Self::Timeout => "test timeout",
            Self::Sanity => "sanity check failure",
        }
    }
}

/// One known failure and its remedy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownFailure {
    /// Short unique handle, `orjson-a64fx-zoneinfo-utc`.
    pub id: String,
    /// EasyBuild names. An `exts_list` extension of that name matches too.
    pub names: Vec<String>,
    /// Versions seen failing. Empty is every version.
    pub versions: Vec<String>,
    /// Targets, families, or `*` for every architecture.
    pub arches: Vec<String>,
    /// Where the run fails.
    pub phase: Phase,
    /// What the bot log shows.
    pub symptom: String,
    /// The easyconfig remedy, as text.
    pub remedy: String,
    /// EESSI hook that applies the remedy, if one does.
    pub hook: Option<String>,
    /// EESSI hook whose rewrite of the easyconfig causes the failure.
    pub caused_by_hook: Option<String>,
    /// Only when the easyconfig depends on this name.
    pub requires_dependency: Option<String>,
    /// Only from this EasyBuild release.
    pub min_easybuild: Option<String>,
    /// Only for these EESSI versions. Empty is every version.
    pub eessi_versions: Vec<String>,
    /// Commits whose pin carries the remedy.
    pub fixed_by: Vec<String>,
    /// Text the easyconfig contains once the remedy is in it.
    pub fixed_marker: Option<String>,
    /// Pull requests and comments the entry was seeded from.
    pub sources: Vec<String>,
}

/// The known-failure table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownFailureList {
    entries: Vec<KnownFailure>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    entry: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    id: String,
    names: Vec<String>,
    #[serde(default)]
    versions: Vec<String>,
    arches: Vec<String>,
    phase: String,
    symptom: String,
    remedy: String,
    #[serde(default)]
    hook: Option<String>,
    #[serde(default)]
    caused_by_hook: Option<String>,
    #[serde(default)]
    requires_dependency: Option<String>,
    #[serde(default)]
    min_easybuild: Option<String>,
    #[serde(default)]
    eessi_versions: Vec<String>,
    #[serde(default)]
    fixed_by: Vec<String>,
    #[serde(default)]
    fixed_marker: Option<String>,
    #[serde(default)]
    sources: Vec<String>,
}

fn trimmed(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn present(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

impl KnownFailureList {
    /// The table shipped in `data/arch-known-failures.toml`.
    pub fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_KNOWN_FAILURES).context("data/arch-known-failures.toml")
    }

    /// Read a table from `path`. An empty file is an empty table.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Parse the TOML document.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawFile = toml::from_str(text).context("known-failure table is not valid TOML")?;
        let mut entries = Vec::new();
        let mut ids = BTreeSet::new();
        for entry in raw.entry {
            let id = entry.id.trim().to_string();
            let names = trimmed(entry.names);
            let arches = trimmed(entry.arches);
            let symptom = entry.symptom.trim().to_string();
            let remedy = entry.remedy.trim().to_string();
            if id.is_empty()
                || names.is_empty()
                || arches.is_empty()
                || symptom.is_empty()
                || remedy.is_empty()
            {
                bail!("a known failure needs an id, names, arches, a symptom, and a remedy");
            }
            if !ids.insert(id.clone()) {
                bail!("known-failure table repeats id {id}");
            }
            let Some(phase) = Phase::parse(&entry.phase) else {
                bail!(
                    "known failure {id}: phase {} is not fetch, build, test, timeout, or sanity",
                    entry.phase
                );
            };
            let min_easybuild = present(entry.min_easybuild);
            if let Some(min) = &min_easybuild {
                if release_parts(min).is_none() {
                    bail!("known failure {id}: min_easybuild {min} is not X.Y.Z");
                }
            }
            let fixed_by = trimmed(entry.fixed_by);
            for commit in &fixed_by {
                if commit.len() < 7 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                    bail!("known failure {id}: fixed_by {commit} is not a commit hash");
                }
            }
            entries.push(KnownFailure {
                id,
                names,
                versions: trimmed(entry.versions),
                arches,
                phase,
                symptom,
                remedy,
                hook: present(entry.hook),
                caused_by_hook: present(entry.caused_by_hook),
                requires_dependency: present(entry.requires_dependency),
                min_easybuild,
                eessi_versions: trimmed(entry.eessi_versions),
                fixed_by,
                fixed_marker: present(entry.fixed_marker),
                sources: entry.sources,
            });
        }
        Ok(Self { entries })
    }

    /// Every entry, in file order.
    pub fn entries(&self) -> &[KnownFailure] {
        &self.entries
    }

    /// The entry with this id.
    pub fn get(&self, id: &str) -> Option<&KnownFailure> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

/// The EESSI and EasyBuild versions a plan is built for.
///
/// `None` means the plan does not say, and every entry applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanContext {
    /// EESSI version, `2025.06`.
    pub eessi_version: Option<String>,
    /// EasyBuild release, `5.4.0`.
    pub easybuild_version: Option<String>,
}

impl PlanContext {
    /// Versions from an EESSI easystack file name such as
    /// `eessi-2026.06-eb-5.4.0-2026.1.yml`, unless an override is given.
    pub fn from_easystack_name(
        name: &str,
        eessi_version: Option<&str>,
        easybuild_version: Option<&str>,
    ) -> Self {
        Self {
            eessi_version: eessi_version
                .map(str::to_string)
                .or_else(|| eessi_version_in_file_name(name)),
            easybuild_version: easybuild_version
                .map(str::to_string)
                .or_else(|| crate::from_commit::version_in_file_name(name)),
        }
    }

    fn admits(&self, entry: &KnownFailure) -> bool {
        if let (Some(version), false) = (&self.eessi_version, entry.eessi_versions.is_empty()) {
            if !entry.eessi_versions.iter().any(|v| v == version) {
                return false;
            }
        }
        if let (Some(version), Some(min)) = (&self.easybuild_version, &entry.min_easybuild) {
            if let (Some(have), Some(need)) = (release_parts(version), release_parts(min)) {
                if have < need {
                    return false;
                }
            }
        }
        true
    }
}

/// `eessi-YYYY.MM` in a file name.
fn eessi_version_in_file_name(name: &str) -> Option<String> {
    let mut rest = name;
    while let Some(index) = rest.find("eessi-") {
        let boundary = index == 0 || !rest.as_bytes()[index - 1].is_ascii_alphanumeric();
        let after = &rest[index + "eessi-".len()..];
        if boundary {
            let version: String = after
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            let version = version.trim_end_matches('.');
            let parts: Vec<&str> = version.split('.').collect();
            if parts.len() == 2 && parts.iter().all(|p| !p.is_empty()) {
                return Some(version.to_string());
            }
        }
        rest = after;
    }
    None
}

fn release_parts(text: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = text.trim().split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|parts| !parts.is_empty())
}

/// The architecture a hit is reported for, when `entry_arch` and `query`
/// meet. A family meets each of its targets, and the more specific one is
/// reported. `*` meets everything.
fn meet(entry_arch: &str, query: &str) -> Option<String> {
    if entry_arch == "*" {
        return Some(query.to_string());
    }
    if query == "*" {
        return Some(entry_arch.to_string());
    }
    let entry: Vec<&str> = entry_arch.split('/').collect();
    let asked: Vec<&str> = query.split('/').collect();
    if entry.starts_with(&asked) {
        Some(entry_arch.to_string())
    } else if asked.starts_with(&entry) {
        Some(query.to_string())
    } else {
        None
    }
}

fn arch_text(arch: &str) -> &str {
    if arch == "*" {
        "every architecture"
    } else {
        arch
    }
}

fn commits_match(pin: &str, fixed: &str) -> bool {
    let pin = pin.trim().to_ascii_lowercase();
    let fixed = fixed.trim().to_ascii_lowercase();
    pin.len() >= 7 && fixed.len() >= 7 && (pin.starts_with(&fixed) || fixed.starts_with(&pin))
}

/// One known failure a plan includes or depends on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnownHit {
    /// The table entry.
    pub id: String,
    /// Easyconfig filename that fails.
    pub file: String,
    /// `name version` of the extension that fails, when the match is an
    /// `exts_list` entry rather than the easyconfig itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// Architectures it fails on, as reported.
    pub arches: Vec<String>,
    /// Where the run fails.
    pub phase: Phase,
    /// What the bot log shows.
    pub symptom: String,
    /// The easyconfig remedy.
    pub remedy: String,
    /// EESSI hook that applies it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    /// EESSI hook whose rewrite causes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caused_by_hook: Option<String>,
    /// Evidence.
    pub sources: Vec<String>,
    /// Plan entries that depend on `file`. Empty when the plan names it.
    pub required_by: BTreeSet<String>,
    /// Why the remedy is already in place. `None` is a warning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedied: Option<String>,
}

impl KnownHit {
    /// The lines the command prints for this hit, without a final newline.
    pub fn message(&self) -> String {
        let mut subject = self.file.clone();
        if let Some(extension) = &self.extension {
            subject.push_str(&format!(" (extension {extension})"));
        }
        let arches = self
            .arches
            .iter()
            .map(|arch| arch_text(arch))
            .collect::<Vec<_>>()
            .join(", ");
        if let Some(remedied) = &self.remedied {
            return format!(
                "{subject} on {arches}: known {}, remedied: {remedied}",
                self.phase.label()
            );
        }
        let mut line = format!(
            "{subject} on {arches}: known {}: {}",
            self.phase.label(),
            self.symptom
        );
        if !self.required_by.is_empty() {
            line.push_str(" (required by ");
            line.push_str(
                &self
                    .required_by
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            line.push(')');
        }
        if let Some(hook) = &self.caused_by_hook {
            line.push_str(&format!("\n  caused by: the EESSI {hook} hook"));
        }
        line.push_str("\n  remedy: ");
        line.push_str(&self.remedy);
        if let Some(hook) = &self.hook {
            line.push_str(&format!("\n  in EESSI: the {hook} hook applies it"));
        }
        let cites = citations(&self.sources);
        if !cites.is_empty() {
            line.push_str("\n  seen in: ");
            line.push_str(&cites);
        }
        line
    }
}

/// `software-layer #1871, #1829; software-layer-scripts #330`, from the
/// GitHub pull request and commit links in `sources`.
pub fn citations(sources: &[String]) -> String {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for source in sources {
        let Some(rest) = source.strip_prefix("https://github.com/") else {
            continue;
        };
        let parts: Vec<&str> = rest.split(['/', '#']).collect();
        if parts.len() < 4 {
            continue;
        }
        let repo = parts[1].to_string();
        let item = match parts[2] {
            "pull" | "issues" if parts[3].chars().all(|c| c.is_ascii_digit()) => {
                format!("#{}", parts[3])
            }
            "commit" if parts[3].len() >= 7 => parts[3][..7].to_string(),
            _ => continue,
        };
        match groups.iter_mut().find(|(name, _)| name == &repo) {
            Some((_, items)) => {
                if !items.contains(&item) {
                    items.push(item);
                }
            }
            None => groups.push((repo, vec![item])),
        }
    }
    groups
        .into_iter()
        .map(|(repo, items)| format!("{repo} {}", items.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

struct Subject<'a> {
    file: String,
    extension: Option<String>,
    candidate: Option<&'a Candidate>,
}

fn subjects<'a>(
    entry: &KnownFailure,
    file: &str,
    candidate: Option<&'a Candidate>,
) -> Vec<Subject<'a>> {
    let version_ok =
        |version: &str| entry.versions.is_empty() || entry.versions.iter().any(|v| v == version);
    let (name, version) = match candidate {
        Some(candidate) => (candidate.name.clone(), candidate.version.clone()),
        None => match split_easyconfig_filename(file) {
            Some((name, versionfull)) => {
                let version = versionfull.split('-').next().unwrap_or("").to_string();
                (name, version)
            }
            None => return Vec::new(),
        },
    };
    if entry.names.iter().any(|n| n == &name) {
        if version_ok(&version) {
            return vec![Subject {
                file: file.to_string(),
                extension: None,
                candidate,
            }];
        }
        return Vec::new();
    }
    let Some(candidate) = candidate else {
        return Vec::new();
    };
    candidate
        .exts_list
        .iter()
        .filter(|ext| entry.names.iter().any(|n| n == &ext.name) && version_ok(&ext.version))
        .map(|ext| Subject {
            file: file.to_string(),
            extension: Some(if ext.version.is_empty() {
                ext.name.clone()
            } else {
                format!("{} {}", ext.name, ext.version)
            }),
            candidate: Some(candidate),
        })
        .collect()
}

fn depends_on(subject: &Subject<'_>, dependency: &str) -> bool {
    match subject.candidate {
        Some(candidate) => candidate
            .dependencies
            .iter()
            .chain(candidate.builddependencies.iter())
            .any(|dep| dep.name == dependency),
        None => subject.file.contains(&format!("-{dependency}-")),
    }
}

fn remedied_by(
    entry: &KnownFailure,
    subject: &Subject<'_>,
    pins: &BTreeMap<String, Vec<(&'static str, String)>>,
    by_file_all: &BTreeMap<String, Vec<&Candidate>>,
) -> Option<String> {
    if let Some(pins) = pins.get(&subject.file) {
        for (option, commit) in pins {
            if entry
                .fixed_by
                .iter()
                .any(|fixed| commits_match(commit, fixed))
            {
                let short: String = commit.chars().take(7).collect();
                return Some(format!("{option} {short} carries the remedy"));
            }
        }
    }
    let marker = entry.fixed_marker.as_deref()?;
    let mut paths: Vec<&str> = Vec::new();
    if let Some(candidates) = by_file_all.get(&subject.file) {
        paths.extend(
            candidates
                .iter()
                .map(|candidate| candidate.easyconfig_path.as_str()),
        );
    } else if let Some(candidate) = subject.candidate {
        paths.push(candidate.easyconfig_path.as_str());
    }
    if !paths.into_iter().any(|path| {
        std::fs::read_to_string(path)
            .map(|text| text.contains(marker))
            .unwrap_or(false)
    }) {
        return None;
    }
    let shown = if marker.len() > 16 {
        format!("{}...", &marker[..8])
    } else {
        marker.to_string()
    };
    Some(format!("the easyconfig already has {shown}"))
}

fn plan_pins(plan: &[EasystackEntry]) -> BTreeMap<String, Vec<(&'static str, String)>> {
    let mut pins: BTreeMap<String, Vec<(&'static str, String)>> = BTreeMap::new();
    for entry in plan {
        let file = basename(&entry.file);
        if let Some(commit) = &entry.from_commit {
            pins.entry(file.clone())
                .or_default()
                .push(("from-commit", commit.clone()));
        }
        if let Some(commit) = &entry.include_easyblocks_from_commit {
            pins.entry(file)
                .or_default()
                .push(("include-easyblocks-from-commit", commit.clone()));
        }
    }
    pins
}

type HitKey = (String, String, Option<String>);

#[allow(clippy::too_many_arguments)]
fn record(
    hits: &mut BTreeMap<HitKey, KnownHit>,
    entry: &KnownFailure,
    queries: &[String],
    subject: &Subject<'_>,
    pins: &BTreeMap<String, Vec<(&'static str, String)>>,
    by_file_all: &BTreeMap<String, Vec<&Candidate>>,
    required_by: BTreeSet<String>,
) {
    if let Some(dependency) = &entry.requires_dependency {
        if !depends_on(subject, dependency) {
            return;
        }
    }
    let mut arches: Vec<String> = Vec::new();
    for arch in &entry.arches {
        for query in queries {
            if let Some(hit) = meet(arch, query) {
                if !arches.contains(&hit) {
                    arches.push(hit);
                }
            }
        }
    }
    if arches.is_empty() {
        return;
    }
    let remedied = remedied_by(entry, subject, pins, by_file_all);
    let key = (
        subject.file.clone(),
        entry.id.clone(),
        subject.extension.clone(),
    );
    let hit = KnownHit {
        id: entry.id.clone(),
        file: subject.file.clone(),
        extension: subject.extension.clone(),
        arches,
        phase: entry.phase,
        symptom: entry.symptom.clone(),
        remedy: entry.remedy.clone(),
        hook: entry.hook.clone(),
        caused_by_hook: entry.caused_by_hook.clone(),
        sources: entry.sources.clone(),
        required_by,
        remedied,
    };
    merge_hit(hits, key, hit);
}

fn merge_hit(hits: &mut BTreeMap<HitKey, KnownHit>, key: HitKey, hit: KnownHit) {
    match hits.get_mut(&key) {
        Some(existing) => {
            for arch in hit.arches {
                if !existing.arches.contains(&arch) {
                    existing.arches.push(arch);
                }
            }
            existing.required_by.extend(hit.required_by);
            if hit.remedied.is_none() {
                existing.remedied = None;
            }
        }
        None => {
            hits.insert(key, hit);
        }
    }
}

/// Known failures that `plan` includes or depends on, for `arches`.
///
/// `arches` empty checks every architecture in the table. A module
/// `is_installed` reports is not rebuilt, so it is not a hit, and the
/// closure stops there.
pub fn known_failures_in_plan(
    list: &KnownFailureList,
    arches: &[String],
    candidates: &[Candidate],
    plan: &[EasystackEntry],
    context: &PlanContext,
    is_installed: &dyn Fn(&ModuleKey) -> bool,
) -> Vec<KnownHit> {
    let mut hits = BTreeMap::new();
    collect_hits(
        &mut hits,
        list,
        arches,
        candidates,
        plan,
        context,
        is_installed,
    );
    hits.into_values().collect()
}

fn collect_hits(
    hits: &mut BTreeMap<HitKey, KnownHit>,
    list: &KnownFailureList,
    arches: &[String],
    candidates: &[Candidate],
    plan: &[EasystackEntry],
    context: &PlanContext,
    is_installed: &dyn Fn(&ModuleKey) -> bool,
) {
    let queries: Vec<String> = if arches.is_empty() {
        vec!["*".to_string()]
    } else {
        arches.iter().map(|arch| arch.trim().to_string()).collect()
    };
    let entries: Vec<&KnownFailure> = list
        .entries()
        .iter()
        .filter(|entry| context.admits(entry))
        .collect();
    if entries.is_empty() {
        return;
    }
    let pins = plan_pins(plan);
    let by_file = index_by_file(candidates);
    let mut by_file_all: BTreeMap<String, Vec<&Candidate>> = BTreeMap::new();
    for candidate in candidates {
        by_file_all
            .entry(basename(&candidate.easyconfig_path))
            .or_default()
            .push(candidate);
    }
    let mut start_keys: Vec<ModuleKey> = Vec::new();
    let mut start_files: BTreeSet<String> = BTreeSet::new();
    for item in plan {
        let file = basename(&item.file);
        if !start_files.insert(file.clone()) {
            continue;
        }
        match by_file.get(&file) {
            Some(candidate) => start_keys.push(ModuleKey::of(candidate)),
            None => {
                for entry in &entries {
                    for subject in subjects(entry, &file, None) {
                        record(
                            hits,
                            entry,
                            &queries,
                            &subject,
                            &pins,
                            &by_file_all,
                            BTreeSet::new(),
                        );
                    }
                }
            }
        }
    }
    if start_keys.is_empty() {
        return;
    }

    let closure = closure_against_installed(candidates, &start_keys, Choice::Newest, is_installed);
    let in_closure: BTreeSet<ModuleKey> = closure.to_build.iter().cloned().collect();
    let start_set: BTreeSet<ModuleKey> = start_keys.iter().cloned().collect();
    let uses = reverse_uses(candidates, Choice::Newest);
    let by_key: BTreeMap<ModuleKey, &Candidate> =
        candidates.iter().map(|c| (ModuleKey::of(c), c)).collect();

    for key in &closure.to_build {
        let Some(candidate) = by_key.get(key) else {
            continue;
        };
        let file = basename(&candidate.easyconfig_path);
        let included = start_set.contains(key) || start_files.contains(&file);
        let mut required_by: Option<BTreeSet<String>> = None;
        for entry in &entries {
            for subject in subjects(entry, &file, Some(candidate)) {
                let requirers = if included {
                    BTreeSet::new()
                } else {
                    required_by
                        .get_or_insert_with(|| {
                            plan_requirers(key, &start_set, &in_closure, &uses)
                                .iter()
                                .filter_map(|requirer| by_key.get(requirer))
                                .map(|candidate| basename(&candidate.easyconfig_path))
                                .collect()
                        })
                        .clone()
                };
                record(
                    hits,
                    entry,
                    &queries,
                    &subject,
                    &pins,
                    &by_file_all,
                    requirers,
                );
            }
        }
    }
}

/// Arguments for `stack known-failures`.
pub struct KnownFailuresRequest {
    /// Architectures to check. Empty checks every one in the table.
    pub arches: Vec<String>,
    /// Easystacks whose entries are the plan. Repeatable.
    pub easystacks: Vec<PathBuf>,
    /// Robot trees the dependency closure is read from. Empty checks only
    /// the filenames the easystacks name.
    pub easyconfigs: Vec<PathBuf>,
    /// Installed modules, one `Name/VersionFull` per line. An installed
    /// module is not rebuilt, so it is not warned about.
    pub installed: Option<PathBuf>,
    /// Override `data/arch-known-failures.toml`.
    pub known_failures: Option<PathBuf>,
    /// Override the EESSI version in each easystack file name.
    pub eessi_version: Option<String>,
    /// Override the EasyBuild release in each easystack file name.
    pub easybuild_version: Option<String>,
}

/// What `stack known-failures` prints.
pub struct KnownFailuresReport {
    /// Stdout, including the count line.
    pub stdout: String,
    /// Every hit, warnings and remedied ones.
    pub hits: Vec<KnownHit>,
}

impl KnownFailuresReport {
    /// Hits whose remedy is not in place yet.
    pub fn warnings(&self) -> impl Iterator<Item = &KnownHit> {
        self.hits.iter().filter(|hit| hit.remedied.is_none())
    }
}

/// Warn about known per-architecture failures a plan includes or depends on.
pub fn run_known_failures(request: &KnownFailuresRequest) -> Result<KnownFailuresReport> {
    if request.easystacks.is_empty() {
        return Err(usage("pass at least one --easystack"));
    }
    let list = match &request.known_failures {
        Some(path) => KnownFailureList::load(path)?,
        None => KnownFailureList::bundled()?,
    };
    let installed = match &request.installed {
        Some(path) => parse_installed(
            &std::fs::read_to_string(path)
                .with_context(|| format!("--installed {}", path.display()))?,
        ),
        None => BTreeSet::new(),
    };
    let candidates;
    let candidate_slice: &[Candidate] = if request.easyconfigs.is_empty() {
        &[]
    } else {
        let trees: Vec<&Path> = request.easyconfigs.iter().map(PathBuf::as_path).collect();
        candidates = parse_easyconfig_trees(&trees).map_err(|error| {
            anyhow::Error::msg(error).context(format!(
                "--easyconfigs {}",
                request
                    .easyconfigs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        &candidates.candidates
    };
    let is_installed = |key: &ModuleKey| installed.contains(&crate::build_order::module_name(key));

    let mut hits = BTreeMap::new();
    for path in &request.easystacks {
        if path.is_dir() {
            return Err(usage(format!(
                "--easystack {} is a directory; pass the easystack file",
                path.display()
            )));
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--easystack {}", path.display()))?;
        let entries = parse_easystack_entries(&text)
            .with_context(|| format!("--easystack {}", path.display()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let context = PlanContext::from_easystack_name(
            name,
            request.eessi_version.as_deref(),
            request.easybuild_version.as_deref(),
        );
        collect_hits(
            &mut hits,
            &list,
            &request.arches,
            candidate_slice,
            &entries,
            &context,
            &is_installed,
        );
    }
    let hits: Vec<KnownHit> = hits.into_values().collect();
    let mut stdout = String::new();
    for hit in &hits {
        stdout.push_str(&hit.message());
        stdout.push('\n');
    }
    let remedied = hits.iter().filter(|hit| hit.remedied.is_some()).count();
    stdout.push_str(&format!(
        "known={} remedied={}\n",
        hits.len() - remedied,
        remedied
    ));
    Ok(KnownFailuresReport { stdout, hits })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_table_names_the_bot_failures_with_a_remedy() {
        let list = KnownFailureList::bundled().expect("bundled table");
        let orjson = list.get("orjson-a64fx-zoneinfo-utc").expect("orjson");
        assert_eq!(orjson.arches, ["aarch64/a64fx"]);
        assert!(orjson.remedy.contains("not test_datetime_zoneinfo_utc"));
        assert!(orjson.sources.iter().any(|s| s.contains("/pull/1871")));
        assert!(orjson.sources.iter().any(|s| s.contains("/pull/1829")));
        let kaleido = list.get("kaleido-aarch64-wheel").expect("Kaleido");
        assert_eq!(kaleido.phase, Phase::Fetch);
        assert!(kaleido.remedy.contains("ARCH == 'aarch64'"));
        let walberla = list.get("walberla-sve-constexpr").expect("waLBerla");
        assert!(walberla.arches.iter().all(|a| a.starts_with("aarch64/")));
        let gromacs = list.get("gromacs-cuda-demoted").expect("GROMACS");
        assert_eq!(gromacs.requires_dependency.as_deref(), Some("CUDA"));
        assert_eq!(
            gromacs.caused_by_hook.as_deref(),
            Some("inject_gpu_property")
        );
        assert_eq!(gromacs.hook, None);
        for entry in list.entries() {
            assert!(
                entry
                    .sources
                    .iter()
                    .any(|s| s.starts_with("https://github.com/EESSI/software-layer/pull/")),
                "{} cites no software-layer pull request",
                entry.id
            );
        }
    }

    #[test]
    fn a_family_meets_its_targets_and_star_meets_everything() {
        assert_eq!(
            meet("aarch64/a64fx", "aarch64").as_deref(),
            Some("aarch64/a64fx")
        );
        assert_eq!(
            meet("aarch64", "aarch64/nvidia/grace").as_deref(),
            Some("aarch64/nvidia/grace")
        );
        assert_eq!(meet("aarch64/a64fx", "aarch64/neoverse_v1"), None);
        assert_eq!(meet("aarch64", "x86_64/amd/zen2"), None);
        assert_eq!(
            meet("*", "riscv64/generic").as_deref(),
            Some("riscv64/generic")
        );
        assert_eq!(
            meet("x86_64/intel/haswell", "*").as_deref(),
            Some("x86_64/intel/haswell")
        );
    }

    #[test]
    fn the_easystack_name_gives_the_eessi_and_easybuild_versions() {
        let context =
            PlanContext::from_easystack_name("eessi-2026.06-eb-5.4.0-2026.1.yml", None, None);
        assert_eq!(context.eessi_version.as_deref(), Some("2026.06"));
        assert_eq!(context.easybuild_version.as_deref(), Some("5.4.0"));
        let overridden =
            PlanContext::from_easystack_name("plan.yml", Some("2025.06"), Some("5.3.0"));
        assert_eq!(overridden.eessi_version.as_deref(), Some("2025.06"));
        assert_eq!(overridden.easybuild_version.as_deref(), Some("5.3.0"));
        assert_eq!(
            PlanContext::from_easystack_name("plan.yml", None, None),
            PlanContext::default()
        );
    }

    #[test]
    fn min_easybuild_and_eessi_versions_gate_an_entry() {
        let list = KnownFailureList::bundled().expect("bundled table");
        let gromacs = list.get("gromacs-cuda-demoted").expect("GROMACS");
        let old = PlanContext {
            eessi_version: Some("2026.06".into()),
            easybuild_version: Some("5.3.0".into()),
        };
        let new = PlanContext {
            eessi_version: Some("2026.06".into()),
            easybuild_version: Some("5.4.1".into()),
        };
        assert!(!old.admits(gromacs));
        assert!(new.admits(gromacs));
        assert!(PlanContext::default().admits(gromacs));
        let cargo = list.get("cargo-haswell-sigsegv").expect("cargo");
        assert!(!new.admits(cargo), "the haswell segfault is 2025.06 only");
    }

    #[test]
    fn filename_only_plans_match_by_name_version_and_cuda_suffix() {
        let list = KnownFailureList::bundled().expect("bundled table");
        let plan: Vec<EasystackEntry> = [
            "numba-0.62.0-foss-2025a.eb",
            "numba-0.61.2-foss-2025a.eb",
            "GROMACS-2026.3-foss-2026.1-CUDA-13.3.0.eb",
            "GROMACS-2026.3-foss-2026.1.eb",
        ]
        .iter()
        .map(|file| EasystackEntry {
            file: file.to_string(),
            from_commit: None,
            include_easyblocks_from_commit: None,
            include_easyblocks_from_pr: None,
        })
        .collect();
        let hits = known_failures_in_plan(
            &list,
            &["aarch64/a64fx".to_string()],
            &[],
            &plan,
            &PlanContext::default(),
            &|_| false,
        );
        let found: Vec<(&str, &str)> = hits
            .iter()
            .map(|hit| (hit.file.as_str(), hit.id.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "GROMACS-2026.3-foss-2026.1-CUDA-13.3.0.eb",
                    "gromacs-cuda-demoted"
                ),
                ("numba-0.62.0-foss-2025a.eb", "numba-a64fx-sum-axis"),
            ]
        );
        assert_eq!(hits[0].arches, ["aarch64/a64fx"]);
    }

    #[test]
    fn citations_group_by_repository() {
        let sources = vec![
            "https://github.com/EESSI/software-layer/pull/1871#issuecomment-6467091351".to_string(),
            "https://github.com/EESSI/software-layer/pull/1871#issuecomment-6475983835".to_string(),
            "https://github.com/EESSI/software-layer/pull/1829#issuecomment-6475355042".to_string(),
            "https://github.com/EESSI/software-layer-scripts/pull/330".to_string(),
            "https://github.com/EESSI/software-layer-scripts/commit/ce20b65f518b28c51e1b2fdf38ac262148bd0e9f".to_string(),
            "https://gitlab.com/eessi/support/-/work_items/270".to_string(),
        ];
        assert_eq!(
            citations(&sources),
            "software-layer #1871, #1829; software-layer-scripts #330, ce20b65"
        );
    }

    #[test]
    fn a_bad_phase_or_a_repeated_id_is_refused() {
        let entry = |id: &str, phase: &str| {
            format!(
                "[[entry]]\nid = \"{id}\"\nnames = [\"x\"]\narches = [\"*\"]\nphase = \"{phase}\"\nsymptom = \"s\"\nremedy = \"r\"\n"
            )
        };
        let bad = KnownFailureList::parse(&entry("a", "link")).unwrap_err();
        assert!(format!("{bad:#}").contains("phase link"), "{bad:#}");
        let twice = format!("{}{}", entry("a", "test"), entry("a", "build"));
        let repeated = KnownFailureList::parse(&twice).unwrap_err();
        assert!(
            format!("{repeated:#}").contains("repeats id a"),
            "{repeated:#}"
        );
        assert!(KnownFailureList::parse("")
            .expect("empty")
            .entries()
            .is_empty());
    }
}
