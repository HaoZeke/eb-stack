//! Validate CUDA compute-capability requests in an easystack plan.
//!
//! This check targets the failures seen in software-layer pull request 1461:
//! CUDA-dependent recipes in a shared easystack requested compute capabilities
//! that either the selected CUDA toolkit does not support, an easyblock cannot
//! map, or the EESSI accelerator targets do not build for.

use crate::build_order::{closure_against_installed, Choice, ModuleKey};
use crate::cli_parse::usage;
use crate::domain::{Candidate, DepReq};
use crate::easystack::parse_easystack_entries;
use crate::eb_parse::parse_easyconfig_trees;
use crate::version::cmp_version;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Bundled CUDA capability support table.
pub const BUNDLED_CUDA_CAPABILITIES: &str = include_str!("../data/cuda-capabilities.toml");
/// Bundled EESSI accelerator target table.
pub const BUNDLED_EESSI_ACCEL_TARGETS: &str =
    include_str!("../data/eessi-accelerator-targets.toml");
/// Bundled known easyblock capability-map failures.
pub const BUNDLED_EASYBLOCK_CC_MAP: &str =
    include_str!("../data/easyblock-cuda-capability-map.toml");

#[derive(Debug, Deserialize)]
struct RawCudaTable {
    #[serde(default)]
    entry: Vec<RawCudaEntry>,
}

#[derive(Debug, Deserialize)]
struct RawCudaEntry {
    cuda: String,
    #[serde(default)]
    supported: Vec<String>,
    #[serde(default)]
    dropped: Vec<String>,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Debug, Clone)]
struct CudaSupportEntry {
    supported: BTreeSet<String>,
    dropped: BTreeSet<String>,
    sources: Vec<String>,
}

#[derive(Debug, Clone)]
struct CudaSupportTable {
    by_version: BTreeMap<String, CudaSupportEntry>,
}

impl CudaSupportTable {
    fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_CUDA_CAPABILITIES).context("data/cuda-capabilities.toml")
    }

    fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let raw: RawCudaTable = toml::from_str(text).context("CUDA table is not valid TOML")?;
        let mut by_version = BTreeMap::new();
        for entry in raw.entry {
            let version = cuda_version_key(&entry.cuda).unwrap_or_else(|| entry.cuda.trim().into());
            if version.is_empty() {
                bail!("CUDA support entry needs a cuda version");
            }
            if by_version.contains_key(&version) {
                bail!("CUDA support table repeats version {version}");
            }
            let supported: BTreeSet<String> = entry
                .supported
                .iter()
                .filter_map(|cc| normalize_cc(cc))
                .collect();
            let dropped: BTreeSet<String> = entry
                .dropped
                .iter()
                .filter_map(|cc| normalize_cc(cc))
                .collect();
            by_version.insert(
                version,
                CudaSupportEntry {
                    supported,
                    dropped,
                    sources: entry.sources,
                },
            );
        }
        Ok(Self { by_version })
    }

    fn get(&self, cuda_version: &str) -> Option<&CudaSupportEntry> {
        let key = cuda_version_key(cuda_version).unwrap_or_else(|| cuda_version.to_string());
        self.by_version.get(&key)
    }
}

#[derive(Debug, Deserialize)]
struct RawTargetTable {
    #[serde(default)]
    target: Vec<RawAccelTarget>,
}

#[derive(Debug, Deserialize)]
struct RawAccelTarget {
    name: String,
    arch: String,
    #[serde(default)]
    required: Vec<String>,
    min_cuda: Option<String>,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Debug, Clone)]
struct AccelTarget {
    name: String,
    arch: String,
    required: BTreeSet<String>,
    min_cuda: Option<String>,
    sources: Vec<String>,
}

#[derive(Debug, Clone)]
struct AccelTargets {
    by_name: BTreeMap<String, AccelTarget>,
}

