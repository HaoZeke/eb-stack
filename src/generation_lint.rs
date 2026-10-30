//! Toolchain generations an EESSI version carries, checked on a plan's easystacks.
//!
//! Each EESSI version is built on a fixed set of toolchain generations:
//! 2023.06 on 2022b, 2023a and 2023b, 2025.06 on 2024a, 2025a and 2025b.
//! `data/eessi-generations.toml` is that table. An easystack entry on a
//! toolchain outside its version's generations cannot go into that version:
//! software-layer #1472 put pv on GCCcore-12.3.0, which is 2023a, into a
//! 2025.06 easystack. The bot builds one EESSI version per pull request, so
//! easystacks for two versions in one pull request (#1834) fail as well.
//!
//! A plan that adds a second version of a software in one generation, next
//! to one the plan or the stack already has, also fails unless that name is
//! allowed explicitly. Some names carry several versions on purpose (CMake,
//! Rust), and the allow is where a reviewer says so.

use crate::arch_support::easystack_filenames;
use crate::cli_parse::usage;
use crate::domain::Toolchain;
use crate::easystack::{filename_toolchain, parse_easystack_entries};
use crate::hierarchy::known_hierarchy;
use crate::pr_overlap::{added_entries, eessi_version_of, head_entries, read_pr_inputs};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The table shipped with eb-stack.
pub const BUNDLED_GENERATIONS: &str = include_str!("../data/eessi-generations.toml");

/// EESSI versions and the toolchain generations each one carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationTable {
    versions: BTreeMap<String, Vec<String>>,
    extra: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    #[serde(default)]
    version: Vec<RawVersion>,
    #[serde(default)]
    generation: Vec<RawGeneration>,
}

#[derive(Debug, Deserialize)]
struct RawVersion {
    eessi: String,
    generations: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawGeneration {
    name: String,
    toolchains: Vec<String>,
}

impl GenerationTable {
    /// The table shipped in `data/eessi-generations.toml`.
    pub fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_GENERATIONS).context("data/eessi-generations.toml")
    }

    /// Read a table from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Parse the TOML document.
    ///
    /// A generation belongs to one EESSI version. That is what lets a
    /// finding name the version an entry belongs in.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawFile = toml::from_str(text).context("generation table is not valid TOML")?;
        let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut owner: BTreeMap<String, String> = BTreeMap::new();
        for entry in raw.version {
            let eessi = entry.eessi.trim().to_string();
            let generations: Vec<String> = entry
                .generations
                .iter()
                .map(|g| g.trim().to_string())
                .filter(|g| !g.is_empty())
                .collect();
            if eessi.is_empty() || generations.is_empty() {
                bail!("a version entry needs an eessi version and at least one generation");
            }
            if versions.contains_key(&eessi) {
                bail!("generation table repeats EESSI {eessi}");
            }
            for generation in &generations {
                if let Some(other) = owner.insert(generation.clone(), eessi.clone()) {
                    bail!("generation {generation} is listed for EESSI {other} and {eessi}");
                }
            }
            versions.insert(eessi, generations);
        }
        let mut extra: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entry in raw.generation {
            let name = entry.name.trim().to_string();
            if !owner.contains_key(&name) {
                bail!("generation {name} has toolchains but no EESSI version carries it");
            }
            extra
                .entry(name)
                .or_default()
                .extend(entry.toolchains.iter().map(|t| t.trim().to_string()));
        }
        Ok(Self { versions, extra })
    }

    /// EESSI versions the table knows, in order.
    pub fn versions(&self) -> Vec<&str> {
        self.versions.keys().map(String::as_str).collect()
    }

    /// Generations `eessi` carries, or `None` for a version the table lacks.
    pub fn generations(&self, eessi: &str) -> Option<&[String]> {
        self.versions.get(eessi).map(Vec::as_slice)
    }

    /// The EESSI version that carries `generation`.
    pub fn version_carrying(&self, generation: &str) -> Option<&str> {
        self.versions
            .iter()
            .find(|(_, generations)| generations.iter().any(|g| g == generation))
            .map(|(eessi, _)| eessi.as_str())
    }

    /// Toolchain labels of `generation` other than system: the shipped foss
    /// hierarchy and the table's extra toolchains.
    pub fn members(&self, generation: &str) -> Vec<String> {
        let parent = Toolchain {
            name: "foss".into(),
            version: generation.to_string(),
        };
        let mut labels: Vec<String> = known_hierarchy(&parent)
            .map(|h| h.bare_tuple_labels())
            .unwrap_or_default();
        labels.extend(self.extra.get(generation).cloned().unwrap_or_default());
        labels
    }

    /// The generation a toolchain label such as `GCCcore-12.3.0` is in.
    ///
    /// A toolchain whose version is a generation (`lompi-2025b`) is in that
    /// generation even when no EESSI version carries it. A compiler version
    /// outside every hierarchy the table carries is `None`.
    pub fn generation_of(&self, toolchain: &str) -> Option<String> {
        for generations in self.versions.values() {
            for generation in generations {
                let named = toolchain
                    .strip_suffix(generation.as_str())
                    .is_some_and(|head| head.ends_with('-'));
                if named || self.members(generation).iter().any(|m| m == toolchain) {
                    return Some(generation.clone());
                }
            }
        }
        let (_, version) = toolchain.rsplit_once('-')?;
        looks_like_generation(version).then(|| version.to_string())
    }
}

