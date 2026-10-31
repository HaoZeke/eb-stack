//! Architectures that cannot build an EasyBuild entry.
//!
//! EESSI builds one shared easystack on every CPU architecture. An entry
//! that cannot be built on one of them fails that architecture's bot run,
//! even when the pull request added something else. The list in
//! `data/arch-unsupported.toml` names those entries. A plan that includes
//! one, or depends on one, stops before anything is written.
//!
//! The other check is cheaper and does not read recipes. An entry in a
//! shared easystack that the architecture has never installed, and that is
//! not in that architecture's `additional_arch` easystack, is flagged.

use crate::build_order::{closure_against_installed, reverse_uses, Choice, ModuleKey, ReverseUse};
use crate::cli_parse::usage;
use crate::domain::Candidate;
use crate::easystack::{parse_easystack_entries, EasystackEntry};
use crate::eb_parse::parse_easyconfig_trees_for_arch;
use crate::pr_overlap::parse_installed;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The list shipped with eb-stack.
pub const BUNDLED_UNSUPPORTED: &str = include_str!("../data/arch-unsupported.toml");

/// One package an architecture cannot build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedEntry {
    /// Architecture family, `riscv64` or `aarch64`.
    pub arch: String,
    /// EasyBuild name, `GDRCopy`. Every version of that name is covered.
    pub name: String,
    /// Why the bot cannot build it.
    pub reason: String,
    /// Software-layer pull requests the entry was seeded from.
    pub sources: Vec<String>,
}

/// The unsupported list, keyed by architecture family and EasyBuild name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedList {
    entries: BTreeMap<(String, String), UnsupportedEntry>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(default)]
    entry: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEntry {
    arch: String,
    name: String,
    reason: String,
    #[serde(default)]
    sources: Vec<String>,
}

impl UnsupportedList {
    /// The list shipped in `data/arch-unsupported.toml`.
    pub fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_UNSUPPORTED).context("data/arch-unsupported.toml")
    }

    /// Read a list from `path`. An empty file is an empty list.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Parse the TOML document.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawFile = toml::from_str(text).context("unsupported list is not valid TOML")?;
        let mut entries = BTreeMap::new();
        for entry in raw.entry {
            let arch = arch_family(&entry.arch).to_string();
            let name = entry.name.trim().to_string();
            let reason = entry.reason.trim().to_string();
            if arch.is_empty() || name.is_empty() || reason.is_empty() {
                bail!("an unsupported entry needs an arch, a name, and a reason");
            }
            let key = (arch.clone(), name.clone());
            if entries.contains_key(&key) {
                bail!("unsupported list repeats {name} on {arch}");
            }
            entries.insert(
                key,
                UnsupportedEntry {
                    arch,
                    name,
                    reason,
                    sources: entry.sources,
                },
            );
        }
        Ok(Self { entries })
    }

    /// Architecture families the list mentions, in order.
    pub fn arches(&self) -> Vec<String> {
        let mut arches: Vec<String> = self.entries.keys().map(|(arch, _)| arch.clone()).collect();
        arches.sort();
        arches.dedup();
        arches
    }

    /// The entry for `name` on `arch`, if this architecture cannot build it.
    ///
    /// `arch` may be a family (`riscv64`) or a target (`riscv64/generic`).
    /// The family is the text before the first slash.
    pub fn get(&self, arch: &str, name: &str) -> Option<&UnsupportedEntry> {
        self.entries
            .get(&(arch_family(arch).to_string(), name.to_string()))
    }
}

/// `riscv64/generic` and `riscv64` are the same family.
pub fn arch_family(arch: &str) -> &str {
    arch.split('/').next().unwrap_or(arch).trim()
}

/// EasyBuild name from an easyconfig filename.
///
/// The name is every dash-separated part before the first part that opens
/// with a digit, which is the version: `bwa-mem2-2.3-GCC-14.3.0.eb` is
/// `bwa-mem2`, and `GDRCopy-2.5.2-GCCcore-15.2.0.eb` is `GDRCopy`.
pub fn easyconfig_name(filename: &str) -> Option<String> {
    let (name, _) = split_easyconfig_filename(filename)?;
    Some(name)
}

/// Module string for an easyconfig filename: `Name/version-toolchain`.
pub fn module_from_filename(filename: &str) -> Option<String> {
    let (name, versionfull) = split_easyconfig_filename(filename)?;
    Some(format!("{name}/{versionfull}"))
}