impl AccelTargets {
    fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_EESSI_ACCEL_TARGETS).context("data/eessi-accelerator-targets.toml")
    }

    fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let raw: RawTargetTable =
            toml::from_str(text).context("accelerator target table is not valid TOML")?;
        let mut by_name = BTreeMap::new();
        for target in raw.target {
            let name = target.name.trim().to_string();
            let arch = target.arch.trim().to_string();
            if name.is_empty() || arch.is_empty() {
                bail!("accelerator target needs name and arch");
            }
            if by_name.contains_key(&name) {
                bail!("accelerator target table repeats {name}");
            }
            let required: BTreeSet<String> = target
                .required
                .iter()
                .filter_map(|cc| normalize_cc(cc))
                .collect();
            by_name.insert(
                name.clone(),
                AccelTarget {
                    name,
                    arch,
                    required,
                    min_cuda: target.min_cuda.and_then(|v| cuda_version_key(&v)),
                    sources: target.sources,
                },
            );
        }
        Ok(Self { by_name })
    }

    fn selected(&self, names: &[String]) -> Result<Vec<&AccelTarget>> {
        if names.is_empty() {
            return Ok(self.by_name.values().collect());
        }
        let mut selected = Vec::new();
        for name in names {
            let Some(target) = self.by_name.get(name) else {
                let known = self.by_name.keys().cloned().collect::<Vec<_>>().join(", ");
                bail!("unknown --target {name}; known targets: {known}");
            };
            selected.push(target);
        }
        Ok(selected)
    }
}

#[derive(Debug, Deserialize)]
struct RawEasyblockTable {
    #[serde(default)]
    entry: Vec<RawEasyblockEntry>,
}

#[derive(Debug, Deserialize)]
struct RawEasyblockEntry {
    name: String,
    #[serde(default)]
    unsupported: Vec<String>,
    reason: String,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Debug, Clone)]
struct EasyblockEntry {
    unsupported: BTreeSet<String>,
    reason: String,
    sources: Vec<String>,
}

#[derive(Debug, Clone)]
struct EasyblockTable {
    by_name: BTreeMap<String, EasyblockEntry>,
}

impl EasyblockTable {
    fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_EASYBLOCK_CC_MAP).context("data/easyblock-cuda-capability-map.toml")
    }

    fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let raw: RawEasyblockTable =
            toml::from_str(text).context("easyblock map table is not valid TOML")?;
        let mut by_name = BTreeMap::new();
        for row in raw.entry {
            let name = row.name.trim().to_string();
            let reason = row.reason.trim().to_string();
            if name.is_empty() || reason.is_empty() {
                bail!("easyblock map entry needs name and reason");
            }
            if by_name.contains_key(&name) {
                bail!("easyblock map table repeats {name}");
            }
            by_name.insert(
                name,
                EasyblockEntry {
                    unsupported: row
                        .unsupported
                        .iter()
                        .filter_map(|cc| normalize_cc(cc))
                        .collect(),
                    reason,
                    sources: row.sources,
                },
            );
        }
        Ok(Self { by_name })
    }
}

/// Inputs for `stack cuda-cc`.
pub struct CudaCcRequest {
    /// Easystacks whose entries are checked.
    pub easystacks: Vec<PathBuf>,
    /// Easyconfig trees for dependency closure.
    pub easyconfigs: Vec<PathBuf>,
    /// EESSI accelerator targets to validate against. Empty means all bundled targets.
    pub targets: Vec<String>,
    /// Override bundled CUDA support table.
    pub cuda_table: Option<PathBuf>,
    /// Override bundled accelerator target table.
    pub accelerator_targets: Option<PathBuf>,
    /// Override bundled easyblock map table.
    pub easyblock_map: Option<PathBuf>,
}

/// What `stack cuda-cc` prints.
pub struct CudaCcReport {
    /// Text printed to stdout.
    pub stdout: String,
    /// Findings that fail the command.
    pub findings: Vec<String>,
}