/// `2023a`, `2025b`, `2026.1`.
fn looks_like_generation(version: &str) -> bool {
    let Some((year, rest)) = version.split_at_checked(4) else {
        return false;
    };
    year.chars().all(|c| c.is_ascii_digit())
        && (matches!(rest, "a" | "b")
            || rest
                .strip_prefix('.')
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())))
}

/// One easystack file of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanFile {
    /// Path, from which the EESSI version is read.
    pub path: String,
    /// Easyconfig filenames the plan adds.
    pub added: Vec<String>,
    /// Easyconfig filenames already in the file, which count as installed.
    pub kept: Vec<String>,
}

/// The easystacks one pull request, or one run over easystack files, changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// `#1472` for a pull request. `None` for easystack files.
    pub label: Option<String>,
    /// The files.
    pub files: Vec<PlanFile>,
}

/// A name allowed more than one version, in one generation or in all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionAllow {
    /// EasyBuild name, `CMake`.
    pub name: String,
    /// Generation, `2025b`. `None` allows every generation.
    pub generation: Option<String>,
}

impl VersionAllow {
    /// `NAME` or `NAME@GENERATION`.
    pub fn parse(spec: &str) -> Result<Self> {
        let (name, generation) = match spec.split_once('@') {
            Some((name, generation)) => (name.trim(), Some(generation.trim())),
            None => (spec.trim(), None),
        };
        if name.is_empty() || generation.is_some_and(str::is_empty) {
            return Err(usage(format!(
                "--allow-versions {spec}: expected NAME or NAME@GENERATION"
            )));
        }
        Ok(Self {
            name: name.to_string(),
            generation: generation.map(String::from),
        })
    }

    fn covers(&self, name: &str, generation: &str) -> bool {
        self.name == name && self.generation.as_deref().is_none_or(|g| g == generation)
    }
}