pub(crate) fn split_easyconfig_filename(filename: &str) -> Option<(String, String)> {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    let stem = base.strip_suffix(".eb").unwrap_or(base);
    let parts: Vec<&str> = stem.split('-').collect();
    let version_at = parts.iter().enumerate().skip(1).find_map(|(index, part)| {
        part.starts_with(|c: char| c.is_ascii_digit())
            .then_some(index)
    })?;
    if version_at == 0 {
        return None;
    }
    let name = parts[..version_at].join("-");
    let versionfull = parts[version_at..].join("-");
    if name.is_empty() || versionfull.is_empty() {
        return None;
    }
    Some((name, versionfull))
}

/// One unsupported entry a plan includes or depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanHit {
    /// Easyconfig filename of the unsupported entry.
    pub file: String,
    /// Architecture family.
    pub arch: String,
    /// Why that architecture cannot build it.
    pub reason: String,
    /// Pull requests the list cites for this entry.
    pub sources: Vec<String>,
    /// Plan entries that depend on `file`. Empty when the plan names `file`.
    pub required_by: BTreeSet<String>,
    /// The plan names `file` itself, so `required_by` stays empty.
    included: bool,
}

impl PlanHit {
    /// The line the command prints and the error repeats.
    pub fn message(&self) -> String {
        let mut line = format!(
            "{} is unsupported on {}: {}",
            self.file, self.arch, self.reason
        );
        let cites = pull_numbers(&self.sources);
        if !cites.is_empty() {
            line.push_str(" (software-layer ");
            line.push_str(&cites.join(", "));
            line.push(')');
        }
        if !self.included && !self.required_by.is_empty() {
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
        line
    }
}

fn pull_numbers(sources: &[String]) -> Vec<String> {
    sources
        .iter()
        .filter_map(|source| {
            let number = source.rsplit(['/', '#']).find(|part| !part.is_empty())?;
            number
                .chars()
                .all(|c| c.is_ascii_digit())
                .then(|| format!("#{number}"))
        })
        .collect()
}

/// Entries in `starts` that this list rejects for `arches`, including
/// dependencies of those entries. `arches` empty means every family in the list.
pub fn unsupported_in_plan(
    list: &UnsupportedList,
    arches: &[String],
    candidates: &[Candidate],
    starts: &[String],
) -> Vec<PlanHit> {
    let families: Vec<String> = if arches.is_empty() {
        list.arches()
    } else {
        arches
            .iter()
            .map(|arch| arch_family(arch).to_string())
            .collect()
    };
    let mut families = families;
    families.sort();
    families.dedup();

    let by_file = index_by_file(candidates);
    let mut start_keys: Vec<ModuleKey> = Vec::new();
    let mut start_files: BTreeSet<String> = BTreeSet::new();
    let mut missing: Vec<String> = Vec::new();
    for start in starts {
        let file = basename(start);
        start_files.insert(file.clone());
        match by_file.get(&file) {
            Some(candidate) => start_keys.push(ModuleKey::of(candidate)),
            None => missing.push(file),
        }
    }

    let mut hits: BTreeMap<(String, String), PlanHit> = BTreeMap::new();
    for file in &missing {
        let Some(name) = easyconfig_name(file) else {
            continue;
        };
        for arch in &families {
            if let Some(entry) = list.get(arch, &name) {
                record_included(&mut hits, entry, file);
            }
        }
    }
    if start_keys.is_empty() {
        return hits.into_values().collect();
    }

    let closure = closure_against_installed(candidates, &start_keys, Choice::Newest, &|_| false);
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
        for arch in &families {
            let Some(entry) = list.get(arch, &candidate.name) else {
                continue;
            };
            if start_set.contains(key) || start_files.contains(&file) {
                record_included(&mut hits, entry, &file);
                continue;
            }
            let requirers = plan_requirers(key, &start_set, &in_closure, &uses);
            let required_by: BTreeSet<String> = requirers
                .iter()
                .filter_map(|requirer| by_key.get(requirer))
                .map(|candidate| basename(&candidate.easyconfig_path))
                .collect();
            record_dependency(&mut hits, entry, &file, required_by);
        }
    }
    hits.into_values().collect()
}