/// Check CUDA compute capabilities in the plan's CUDA-dependent recipes.
pub fn run_cuda_cc_check(request: &CudaCcRequest) -> Result<CudaCcReport> {
    if request.easystacks.is_empty() {
        return Err(usage("pass at least one --easystack"));
    }
    if request.easyconfigs.is_empty() {
        return Err(usage("pass at least one --easyconfigs"));
    }
    let cuda_table = match &request.cuda_table {
        Some(path) => CudaSupportTable::load(path)?,
        None => CudaSupportTable::bundled()?,
    };
    let targets = match &request.accelerator_targets {
        Some(path) => AccelTargets::load(path)?,
        None => AccelTargets::bundled()?,
    };
    let selected_targets = targets.selected(&request.targets)?;
    let easyblock_table = match &request.easyblock_map {
        Some(path) => EasyblockTable::load(path)?,
        None => EasyblockTable::bundled()?,
    };

    let starts = read_easystack_entries(&request.easystacks)?;
    let trees: Vec<&Path> = request.easyconfigs.iter().map(PathBuf::as_path).collect();
    let parsed = parse_easyconfig_trees(&trees).map_err(|error| {
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
    let by_file = index_by_file(&parsed.candidates);
    let start_keys: Vec<ModuleKey> = starts
        .iter()
        .filter_map(|file| by_file.get(file).map(|candidate| ModuleKey::of(candidate)))
        .collect();
    let closure =
        closure_against_installed(&parsed.candidates, &start_keys, Choice::Newest, &|_| false);
    let closure_set: BTreeSet<ModuleKey> = closure.to_build.into_iter().collect();
    let mut findings = BTreeSet::new();
    let mut checked = 0usize;

    for candidate in &parsed.candidates {
        if !closure_set.contains(&ModuleKey::of(candidate)) || !is_cuda_dependent(candidate) {
            continue;
        }
        let Some(path) = file_path(&candidate.easyconfig_path) else {
            continue;
        };
        let requested = requested_ccs(path)?;
        if requested.is_empty() {
            continue;
        }
        checked += 1;
        let cuda_version = cuda_dependency_version(candidate);
        let file = basename(&candidate.easyconfig_path);
        if let Some(version) = cuda_version.as_deref() {
            if let Some(rule) = cuda_table.get(version) {
                let mut unsupported = Vec::new();
                let mut dropped = Vec::new();
                for cc in &requested {
                    if rule.supported.contains(cc) {
                        continue;
                    }
                    if rule.dropped.contains(cc) {
                        dropped.push(cc.clone());
                    } else {
                        unsupported.push(cc.clone());
                    }
                }
                if !dropped.is_empty() {
                    findings.insert(format!(
                        "{file}: CUDA {version} drops compute capabilities {}{}",
                        dropped.join(", "),
                        cites(&rule.sources),
                    ));
                }
                if !unsupported.is_empty() {
                    findings.insert(format!(
                        "{file}: CUDA {version} does not support compute capabilities {}{}",
                        unsupported.join(", "),
                        cites(&rule.sources),
                    ));
                }
            }
        }

        for target in &selected_targets {
            let missing: Vec<String> = target
                .required
                .iter()
                .filter(|cc| !requested.contains(*cc))
                .cloned()
                .collect();
            if !missing.is_empty() {
                findings.insert(format!(
                    "{file}: missing target capabilities {} for {} ({}){}",
                    missing.join(", "),
                    target.name,
                    target.arch,
                    cites(&target.sources),
                ));
            }
            if let (Some(min_cuda), Some(actual)) =
                (target.min_cuda.as_deref(), cuda_version.as_deref())
            {
                if cmp_version(actual, min_cuda).is_lt() {
                    findings.insert(format!(
                        "{file}: CUDA {actual} is below {target} minimum CUDA {min_cuda}{}",
                        cites(&target.sources),
                        target = target.name
                    ));
                }
            }
        }

        if let Some(rule) = easyblock_rule(&easyblock_table, candidate) {
            let blocked: Vec<String> = requested
                .iter()
                .filter(|cc| rule.unsupported.contains(*cc))
                .cloned()
                .collect();
            if !blocked.is_empty() {
                findings.insert(format!(
                    "{file}: {name} easyblock map rejects compute capabilities {}: {}{}",
                    blocked.join(", "),
                    rule.reason,
                    cites(&rule.sources),
                    name = candidate
                        .easyblock
                        .as_deref()
                        .unwrap_or(candidate.name.as_str())
                ));
            }
        }
    }
    let findings: Vec<String> = findings.into_iter().collect();
    let mut stdout = String::new();
    for finding in &findings {
        stdout.push_str(finding);
        stdout.push('\n');
    }
    stdout.push_str(&format!(
        "checked={} findings={} targets={}\n",
        checked,
        findings.len(),
        selected_targets.len()
    ));
    Ok(CudaCcReport { stdout, findings })
}

fn read_easystack_entries(paths: &[PathBuf]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            return Err(usage(format!(
                "--easystack {} is a directory; pass an easystack file",
                path.display()
            )));
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let entries =
            parse_easystack_entries(&text).with_context(|| format!("parse {}", path.display()))?;
        out.extend(entries.into_iter().map(|entry| entry.file));
    }
    Ok(out)
}

fn easyblock_rule<'a>(
    table: &'a EasyblockTable,
    candidate: &Candidate,
) -> Option<&'a EasyblockEntry> {
    candidate
        .easyblock
        .as_ref()
        .and_then(|name| table.by_name.get(name))
        .or_else(|| table.by_name.get(&candidate.name))
}