/// What a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// An entry on a toolchain outside its EESSI version's generations.
    Outside,
    /// Easystacks for more than one EESSI version, or not the one asked for.
    Mixed,
    /// A second version of a name in one generation, not allowed.
    Versions,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What it is about.
    pub kind: FindingKind,
    /// The line the command prints.
    pub message: String,
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The EESSI version a path names: the directory under
/// `software.eessi.io/`, or `eessi-<V>-eb-` in the file name.
pub fn eessi_version_named(path: &str) -> Option<String> {
    if let Some(version) = eessi_version_of(path) {
        return Some(version);
    }
    let rest = basename(path).strip_prefix("eessi-")?;
    let (version, _) = rest.split_once("-eb-")?;
    (!version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .then(|| version.to_string())
}

#[derive(Default)]
struct VersionSeen {
    added: Vec<String>,
    existing: bool,
}

/// Lint one plan. `eessi_version` is the version for a path that names none,
/// and the version every path is expected to name. `existing` holds the
/// easyconfig filenames already in the stack.
pub fn lint_plan(
    table: &GenerationTable,
    plan: &Plan,
    eessi_version: Option<&str>,
    existing: &BTreeSet<String>,
    allow: &[VersionAllow],
) -> Result<Vec<Finding>> {
    let prefix = plan
        .label
        .as_deref()
        .map(|label| format!("{label}: "))
        .unwrap_or_default();
    let mut findings = Vec::new();

    let mut file_versions = Vec::new();
    let mut by_version: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for file in &plan.files {
        let named = eessi_version_named(&file.path);
        if let (Some(asked), Some(named)) = (eessi_version, named.as_deref()) {
            if asked != named {
                findings.push(Finding {
                    kind: FindingKind::Mixed,
                    message: format!(
                        "{prefix}{} is for EESSI {named}, not --eessi-version {asked}",
                        basename(&file.path)
                    ),
                });
            }
        }
        let Some(version) = named.or_else(|| eessi_version.map(String::from)) else {
            return Err(usage(format!(
                "{} names no EESSI version; pass --eessi-version",
                file.path
            )));
        };
        if table.generations(&version).is_none() {
            return Err(usage(format!(
                "EESSI {version} ({}) is not in the generation table, which has {}",
                file.path,
                table.versions().join(", ")
            )));
        }
        by_version
            .entry(version.clone())
            .or_default()
            .push(basename(&file.path));
        file_versions.push(version);
    }
    if by_version.len() > 1 {
        let listed: Vec<String> = by_version
            .iter()
            .map(|(version, files)| format!("{version} ({})", files.join(", ")))
            .collect();
        findings.push(Finding {
            kind: FindingKind::Mixed,
            message: format!(
                "{prefix}the easystacks are for {} EESSI versions: {}; the bot builds one EESSI version per pull request",
                by_version.len(),
                listed.join(", ")
            ),
        });
    }

    for (file, version) in plan.files.iter().zip(&file_versions) {
        let carried = table.generations(version).unwrap_or_default();
        for entry in &file.added {
            let Some(split) = filename_toolchain(entry) else {
                continue;
            };
            let generation = table.generation_of(&split.toolchain);
            if generation
                .as_deref()
                .is_some_and(|g| carried.iter().any(|c| c == g))
            {
                continue;
            }
            let placed = match &generation {
                Some(g) => match table.version_carrying(g) {
                    Some(home) => format!("is generation {g}, which EESSI {home} carries"),
                    None => format!("is generation {g}, which no EESSI version carries"),
                },
                None => "is in no generation an EESSI version carries".to_string(),
            };
            findings.push(Finding {
                kind: FindingKind::Outside,
                message: format!(
                    "{prefix}{entry} in {} is outside EESSI {version}: {} {placed}; {version} carries {}",
                    basename(&file.path),
                    split.toolchain,
                    carried.join(", ")
                ),
            });
        }
    }

    let mut groups: BTreeMap<(String, String), BTreeMap<String, VersionSeen>> = BTreeMap::new();
    let mut note = |file: &str, added: bool| {
        let Some(split) = filename_toolchain(file) else {
            return;
        };
        let Some(generation) = table.generation_of(&split.toolchain) else {
            return;
        };
        let seen = groups
            .entry((generation, split.name))
            .or_default()
            .entry(split.version)
            .or_default();
        if added {
            if !seen.added.iter().any(|f| f == file) {
                seen.added.push(file.to_string());
            }
        } else {
            seen.existing = true;
        }
    };
    for file in &plan.files {
        for entry in &file.added {
            note(entry, true);
        }
    }
    let added: BTreeSet<&str> = plan
        .files
        .iter()
        .flat_map(|f| f.added.iter().map(String::as_str))
        .collect();
    for entry in plan
        .files
        .iter()
        .flat_map(|f| f.kept.iter())
        .chain(existing.iter())
    {
        if !added.contains(entry.as_str()) {
            note(entry, false);
        }
    }
    for ((generation, name), versions) in &groups {
        if versions.len() < 2 || versions.values().all(|seen| seen.added.is_empty()) {
            continue;
        }
        if allow.iter().any(|a| a.covers(name, generation)) {
            continue;
        }
        let listed: Vec<String> = versions
            .iter()
            .map(|(version, seen)| {
                if seen.added.is_empty() {
                    format!("{version} (already in the stack)")
                } else {
                    format!("{version} (added: {})", seen.added.join(", "))
                }
            })
            .collect();
        findings.push(Finding {
            kind: FindingKind::Versions,
            message: format!(
                "{prefix}{name} has {} versions in generation {generation}: {}; pass --allow-versions {name}@{generation} to accept",
                versions.len(),
                listed.join(", ")
            ),
        });
    }
    Ok(findings)
}

/// Arguments for `stack generation`.
pub struct GenerationLintRequest {
    /// Easystack files. Every entry is part of the plan.
    pub easystacks: Vec<PathBuf>,
    /// Pull-request JSON in the `pr-overlap --pr` shape. The lines each pull
    /// request adds are its plan, and the rest of the file is the stack.
    pub prs: Vec<PathBuf>,
    /// EESSI version for a path that names none, and the one every path
    /// should name.
    pub eessi_version: Option<String>,
    /// Easystacks already deployed, or directories of them.
    pub deployed: Vec<PathBuf>,
    /// `NAME` or `NAME@GENERATION` allowed a second version.
    pub allow_versions: Vec<String>,
    /// Replace `data/eessi-generations.toml`.
    pub generations: Option<PathBuf>,
}

/// What `stack generation` prints.
pub struct GenerationLintReport {
    /// Stdout, including the count line.
    pub stdout: String,
    /// Every finding, in print order.
    pub findings: Vec<Finding>,
}

impl GenerationLintReport {
    /// The messages of one kind.
    pub fn of(&self, kind: FindingKind) -> Vec<String> {
        self.findings
            .iter()
            .filter(|f| f.kind == kind)
            .map(|f| f.message.clone())
            .collect()
    }
}

/// Lint easystack files, or the easystack changes of pull requests, against
/// the generations of their EESSI version.
pub fn run_generation_lint(request: &GenerationLintRequest) -> Result<GenerationLintReport> {
    if request.easystacks.is_empty() && request.prs.is_empty() {
        return Err(usage("pass at least one --easystack or --pr"));
    }
    let table = match &request.generations {
        Some(path) => GenerationTable::load(path)?,
        None => GenerationTable::bundled()?,
    };
    let allow = request
        .allow_versions
        .iter()
        .map(|spec| VersionAllow::parse(spec))
        .collect::<Result<Vec<_>>>()?;
    let existing = if request.deployed.is_empty() {
        BTreeSet::new()
    } else {
        easystack_filenames(&request.deployed).context("--deployed")?
    };

    let mut plans = Vec::new();
    if !request.easystacks.is_empty() {
        let mut files = Vec::new();
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
            files.push(PlanFile {
                path: path.display().to_string(),
                added: entries.into_iter().map(|entry| entry.file).collect(),
                kept: Vec::new(),
            });
        }
        plans.push(Plan { label: None, files });
    }
    for path in &request.prs {
        for pr in read_pr_inputs(path, "--pr")? {
            let files: Vec<PlanFile> = pr
                .easystacks
                .iter()
                .filter(|change| change.head.is_some())
                .map(|change| {
                    let added: Vec<String> =
                        added_entries(change).into_iter().map(|e| e.file).collect();
                    let kept = head_entries(change)
                        .into_iter()
                        .map(|e| e.file)
                        .filter(|file| !added.contains(file))
                        .collect();
                    PlanFile {
                        path: change.path.clone(),
                        added,
                        kept,
                    }
                })
                .collect();
            if !files.is_empty() {
                plans.push(Plan {
                    label: Some(format!("#{}", pr.number)),
                    files,
                });
            }
        }
    }

    let mut findings = Vec::new();
    for plan in &plans {
        findings.extend(lint_plan(
            &table,
            plan,
            request.eessi_version.as_deref(),
            &existing,
            &allow,
        )?);
    }
    let mut stdout = String::new();
    for finding in &findings {
        stdout.push_str(&finding.message);
        stdout.push('\n');
    }
    let count = |kind| findings.iter().filter(|f| f.kind == kind).count();
    stdout.push_str(&format!(
        "outside={} mixed={} versions={}\n",
        count(FindingKind::Outside),
        count(FindingKind::Mixed),
        count(FindingKind::Versions)
    ));
    Ok(GenerationLintReport { stdout, findings })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(label: Option<&str>, files: &[(&str, &[&str], &[&str])]) -> Plan {
        Plan {
            label: label.map(String::from),
            files: files
                .iter()
                .map(|(path, added, kept)| PlanFile {
                    path: path.to_string(),
                    added: added.iter().map(|s| s.to_string()).collect(),
                    kept: kept.iter().map(|s| s.to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn bundled_table_places_each_generation_in_one_version() {
        let table = GenerationTable::bundled().expect("bundled table");
        assert_eq!(table.versions(), ["2023.06", "2025.06", "2026.06"]);
        assert_eq!(
            table.generations("2025.06").unwrap(),
            ["2024a", "2025a", "2025b"]
        );
        assert_eq!(
            table.generation_of("GCCcore-12.3.0").as_deref(),
            Some("2023a")
        );
        assert_eq!(table.generation_of("GCC-12.2.0").as_deref(), Some("2022b"));
        assert_eq!(table.generation_of("gompi-2025b").as_deref(), Some("2025b"));
        assert_eq!(
            table.generation_of("lompi-2026.1").as_deref(),
            Some("2026.1")
        );
        assert_eq!(
            table.generation_of("llvm-compilers-20.1.8").as_deref(),
            Some("2025b")
        );
        assert_eq!(table.generation_of("foss-2022a").as_deref(), Some("2022a"));
        assert_eq!(table.generation_of("GCCcore-11.3.0"), None);
        assert_eq!(table.version_carrying("2023a"), Some("2023.06"));
        assert_eq!(table.version_carrying("2022a"), None);
    }

    #[test]
    fn a_generation_in_two_versions_is_refused() {
        let err = GenerationTable::parse(
            "[[version]]\neessi = \"a\"\ngenerations = [\"2025a\"]\n[[version]]\neessi = \"b\"\ngenerations = [\"2025a\"]\n",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("2025a is listed for EESSI a and b"));
    }

    #[test]
    fn versions_named_by_directory_or_file_name() {
        assert_eq!(
            eessi_version_named("easystacks/software.eessi.io/2025.06/x.yml").as_deref(),
            Some("2025.06")
        );
        assert_eq!(
            eessi_version_named("/tmp/eessi-2026.06-eb-5.4.1-2026.1.yml").as_deref(),
            Some("2026.06")
        );
        assert_eq!(eessi_version_named("/tmp/stack.yml"), None);
    }

    #[test]
    fn an_old_compiler_in_a_new_version_names_where_it_belongs() {
        let table = GenerationTable::bundled().unwrap();
        let findings = lint_plan(
            &table,
            &plan(
                Some("#1472"),
                &[(
                    "easystacks/software.eessi.io/2025.06/eessi-2025.06-eb-5.2.1-2025b.yml",
                    &["pv-1.7.24-GCCcore-12.3.0.eb"],
                    &["JUBE-2.7.1-GCCcore-14.3.0.eb"],
                )],
            ),
            None,
            &BTreeSet::new(),
            &[],
        )
        .unwrap();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].kind, FindingKind::Outside);
        assert_eq!(
            findings[0].message,
            "#1472: pv-1.7.24-GCCcore-12.3.0.eb in eessi-2025.06-eb-5.2.1-2025b.yml is outside EESSI 2025.06: GCCcore-12.3.0 is generation 2023a, which EESSI 2023.06 carries; 2025.06 carries 2024a, 2025a, 2025b"
        );
    }

    #[test]
    fn a_second_version_needs_an_allow_and_an_old_duplicate_does_not() {
        let table = GenerationTable::bundled().unwrap();
        let path = "easystacks/software.eessi.io/2025.06/eessi-2025.06-eb-5.3.0-2025b.yml";
        let existing: BTreeSet<String> = [
            "CMake-3.31.8-GCCcore-14.3.0.eb",
            "CMake-4.0.3-GCCcore-14.3.0.eb",
        ]
        .map(String::from)
        .into();
        let gromacs = plan(
            None,
            &[(
                path,
                &["GROMACS-2026.2-foss-2025b.eb"],
                &["GROMACS-2025.4-foss-2025b.eb"],
            )],
        );
        let findings = lint_plan(&table, &gromacs, None, &existing, &[]).unwrap();
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(
            findings[0].message,
            "GROMACS has 2 versions in generation 2025b: 2025.4 (already in the stack), 2026.2 (added: GROMACS-2026.2-foss-2025b.eb); pass --allow-versions GROMACS@2025b to accept"
        );
        for spec in ["GROMACS@2025b", "GROMACS"] {
            let allow = [VersionAllow::parse(spec).unwrap()];
            assert!(lint_plan(&table, &gromacs, None, &existing, &allow)
                .unwrap()
                .is_empty());
        }
        let elsewhere = [VersionAllow::parse("GROMACS@2025a").unwrap()];
        assert_eq!(
            lint_plan(&table, &gromacs, None, &existing, &elsewhere)
                .unwrap()
                .len(),
            1
        );
        let suffix_only = plan(
            None,
            &[(
                path,
                &["GROMACS-2025.4-foss-2025b-CUDA-12.9.1.eb"],
                &["GROMACS-2025.4-foss-2025b.eb"],
            )],
        );
        assert!(lint_plan(&table, &suffix_only, None, &existing, &[])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_path_without_a_version_needs_the_flag() {
        let table = GenerationTable::bundled().unwrap();
        let p = plan(
            None,
            &[("stack.yml", &["zlib-1.3.1-GCCcore-14.3.0.eb"], &[])],
        );
        let err = lint_plan(&table, &p, None, &BTreeSet::new(), &[]).unwrap_err();
        assert!(err.to_string().contains("pass --eessi-version"), "{err}");
        assert!(
            lint_plan(&table, &p, Some("2025.06"), &BTreeSet::new(), &[])
                .unwrap()
                .is_empty()
        );
        let err = lint_plan(&table, &p, Some("2024.06"), &BTreeSet::new(), &[]).unwrap_err();
        assert!(
            err.to_string().contains("not in the generation table"),
            "{err}"
        );
    }

    #[test]
    fn allow_specs_parse() {
        assert!(VersionAllow::parse("@2025b").is_err());
        assert!(VersionAllow::parse("CMake@").is_err());
        assert_eq!(
            VersionAllow::parse("CMake@2025b")
                .unwrap()
                .generation
                .as_deref(),
            Some("2025b")
        );
    }
}