fn record_included(
    hits: &mut BTreeMap<(String, String), PlanHit>,
    entry: &UnsupportedEntry,
    file: &str,
) {
    hits.insert(
        (entry.arch.clone(), file.to_string()),
        PlanHit {
            file: file.to_string(),
            arch: entry.arch.clone(),
            reason: entry.reason.clone(),
            sources: entry.sources.clone(),
            required_by: BTreeSet::new(),
            included: true,
        },
    );
}

fn record_dependency(
    hits: &mut BTreeMap<(String, String), PlanHit>,
    entry: &UnsupportedEntry,
    file: &str,
    required_by: BTreeSet<String>,
) {
    let key = (entry.arch.clone(), file.to_string());
    if let Some(existing) = hits.get_mut(&key) {
        if existing.included {
            return;
        }
        existing.required_by.extend(required_by);
        return;
    }
    hits.insert(
        key,
        PlanHit {
            file: file.to_string(),
            arch: entry.arch.clone(),
            reason: entry.reason.clone(),
            sources: entry.sources.clone(),
            required_by,
            included: false,
        },
    );
}

/// Start entries that reach `hit` through modules in the plan's closure.
pub(crate) fn plan_requirers(
    hit: &ModuleKey,
    starts: &BTreeSet<ModuleKey>,
    in_closure: &BTreeSet<ModuleKey>,
    uses: &BTreeMap<ModuleKey, Vec<ReverseUse>>,
) -> BTreeSet<ModuleKey> {
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(hit.clone());
    seen.insert(hit.clone());
    while let Some(key) = queue.pop_front() {
        let Some(edges) = uses.get(&key) else {
            continue;
        };
        for edge in edges {
            if !seen.insert(edge.dependent.clone()) {
                continue;
            }
            if !in_closure.contains(&edge.dependent) && !starts.contains(&edge.dependent) {
                continue;
            }
            if starts.contains(&edge.dependent) {
                found.insert(edge.dependent.clone());
            } else {
                queue.push_back(edge.dependent.clone());
            }
        }
    }
    found
}

pub(crate) fn index_by_file(candidates: &[Candidate]) -> BTreeMap<String, &Candidate> {
    let mut out = BTreeMap::new();
    // Robot paths are ordered by precedence; keep the first match in that
    // order when duplicate filenames exist across trees.
    for candidate in candidates.iter().rev() {
        out.insert(basename(&candidate.easyconfig_path), candidate);
    }
    out
}

pub(crate) fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// One shared-easystack entry the cheap check flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnprovenFlag {
    /// Easyconfig filename.
    pub file: String,
    /// Architecture the check was asked about.
    pub arch: String,
}

impl UnprovenFlag {
    /// The line the command prints.
    pub fn message(&self) -> String {
        format!(
            "{} is flagged on {}: it has never been installed there and is not in the {} additional_arch easystack",
            self.file, self.arch, self.arch
        )
    }
}

/// Shared-easystack entries `installed` has never seen on `arch` that
/// `additional` also does not name.
///
/// `installed` is `Name/VersionFull` lines. `additional` is easyconfig
/// filenames. Matching the install uses the filename's own module string,
/// so a different version does not count as this entry.
pub fn unproven_entries(
    arch: &str,
    shared: &[EasystackEntry],
    additional: &BTreeSet<String>,
    installed: &BTreeSet<String>,
) -> Vec<UnprovenFlag> {
    let mut flags = Vec::new();
    let mut seen = BTreeSet::new();
    for entry in shared {
        if !seen.insert(entry.file.clone()) {
            continue;
        }
        if additional.contains(&entry.file) {
            continue;
        }
        if let Some(module) = module_from_filename(&entry.file) {
            if installed.contains(&module) {
                continue;
            }
        }
        flags.push(UnprovenFlag {
            file: entry.file.clone(),
            arch: arch.to_string(),
        });
    }
    flags
}

/// Read easyconfig filenames from easystack files and, when `path` is a
/// directory, from every easystack under it.
pub fn easystack_filenames(paths: &[PathBuf]) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for path in paths {
        for file in easystack_files(path)? {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            let entries = parse_easystack_entries(&text)
                .with_context(|| format!("parse {}", file.display()))?;
            names.extend(entries.into_iter().map(|entry| entry.file));
        }
    }
    Ok(names)
}