fn is_cuda_dependent(candidate: &Candidate) -> bool {
    candidate
        .dependencies
        .iter()
        .chain(candidate.builddependencies.iter())
        .any(|dep| dep.name == "CUDA" || dep.name == "CUDAcore")
}

fn cuda_dependency_version(candidate: &Candidate) -> Option<String> {
    candidate
        .dependencies
        .iter()
        .chain(candidate.builddependencies.iter())
        .find_map(dep_version)
}

fn dep_version(dep: &DepReq) -> Option<String> {
    if dep.name != "CUDA" && dep.name != "CUDAcore" {
        return None;
    }
    let req = dep.version_req.trim().trim_start_matches("==");
    cuda_version_key(req)
}

fn requested_ccs(path: &Path) -> Result<BTreeSet<String>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let Some(raw) = parse_assigned_list(&text, "cuda_compute_capabilities") else {
        return Ok(BTreeSet::new());
    };
    Ok(raw.into_iter().filter_map(|cc| normalize_cc(&cc)).collect())
}

fn parse_assigned_list(text: &str, name: &str) -> Option<Vec<String>> {
    for (index, _) in text.match_indices(name) {
        let before_ok = index == 0
            || text[..index]
                .chars()
                .next_back()
                .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
        if !before_ok {
            continue;
        }
        let mut rest = &text[index + name.len()..];
        rest = rest.trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        let rhs = rest[1..].trim_start();
        if let Some(list) = parse_simple_list(rhs) {
            return Some(list);
        }
    }
    None
}

fn parse_simple_list(text: &str) -> Option<Vec<String>> {
    let mut chars = text.chars();
    let open = chars.next()?;
    let close = match open {
        '[' => ']',
        '(' => ')',
        _ => return None,
    };
    let mut depth = 1usize;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    let mut body = String::new();
    for ch in chars {
        if let Some(q) = in_quote {
            body.push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == q {
                in_quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            in_quote = Some(ch);
            body.push(ch);
            continue;
        }
        if ch == open {
            depth += 1;
            body.push(ch);
            continue;
        }
        if ch == close {
            depth -= 1;
            if depth == 0 {
                break;
            }
            body.push(ch);
            continue;
        }
        body.push(ch);
    }
    let mut out = Vec::new();
    let mut item = String::new();
    let mut quote: Option<char> = None;
    let mut esc = false;
    for ch in body.chars() {
        if let Some(q) = quote {
            item.push(ch);
            if esc {
                esc = false;
                continue;
            }
            if ch == '\\' {
                esc = true;
                continue;
            }
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            item.push(ch);
            continue;
        }
        if ch == ',' {
            let trimmed = item.trim();
            if !trimmed.is_empty() {
                out.push(unquote(trimmed));
            }
            item.clear();
            continue;
        }
        item.push(ch);
    }
    let trimmed = item.trim();
    if !trimmed.is_empty() {
        out.push(unquote(trimmed));
    }
    Some(out)
}

fn unquote(text: &str) -> String {
    if text.len() >= 2 {
        let first = text.chars().next().unwrap_or_default();
        let last = text.chars().next_back().unwrap_or_default();
        if (first == '\'' && last == '\'') || (first == '"' && last == '"') {
            return text[1..text.len() - 1].trim().to_string();
        }
    }
    text.to_string()
}

fn normalize_cc(cc: &str) -> Option<String> {
    let raw = cc.trim().trim_start_matches("sm_").to_ascii_lowercase();
    if raw.is_empty() {
        return None;
    }
    if raw
        .chars()
        .all(|ch| ch.is_ascii_digit() || ch == '.' || ch.is_ascii_alphabetic())
    {
        if raw.contains('.') {
            return Some(raw);
        }
        let digits: String = raw.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        let suffix: String = raw.chars().skip_while(|ch| ch.is_ascii_digit()).collect();
        if digits.len() >= 2 {
            let (major, minor) = digits.split_at(digits.len() - 1);
            return Some(format!("{major}.{minor}{suffix}"));
        }
        return Some(raw);
    }
    None
}

fn cuda_version_key(version: &str) -> Option<String> {
    let text = version.trim().trim_start_matches("==");
    let mut parts = text.split('.');
    let major = parts
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    let minor = parts
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if major.is_empty() || minor.is_empty() {
        return None;
    }
    Some(format!("{major}.{minor}"))
}

fn index_by_file(candidates: &[Candidate]) -> BTreeMap<String, &Candidate> {
    let mut out = BTreeMap::new();
    for candidate in candidates {
        out.insert(basename(&candidate.easyconfig_path), candidate);
    }
    out
}

fn file_path(path: &str) -> Option<&Path> {
    (!path.is_empty()).then(|| Path::new(path))
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn cites(sources: &[String]) -> String {
    let refs: Vec<String> = sources
        .iter()
        .filter_map(|source| {
            let number = source.rsplit(['/', '#']).find(|part| !part.is_empty())?;
            number
                .chars()
                .all(|c| c.is_ascii_digit())
                .then(|| format!("#{number}"))
        })
        .collect();
    if refs.is_empty() {
        String::new()
    } else {
        format!(" (software-layer {})", refs.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc_normalization_covers_easybuild_spellings() {
        assert_eq!(normalize_cc("9.0a").as_deref(), Some("9.0a"));
        assert_eq!(normalize_cc("90").as_deref(), Some("9.0"));
        assert_eq!(normalize_cc("120f").as_deref(), Some("12.0f"));
        assert_eq!(normalize_cc("sm_90").as_deref(), Some("9.0"));
    }

    #[test]
    fn bundled_tables_cover_the_g8_failure_shape() {
        let cuda = CudaSupportTable::bundled().expect("cuda table");
        let entry = cuda.get("12.6.0").expect("CUDA 12.6");
        assert!(entry.dropped.contains("12.0f"));
        let targets = AccelTargets::bundled().expect("targets");
        assert!(targets.by_name.contains_key("zen4-cc90"));
        let map = EasyblockTable::bundled().expect("easyblock map");
        let lammps = map.by_name.get("LAMMPS").expect("LAMMPS");
        assert!(lammps.unsupported.contains("9.0a"));
    }

    #[test]
    fn simple_list_parser_reads_inline_and_multiline_lists() {
        let inline = "cuda_compute_capabilities = ['9.0', '9.0a']";
        assert_eq!(
            parse_assigned_list(inline, "cuda_compute_capabilities"),
            Some(vec!["9.0".into(), "9.0a".into()])
        );
        let multiline = "cuda_compute_capabilities = [\n  '9.0',\n  '12.0f',\n]\n";
        assert_eq!(
            parse_assigned_list(multiline, "cuda_compute_capabilities"),
            Some(vec!["9.0".into(), "12.0f".into()])
        );
    }
}