fn easystack_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    if !path.is_dir() {
        bail!("easystack path {} does not exist", path.display());
    }
    let mut files = Vec::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))?;
        for entry in read {
            let entry = entry.with_context(|| format!("read {}", dir.display()))?;
            let child = entry.path();
            if child.is_dir() {
                stack.push(child);
                continue;
            }
            let is_yaml = child
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "yml" || ext == "yaml");
            if is_yaml {
                files.push(child);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Arguments for `stack arch`.
pub struct ArchCheckRequest {
    /// Architectures to check. Empty checks every family in the list.
    pub arches: Vec<String>,
    /// Shared easystacks, or the plan's easystack. Repeatable.
    pub easystacks: Vec<PathBuf>,
    /// Robot trees the dependency closure is read from. Empty checks only
    /// the filenames the easystacks name.
    pub easyconfigs: Vec<PathBuf>,
    /// `path` for the sole architecture, or `arch=path`. Repeatable.
    pub installed: Vec<String>,
    /// `path` for the sole architecture, or `arch=path`. A directory is
    /// every easystack under it. Repeatable.
    pub additional: Vec<String>,
    /// Override `data/arch-unsupported.toml`.
    pub unsupported: Option<PathBuf>,
    /// Optional per-architecture source probe.
    pub source_probe: Option<SourceProbeRequest>,
}

/// Arguments for the optional source probe of `stack arch`. The probe runs
/// on the architecture families `ArchCheckRequest::arches` names, or on
/// every family when that is empty.
pub struct SourceProbeRequest {
    /// Extra probe attempts after the first one.
    pub retries: u32,
    /// Per-request timeout.
    pub timeout: Duration,
}

/// What `stack arch` prints.
pub struct ArchCheckReport {
    /// Stdout, including the count line.
    pub stdout: String,
    /// Arch-specific parse failures for easystack entries.
    pub parse_failed: Vec<String>,
    /// Unsupported-entry lines. Non-empty fails the command.
    pub unsupported: Vec<String>,
    /// Cheap-check lines. Non-empty fails the command.
    pub flagged: Vec<String>,
    /// Source probe findings. Non-empty fails the command.
    pub source_findings: Vec<String>,
}

/// Check a plan against the unsupported list, and flag shared entries the
/// architecture has never installed.
pub fn run_arch_check(request: &ArchCheckRequest) -> Result<ArchCheckReport> {
    if request.easystacks.is_empty() {
        return Err(usage("pass at least one --easystack"));
    }
    let list = match &request.unsupported {
        Some(path) => UnsupportedList::load(path)?,
        None => UnsupportedList::bundled()?,
    };
    let mut starts = Vec::new();
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
        starts.extend(entries.into_iter().map(|entry| entry.file));
    }

    if request.source_probe.is_some() && request.easyconfigs.is_empty() {
        return Err(usage("--probe-sources needs --easyconfigs"));
    }

    let parse_arches = parse_arch_families(request);
    let mut plan_files: BTreeSet<PathBuf> = BTreeSet::new();
    let mut failed_pairs: BTreeSet<(String, String)> = BTreeSet::new();
    let (parse_failed, unsupported): (Vec<String>, Vec<String>) = if request.easyconfigs.is_empty()
    {
        let plan_hits = unsupported_in_plan(&list, &request.arches, &[], &starts);
        (Vec::new(), plan_hits.iter().map(PlanHit::message).collect())
    } else {
        let trees: Vec<&Path> = request.easyconfigs.iter().map(PathBuf::as_path).collect();
        let start_files: BTreeSet<String> = starts.iter().map(|entry| basename(entry)).collect();
        let mut parse_failed = Vec::new();
        let mut unsupported = Vec::new();
        for arch in &parse_arches {
            let parsed = parse_easyconfig_trees_for_arch(&trees, arch).map_err(|error| {
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
            for skipped in &parsed.skipped {
                let file = basename(&skipped.path);
                if !start_files.contains(&file) {
                    continue;
                }
                parse_failed.push(format!(
                    "{file} failed to parse on {arch}: {}",
                    skipped.error
                ));
                failed_pairs.insert((file, arch.clone()));
            }
            let hits = unsupported_in_plan(
                &list,
                std::slice::from_ref(arch),
                &parsed.candidates,
                &starts,
            );
            unsupported.extend(hits.into_iter().map(|hit| hit.message()));
            if request.source_probe.is_some() {
                plan_files.extend(plan_easyconfig_paths(&parsed.candidates, &starts));
            }
        }
        parse_failed.sort();
        parse_failed.dedup();
        unsupported.sort();
        unsupported.dedup();
        (parse_failed, unsupported)
    };

    let flagged = cheap_flags(request, &list, &starts)?;
    let source_findings = match &request.source_probe {
        Some(probe) => probe_plan_sources(probe, &parse_arches, &plan_files, &failed_pairs)?,
        None => Vec::new(),
    };
    let mut stdout = String::new();
    for line in parse_failed
        .iter()
        .chain(unsupported.iter())
        .chain(flagged.iter())
        .chain(source_findings.iter())
    {
        stdout.push_str(line);
        stdout.push('\n');
    }
    stdout.push_str(&format!(
        "parse_failed={} unsupported={} flagged={}",
        parse_failed.len(),
        unsupported.len(),
        flagged.len()
    ));
    if request.source_probe.is_some() {
        let source_unreachable = source_findings
            .iter()
            .filter(|line| line.contains(" is unreachable "))
            .count();
        let source_arch_missing = source_findings
            .iter()
            .filter(|line| line.contains(" is arch-missing "))
            .count();
        stdout.push_str(&format!(
            " source_unreachable={source_unreachable} source_arch_missing={source_arch_missing}"
        ));
    }
    stdout.push('\n');
    Ok(ArchCheckReport {
        stdout,
        parse_failed,
        unsupported,
        flagged,
        source_findings,
    })
}

/// Probe each plan file on every parsed architecture family it parsed on.
/// A pair already reported in `parse_failed` has no sources to resolve.
fn probe_plan_sources(
    probe: &SourceProbeRequest,
    families: &[String],
    plan_files: &BTreeSet<PathBuf>,
    failed_pairs: &BTreeSet<(String, String)>,
) -> Result<Vec<String>> {
    let options = crate::source_probe::ProbeOptions {
        retries: probe.retries,
        timeout: probe.timeout,
    };
    let mut findings = Vec::new();
    for path in plan_files {
        let file = basename(&path.display().to_string());
        let arches: Vec<String> = families
            .iter()
            .filter(|family| !failed_pairs.contains(&(file.clone(), (*family).clone())))
            .cloned()
            .collect();
        if arches.is_empty() {
            continue;
        }
        findings.extend(
            crate::source_probe::probe_easyconfig_sources(
                std::slice::from_ref(path),
                &arches,
                &options,
            )
            .map_err(|error| anyhow::anyhow!("source probe: {error}"))?,
        );
    }
    findings.sort_by(|a, b| {
        (&a.easyconfig, &a.source, &a.arch, &a.kind).cmp(&(
            &b.easyconfig,
            &b.source,
            &b.arch,
            &b.kind,
        ))
    });
    Ok(findings.iter().map(|finding| finding.message()).collect())
}

fn parse_arch_families(request: &ArchCheckRequest) -> Vec<String> {
    if request.arches.is_empty() {
        return crate::source_probe::DEFAULT_TARGET_ARCHES
            .iter()
            .map(|arch| arch.to_string())
            .collect();
    }
    let mut families: Vec<String> = request
        .arches
        .iter()
        .map(|arch| arch_family(arch).to_string())
        .collect();
    families.sort();
    families.dedup();
    families
}

fn plan_easyconfig_paths(candidates: &[Candidate], starts: &[String]) -> Vec<PathBuf> {
    let by_file = index_by_file(candidates);
    let by_key: BTreeMap<ModuleKey, &Candidate> = candidates
        .iter()
        .map(|candidate| (ModuleKey::of(candidate), candidate))
        .collect();
    let mut start_keys: Vec<ModuleKey> = Vec::new();
    for start in starts {
        let file = basename(start);
        if let Some(candidate) = by_file.get(&file) {
            start_keys.push(ModuleKey::of(candidate));
        }
    }
    let closure = closure_against_installed(candidates, &start_keys, Choice::Newest, &|_| false);
    let mut paths = BTreeSet::new();
    for key in closure.to_build {
        if let Some(candidate) = by_key.get(&key) {
            paths.insert(PathBuf::from(candidate.easyconfig_path.clone()));
        }
    }
    paths.into_iter().collect()
}

fn cheap_flags(
    request: &ArchCheckRequest,
    list: &UnsupportedList,
    starts: &[String],
) -> Result<Vec<String>> {
    if request.installed.is_empty() {
        return Ok(Vec::new());
    }
    let families = request_families(&request.arches, list);
    let installed = assign_arch_paths(&request.installed, &families, "--installed")?;
    let additional = assign_arch_paths(&request.additional, &families, "--additional-arch")?;
    let shared: Vec<EasystackEntry> = starts
        .iter()
        .map(|file| EasystackEntry {
            file: file.clone(),
            from_commit: None,
            include_easyblocks_from_commit: None,
            include_easyblocks_from_pr: None,
        })
        .collect();
    let mut lines = Vec::new();
    for arch in families {
        let Some(installed_paths) = installed.get(&arch) else {
            continue;
        };
        let mut modules = BTreeSet::new();
        for path in installed_paths {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("--installed {}", path.display()))?;
            modules.extend(parse_installed(&text));
        }
        let additional_names = match additional.get(&arch) {
            Some(paths) => easystack_filenames(paths)?,
            None => BTreeSet::new(),
        };
        for flag in unproven_entries(&arch, &shared, &additional_names, &modules) {
            lines.push(flag.message());
        }
    }
    Ok(lines)
}

fn request_families(arches: &[String], list: &UnsupportedList) -> Vec<String> {
    let mut families: Vec<String> = if arches.is_empty() {
        list.arches()
    } else {
        arches
            .iter()
            .map(|arch| arch_family(arch).to_string())
            .collect()
    };
    families.sort();
    families.dedup();
    families
}

/// `riscv64=path` binds to that family. A bare path binds to the sole
/// architecture. More than one architecture needs the prefix.
fn assign_arch_paths(
    specs: &[String],
    families: &[String],
    flag: &str,
) -> Result<BTreeMap<String, Vec<PathBuf>>> {
    let mut out: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for spec in specs {
        let (arch, path) = split_arch_path(spec);
        let bound: Vec<String> = match arch {
            Some(arch) => vec![arch_family(arch).to_string()],
            None if families.len() == 1 => vec![families[0].clone()],
            None => {
                return Err(usage(format!(
                    "{flag} {spec} needs an architecture prefix such as riscv64={spec}, because more than one architecture is being checked"
                )));
            }
        };
        for arch in bound {
            if !families.iter().any(|family| family == &arch) {
                return Err(usage(format!(
                    "{flag} names {arch}, which is not an architecture being checked"
                )));
            }
            out.entry(arch).or_default().push(PathBuf::from(path));
        }
    }
    Ok(out)
}

fn split_arch_path(spec: &str) -> (Option<&str>, &str) {
    match spec.split_once('=') {
        Some((arch, path)) if !arch.is_empty() && !path.is_empty() && !arch.contains('/') => {
            (Some(arch), path)
        }
        _ => (None, spec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_list_names_the_bot_failures_and_not_the_patched_one() {
        let list = UnsupportedList::bundled().expect("bundled list");
        let gdrcopy = list.get("riscv64", "GDRCopy").expect("GDRCopy");
        assert!(gdrcopy.reason.contains("CUDA"), "{}", gdrcopy.reason);
        assert!(gdrcopy
            .sources
            .iter()
            .any(|source| source.ends_with("/1775")));
        assert!(gdrcopy
            .sources
            .iter()
            .any(|source| source.ends_with("/1822")));
        assert!(list.get("riscv64/generic", "GDRCopy").is_some());
        let bwa = list
            .get("aarch64/neoverse_v1", "bwa-mem2")
            .expect("bwa-mem2");
        assert!(bwa.reason.contains("-msse"), "{}", bwa.reason);
        assert!(bwa.sources.iter().any(|source| source.ends_with("/1876")));
        assert!(list.get("aarch64", "waLBerla").is_none());
        assert!(list.get("x86_64", "GDRCopy").is_none());
    }

    #[test]
    fn filename_split_keeps_a_hyphenated_name() {
        assert_eq!(
            module_from_filename("bwa-mem2-2.3-GCC-14.3.0.eb").as_deref(),
            Some("bwa-mem2/2.3-GCC-14.3.0")
        );
        assert_eq!(
            easyconfig_name("GDRCopy-2.5.2-GCCcore-15.2.0.eb").as_deref(),
            Some("GDRCopy")
        );
        assert_eq!(
            module_from_filename("SIONlib-1.7.7-GCCcore-15.2.0-tools.eb").as_deref(),
            Some("SIONlib/1.7.7-GCCcore-15.2.0-tools")
        );
    }
}
