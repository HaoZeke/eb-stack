//! Bot jobs a plan entry is likely to run out of memory in.
//!
//! `data/build-memory.toml` records how much memory a build has needed, per
//! EasyBuild name and version family, from EESSI bot jobs that ran out and
//! jobs that did not. `data/eessi-bot-resources.toml` records the cores and
//! memory a bot job gets on each instance and CPU target, and the
//! `PARALLELISM_LIMITS` the EESSI hooks apply. Together they estimate the
//! peak of each build in a plan on each target. An estimate that reaches
//! the job's memory is a warning, and the warning names the remedies the
//! EESSI maintainers have used: a lower parallel level, or a node with more
//! memory. Nothing is changed; the remedies are text.

use crate::arch_support::{plan_requirers, split_easyconfig_filename};
use crate::build_order::{closure_against_installed, reverse_uses, Choice, ModuleKey};
use crate::cli_parse::usage;
use crate::domain::Candidate;
use crate::easystack::parse_easystack_entries;
use crate::eb_parse::parse_easyconfig_trees;
use crate::pr_overlap::parse_installed;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The peak-memory table shipped with eb-stack.
pub const BUNDLED_BUILD_MEMORY: &str = include_str!("../data/build-memory.toml");

/// The bot job table shipped with eb-stack.
pub const BUNDLED_BOT_RESOURCES: &str = include_str!("../data/eessi-bot-resources.toml");

/// One recorded build memory figure.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEntry {
    /// EasyBuild name, `libxc`.
    pub name: String,
    /// Version families, `7.0`. Empty covers every version.
    pub versions: Vec<String>,
    /// Toolchain families as the module names them, `GCC-15.2`. Empty covers all.
    pub toolchains: Vec<String>,
    /// CPU targets or target prefixes, `x86_64/amd/zen4`. Empty covers all.
    pub targets: Vec<String>,
    /// GiB each parallel build process needs.
    pub per_process_gib: f64,
    /// GiB the build needs whatever the parallel level.
    pub base_gib: f64,
    /// The figures are the least the failures allow.
    pub lower_bound: bool,
    /// What uses the memory.
    pub reason: String,
    /// What the maintainers did about it, when the thread says.
    pub remedy: Option<String>,
    /// Threads the figures come from.
    pub sources: Vec<String>,
    /// Bot jobs the figures have to reproduce.
    pub observations: Vec<Observation>,
}

/// One bot job that ran out of memory, or did not.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// Easyconfig filename the job built.
    pub easyconfig: String,
    /// CPU target.
    pub target: String,
    /// Parallel level the build ran with. Absent when the thread does not
    /// say, which only a build with no per-process figure allows.
    pub parallel: Option<u32>,
    /// GiB the job had.
    pub memory_gib: f64,
    /// The job ran out of memory.
    pub oom: bool,
    /// The comment that reports it.
    pub source: String,
}

impl MemoryEntry {
    /// Estimated peak GiB at `parallel`.
    pub fn peak_gib(&self, parallel: u32) -> f64 {
        self.base_gib + self.per_process_gib * f64::from(parallel)
    }

    fn covers(&self, build: &BuildId, target: &str) -> bool {
        self.name == build.name
            && (self.versions.is_empty()
                || self
                    .versions
                    .iter()
                    .any(|family| in_family(&build.version, family, &['.', '-'])))
            && (self.toolchains.is_empty()
                || self
                    .toolchains
                    .iter()
                    .any(|family| in_family(&build.toolchain, family, &['.', '-'])))
            && (self.targets.is_empty()
                || self
                    .targets
                    .iter()
                    .any(|prefix| in_family(target, prefix, &['/'])))
    }
}

fn in_family(value: &str, family: &str, separators: &[char]) -> bool {
    value == family
        || value
            .strip_prefix(family)
            .is_some_and(|rest| rest.starts_with(separators))
}

/// The peak-memory table.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryProfile {
    entries: Vec<MemoryEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    #[serde(default)]
    entry: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    name: String,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    toolchains: Vec<String>,
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    per_process_gib: f64,
    #[serde(default)]
    base_gib: f64,
    bound: Option<String>,
    reason: String,
    remedy: Option<String>,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    observation: Vec<RawObservation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawObservation {
    easyconfig: String,
    target: String,
    parallel: Option<u32>,
    memory_gib: f64,
    outcome: String,
    source: String,
}

impl MemoryProfile {
    /// The table shipped in `data/build-memory.toml`.
    pub fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_BUILD_MEMORY).context("data/build-memory.toml")
    }

    /// Read a table from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Parse the TOML document. A table whose figures contradict one of its
    /// own observations is refused.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawProfile =
            toml::from_str(text).context("build memory table is not valid TOML")?;
        let mut entries = Vec::new();
        for entry in raw.entry {
            let name = entry.name.trim().to_string();
            let reason = entry.reason.trim().to_string();
            if name.is_empty() || reason.is_empty() {
                bail!("a build memory entry needs a name and a reason");
            }
            let finite = entry.per_process_gib.is_finite() && entry.base_gib.is_finite();
            if !finite || entry.per_process_gib < 0.0 || entry.base_gib < 0.0 {
                bail!("{name}: per_process_gib and base_gib cannot be negative");
            }
            if entry.per_process_gib == 0.0 && entry.base_gib == 0.0 {
                bail!("{name}: set per_process_gib, base_gib, or both");
            }
            let lower_bound = match entry.bound.as_deref() {
                None => false,
                Some("lower") => true,
                Some(other) => bail!("{name}: bound is \"lower\" or absent, not {other:?}"),
            };
            let mut observations = Vec::new();
            for observation in entry.observation {
                let oom = match observation.outcome.as_str() {
                    "oom" => true,
                    "ok" => false,
                    other => {
                        bail!("{name}: observation outcome is \"oom\" or \"ok\", not {other:?}")
                    }
                };
                if observation.parallel.is_none() && entry.per_process_gib > 0.0 {
                    bail!(
                        "{name}: the observation of {} on {} needs parallel, because the entry has a per-process figure",
                        observation.easyconfig,
                        observation.target
                    );
                }
                if observation.parallel == Some(0) || observation.memory_gib <= 0.0 {
                    bail!("{name}: observation parallel and memory_gib must be positive");
                }
                observations.push(Observation {
                    easyconfig: observation.easyconfig,
                    target: observation.target,
                    parallel: observation.parallel,
                    memory_gib: observation.memory_gib,
                    oom,
                    source: observation.source,
                });
            }
            entries.push(MemoryEntry {
                name,
                versions: entry.versions,
                toolchains: entry.toolchains,
                targets: entry.targets,
                per_process_gib: entry.per_process_gib,
                base_gib: entry.base_gib,
                lower_bound,
                reason,
                remedy: entry.remedy,
                sources: entry.sources,
                observations,
            });
        }
        let profile = Self { entries };
        let contradictions = profile.contradictions();
        if !contradictions.is_empty() {
            bail!(
                "the build memory table contradicts its observations:\n{}",
                contradictions.join("\n")
            );
        }
        Ok(profile)
    }

    /// Every entry, in file order.
    pub fn entries(&self) -> &[MemoryEntry] {
        &self.entries
    }

    /// The entry with the largest estimate for `build` on `target` at
    /// `parallel`, and that estimate.
    fn estimate(
        &self,
        build: &BuildId,
        target: &str,
        parallel: u32,
    ) -> Option<(&MemoryEntry, f64)> {
        self.entries
            .iter()
            .filter(|entry| entry.covers(build, target))
            .map(|entry| (entry, entry.peak_gib(parallel)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Observations the table does not reproduce, one line each.
    pub fn contradictions(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for entry in &self.entries {
            for observation in &entry.observations {
                let Some(build) = BuildId::from_filename(&observation.easyconfig) else {
                    lines.push(format!(
                        "{}: observation easyconfig {} is not an easyconfig filename",
                        entry.name, observation.easyconfig
                    ));
                    continue;
                };
                let parallel = observation.parallel.unwrap_or(1);
                let peak = self
                    .estimate(&build, &observation.target, parallel)
                    .map_or(0.0, |(_, peak)| peak);
                let predicted = peak >= observation.memory_gib;
                if predicted != observation.oom {
                    lines.push(format!(
                        "{} on {} at parallel {}: estimate {:.1} GiB against {:.1} GiB, but the job {} ({})",
                        observation.easyconfig,
                        observation.target,
                        parallel,
                        peak,
                        observation.memory_gib,
                        if observation.oom { "ran out" } else { "fitted" },
                        observation.source
                    ));
                }
            }
        }
        lines
    }
}

/// One bot node type.
#[derive(Debug, Clone, PartialEq)]
pub struct Partition {
    /// Bot instance, `eessi-bot-mc-aws`.
    pub instance: String,
    /// What the bot prints as "Building on", `amd-zen4`.
    pub node: String,
    /// What the bot prints as "Building for", `x86_64/amd/zen4`.
    pub target: String,
    /// Accelerator the node carries, `nvidia/cc90`.
    pub accel: Option<String>,
    /// CPUs a job holds.
    pub cores: u32,
    /// GiB a build in the job can use.
    pub memory_gib: f64,
    /// Where the figures come from.
    pub sources: Vec<String>,
}

impl Partition {
    fn label(&self) -> String {
        format!("{} {}", self.instance, self.node)
    }
}

/// How a hook limit lowers parallelism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitOp {
    /// `divide_by_factor`: the cores divided by this, at least 1.
    Divide(u32),
    /// `set_maximum`: no more than this.
    Maximum(u32),
}

impl LimitOp {
    fn apply(self, cores: u32) -> u32 {
        match self {
            Self::Divide(factor) => (cores / factor).max(1),
            Self::Maximum(value) => cores.min(value),
        }
    }
}

/// One `PARALLELISM_LIMITS` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookLimit {
    /// EasyBuild name. `None` is the target's own limit.
    pub software: Option<String>,
    /// CPU target, or `*` for every target.
    pub target: String,
    /// The limit.
    pub op: LimitOp,
}

/// The bot job table and the EESSI hook limits.
#[derive(Debug, Clone, PartialEq)]
pub struct BotResources {
    /// EasyBuild's default `max-parallel`.
    pub max_parallel: u32,
    /// Node types, in file order.
    pub partitions: Vec<Partition>,
    /// Hook limits, in file order.
    pub limits: Vec<HookLimit>,
    /// The `eb_hooks.py` the limits were copied from.
    pub hooks_source: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawResources {
    easybuild: RawEasyBuild,
    #[serde(default)]
    partition: Vec<RawPartition>,
    #[serde(default)]
    hooks: RawHooks,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEasyBuild {
    max_parallel: u32,
    #[allow(dead_code)]
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPartition {
    instance: String,
    node: String,
    target: String,
    accel: Option<String>,
    cores: u32,
    memory_gib: f64,
    #[serde(default)]
    sources: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHooks {
    #[serde(default)]
    source: String,
    #[serde(default)]
    limit: Vec<RawLimit>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLimit {
    software: Option<String>,
    target: String,
    divide: Option<u32>,
    maximum: Option<u32>,
}

/// The parallel level a build gets in one job, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parallel {
    /// The level EasyBuild runs with.
    pub level: u32,
    /// CPUs the job holds.
    pub cores: u32,
    /// The hook limit that applied.
    pub hook: Option<LimitOp>,
    /// `maxparallel` in the easyconfig.
    pub recipe: Option<u32>,
}

impl BotResources {
    /// The table shipped in `data/eessi-bot-resources.toml`.
    pub fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_BOT_RESOURCES).context("data/eessi-bot-resources.toml")
    }

    /// Read a table from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Parse the TOML document.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawResources =
            toml::from_str(text).context("bot resource table is not valid TOML")?;
        if raw.easybuild.max_parallel == 0 {
            bail!("easybuild.max_parallel must be positive");
        }
        let mut partitions = Vec::new();
        let mut seen = BTreeSet::new();
        for partition in raw.partition {
            if partition.cores == 0
                || !partition.memory_gib.is_finite()
                || partition.memory_gib <= 0.0
            {
                bail!(
                    "{} {}: cores and memory_gib must be positive",
                    partition.instance,
                    partition.node
                );
            }
            let key = (
                partition.instance.clone(),
                partition.node.clone(),
                partition.target.clone(),
            );
            if !seen.insert(key) {
                bail!(
                    "bot resource table repeats {} {} for {}",
                    partition.instance,
                    partition.node,
                    partition.target
                );
            }
            partitions.push(Partition {
                instance: partition.instance,
                node: partition.node,
                target: partition.target,
                accel: partition.accel,
                cores: partition.cores,
                memory_gib: partition.memory_gib,
                sources: partition.sources,
            });
        }
        let mut limits = Vec::new();
        for limit in raw.hooks.limit {
            let op = match (limit.divide, limit.maximum) {
                (Some(factor), None) if factor > 0 => LimitOp::Divide(factor),
                (None, Some(value)) if value > 0 => LimitOp::Maximum(value),
                _ => bail!(
                    "hook limit for {} on {} needs one positive divide or maximum",
                    limit.software.as_deref().unwrap_or("every build"),
                    limit.target
                ),
            };
            if limit.software.is_none() && limit.target == "*" {
                bail!("a hook limit without software needs a target");
            }
            limits.push(HookLimit {
                software: limit.software,
                target: limit.target,
                op,
            });
        }
        Ok(Self {
            max_parallel: raw.easybuild.max_parallel,
            partitions,
            limits,
            hooks_source: raw.hooks.source,
        })
    }

    /// The hook limit for `name` on `target`, as `post_ready_hook` picks it.
    ///
    /// A software entry decides on its own: its limit for the target, else
    /// its `*` limit, else none. Only software without an entry gets the
    /// target's own limit.
    pub fn hook_limit(&self, name: &str, target: &str) -> Option<LimitOp> {
        let software: Vec<&HookLimit> = self
            .limits
            .iter()
            .filter(|limit| limit.software.as_deref() == Some(name))
            .collect();
        if !software.is_empty() {
            return software
                .iter()
                .find(|limit| limit.target == target)
                .or_else(|| software.iter().find(|limit| limit.target == "*"))
                .map(|limit| limit.op);
        }
        self.limits
            .iter()
            .find(|limit| limit.software.is_none() && limit.target == target)
            .map(|limit| limit.op)
    }

    /// The parallel level `name` builds with on `partition`.
    pub fn parallel(&self, name: &str, partition: &Partition, recipe: Option<u32>) -> Parallel {
        let mut level = partition.cores.min(self.max_parallel);
        if let Some(recipe) = recipe {
            level = level.min(recipe.max(1));
        }
        let hook = self.hook_limit(name, &partition.target);
        if let Some(op) = hook {
            level = level.min(op.apply(partition.cores));
        }
        Parallel {
            level,
            cores: partition.cores,
            hook,
            recipe,
        }
    }

    fn select(&self, arches: &[String]) -> (Vec<&Partition>, Vec<String>) {
        if arches.is_empty() {
            return (self.partitions.iter().collect(), Vec::new());
        }
        let mut chosen = Vec::new();
        let mut unknown = Vec::new();
        for arch in arches {
            let matched: Vec<&Partition> = self
                .partitions
                .iter()
                .filter(|partition| in_family(&partition.target, arch, &['/']))
                .collect();
            if matched.is_empty() {
                unknown.push(arch.clone());
            }
            for partition in matched {
                if !chosen
                    .iter()
                    .any(|seen: &&Partition| std::ptr::eq(*seen, partition))
                {
                    chosen.push(partition);
                }
            }
        }
        (chosen, unknown)
    }
}

/// Name, version, and toolchain of one build.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildId {
    name: String,
    version: String,
    toolchain: String,
}

impl BuildId {
    fn from_filename(filename: &str) -> Option<Self> {
        let (name, versionfull) = split_easyconfig_filename(filename)?;
        let (version, toolchain) = match versionfull.split_once('-') {
            Some((version, rest)) => (version.to_string(), rest.to_string()),
            None => (versionfull, "system".to_string()),
        };
        Some(Self {
            name,
            version,
            toolchain,
        })
    }

    fn from_key(key: &ModuleKey) -> Self {
        Self {
            name: key.name.clone(),
            version: key.version.clone(),
            toolchain: key.toolchain.clone(),
        }
    }
}

/// One plan entry the check looks at.
#[derive(Debug, Clone)]
struct PlanBuild {
    file: String,
    id: BuildId,
    recipe_maxparallel: Option<u32>,
    required_by: BTreeSet<String>,
}

/// `maxparallel = N` at the top level of an easyconfig.
pub fn recipe_maxparallel(text: &str) -> Option<u32> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("maxparallel")?;
        let value = rest.trim_start().strip_prefix('=')?;
        let value = value.split('#').next().unwrap_or("").trim();
        value.parse().ok()
    })
}

/// One build likely to run out of memory in one bot job.
#[derive(Debug, Clone, PartialEq)]
pub struct OomWarning {
    /// Easyconfig filename.
    pub file: String,
    /// The bot node type.
    pub partition: Partition,
    /// The parallel level the build would get.
    pub parallel: Parallel,
    /// Estimated peak GiB.
    pub peak_gib: f64,
    /// The memory table entry the estimate came from.
    pub entry: MemoryEntry,
    /// Plan entries that need `file`. Empty when the plan names it.
    pub required_by: BTreeSet<String>,
    /// Remedies, one line each.
    pub remedies: Vec<String>,
}

impl OomWarning {
    /// The first line the command prints for this warning.
    pub fn message(&self) -> String {
        let mut line = format!(
            "{} may run out of memory on {} ({}): {}, the job has {:.1} GiB",
            self.file,
            self.partition.target,
            self.partition.label(),
            estimate_text(&self.entry, self.parallel.level, self.peak_gib),
            self.partition.memory_gib
        );
        let cites = cite_all(&self.entry.sources);
        if !cites.is_empty() {
            line.push_str(&format!(" ({})", cites.join(", ")));
        }
        if !self.required_by.is_empty() {
            line.push_str(&format!(
                " (required by {})",
                self.required_by
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        line
    }

    /// Every line the command prints for this warning.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.message()];
        lines.push(format!(
            "  parallel {}: {}",
            self.parallel.level,
            parallel_reason(&self.entry.name, &self.parallel)
        ));
        lines.push(format!("  why: {}", self.entry.reason));
        for remedy in &self.remedies {
            lines.push(format!("  remedy: {remedy}"));
        }
        lines
    }
}

fn estimate_text(entry: &MemoryEntry, parallel: u32, peak: f64) -> String {
    if entry.per_process_gib == 0.0 {
        return format!("{:.1} GiB at any parallel level", entry.base_gib);
    }
    let processes = format!("{parallel} x {:.1} GiB", entry.per_process_gib);
    if entry.base_gib == 0.0 {
        format!("{processes} = {peak:.1} GiB")
    } else {
        format!("{:.1} GiB + {processes} = {peak:.1} GiB", entry.base_gib)
    }
}

fn parallel_reason(name: &str, parallel: &Parallel) -> String {
    let mut parts = vec![format!("{} cores", parallel.cores)];
    if let Some(recipe) = parallel.recipe {
        parts.push(format!("maxparallel = {recipe} in the easyconfig"));
    }
    match parallel.hook {
        Some(LimitOp::Divide(factor)) => {
            parts.push(format!("eb_hooks.py divides {name} by {factor}"));
        }
        Some(LimitOp::Maximum(value)) => {
            parts.push(format!("eb_hooks.py caps {name} at {value}"));
        }
        None => parts.push(format!("no eb_hooks.py limit for {name}")),
    }
    parts.join(", ")
}

fn cite(source: &str) -> Option<String> {
    let rest = source.split("github.com/").nth(1)?;
    let mut parts = rest.split('/');
    let _owner = parts.next()?;
    let repo = parts.next()?;
    let kind = parts.next()?;
    if kind != "pull" && kind != "issues" {
        return None;
    }
    let number: String = parts
        .next()?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!number.is_empty()).then(|| format!("{repo} #{number}"))
}

fn cite_all(sources: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for source in sources {
        if let Some(cite) = cite(source) {
            if !out.contains(&cite) {
                out.push(cite);
            }
        }
    }
    out
}

/// The parallel level below `level` at which `entry` fits in `memory_gib`.
fn fitting_parallel(entry: &MemoryEntry, level: u32, memory_gib: f64) -> Option<u32> {
    (1..level)
        .rev()
        .find(|&parallel| entry.peak_gib(parallel) < memory_gib)
}

fn remedies(
    resources: &BotResources,
    profile: &MemoryProfile,
    build: &PlanBuild,
    partition: &Partition,
    entry: &MemoryEntry,
    parallel: &Parallel,
) -> Vec<String> {
    let mut lines = Vec::new();
    match fitting_parallel(entry, parallel.level, partition.memory_gib) {
        Some(fits) => {
            let mut line = format!(
                "maxparallel = {fits} in {}, or a {} limit of {fits} for {} in PARALLELISM_LIMITS in software-layer-scripts eb_hooks.py",
                build.file, entry.name, partition.target
            );
            if entry.lower_bound {
                line.push_str(&format!(
                    "; {:.1} GiB per process is a lower bound, so {fits} may still be too many",
                    entry.per_process_gib
                ));
            }
            lines.push(line);
        }
        None => lines.push(format!(
            "a lower parallel level does not help: one process already needs {:.1} GiB",
            entry.peak_gib(1)
        )),
    }
    let mut larger = Vec::new();
    for other in &resources.partitions {
        if other.target != partition.target || std::ptr::eq(other, partition) {
            continue;
        }
        let other_parallel = resources.parallel(&entry.name, other, build.recipe_maxparallel);
        let peak = profile
            .estimate(&build.id, &other.target, other_parallel.level)
            .map_or(0.0, |(_, peak)| peak);
        if peak < other.memory_gib {
            let accel = other
                .accel
                .as_deref()
                .map(|accel| format!(", accel {accel}"))
                .unwrap_or_default();
            larger.push(format!(
                "{} ({} cores, {:.1} GiB{accel})",
                other.label(),
                other.cores,
                other.memory_gib
            ));
        }
    }
    if larger.is_empty() {
        lines.push(format!(
            "no other {} node in the bot resource table has room for it",
            partition.target
        ));
    } else {
        lines.push(format!(
            "build {} on a node with more memory: {}",
            partition.target,
            larger.join("; ")
        ));
    }
    if let Some(remedy) = &entry.remedy {
        lines.push(remedy.clone());
    }
    lines
}

/// Arguments for `stack oom`.
pub struct OomCheckRequest {
    /// CPU targets or target prefixes. Empty checks every target in the
    /// bot resource table.
    pub arches: Vec<String>,
    /// Easystacks whose entries are the plan. Repeatable.
    pub easystacks: Vec<PathBuf>,
    /// Robot trees the dependency closure is read from. Empty checks only
    /// the filenames the easystacks name.
    pub easyconfigs: Vec<PathBuf>,
    /// Installed modules, one `Name/VersionFull` per line. These are not
    /// built, so they are not checked.
    pub installed: Option<PathBuf>,
    /// Override `data/build-memory.toml`.
    pub profile: Option<PathBuf>,
    /// Override `data/eessi-bot-resources.toml`.
    pub resources: Option<PathBuf>,
}

/// What `stack oom` prints.
pub struct OomCheckReport {
    /// Stdout, including the count line.
    pub stdout: String,
    /// Builds likely to run out of memory.
    pub warnings: Vec<OomWarning>,
    /// Build and node pairs with a memory figure that fit.
    pub fits: usize,
    /// Builds checked.
    pub builds: usize,
    /// Node types checked.
    pub partitions: usize,
    /// `--arch` values the bot resource table has no node for.
    pub unknown_arches: Vec<String>,
}

/// Estimate each plan build's peak on each bot node and warn where it
/// reaches the job's memory.
pub fn run_oom_check(request: &OomCheckRequest) -> Result<OomCheckReport> {
    if request.easystacks.is_empty() {
        return Err(usage("pass at least one --easystack"));
    }
    let profile = match &request.profile {
        Some(path) => MemoryProfile::load(path)?,
        None => MemoryProfile::bundled()?,
    };
    let resources = match &request.resources {
        Some(path) => BotResources::load(path)?,
        None => BotResources::bundled()?,
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
    let installed = match &request.installed {
        Some(path) => parse_installed(
            &std::fs::read_to_string(path)
                .with_context(|| format!("--installed {}", path.display()))?,
        ),
        None => BTreeSet::new(),
    };

    let candidates = if request.easyconfigs.is_empty() {
        Vec::new()
    } else {
        let trees: Vec<&Path> = request.easyconfigs.iter().map(PathBuf::as_path).collect();
        parse_easyconfig_trees(&trees)
            .map_err(|error| {
                anyhow::Error::msg(error).context(format!(
                    "--easyconfigs {}",
                    request
                        .easyconfigs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?
            .candidates
    };
    let builds = plan_builds(&candidates, &starts, &installed);
    let (partitions, unknown_arches) = resources.select(&request.arches);

    let mut warnings = Vec::new();
    let mut fits = 0;
    for build in &builds {
        for partition in &partitions {
            let parallel = resources.parallel(&build.id.name, partition, build.recipe_maxparallel);
            let Some((entry, peak)) =
                profile.estimate(&build.id, &partition.target, parallel.level)
            else {
                continue;
            };
            if peak < partition.memory_gib {
                fits += 1;
                continue;
            }
            let remedies = remedies(&resources, &profile, build, partition, entry, &parallel);
            warnings.push(OomWarning {
                file: build.file.clone(),
                partition: (*partition).clone(),
                parallel,
                peak_gib: peak,
                entry: entry.clone(),
                required_by: build.required_by.clone(),
                remedies,
            });
        }
    }

    let mut stdout = String::new();
    for arch in &unknown_arches {
        stdout.push_str(&format!(
            "{arch} has no bot node in the bot resource table, so it is not checked\n"
        ));
    }
    for warning in &warnings {
        for line in warning.lines() {
            stdout.push_str(&line);
            stdout.push('\n');
        }
    }
    stdout.push_str(&format!(
        "oom={} fits={} builds={} nodes={}\n",
        warnings.len(),
        fits,
        builds.len(),
        partitions.len()
    ));
    Ok(OomCheckReport {
        stdout,
        warnings,
        fits,
        builds: builds.len(),
        partitions: partitions.len(),
        unknown_arches,
    })
}

fn basename(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

fn module_string(key: &ModuleKey) -> String {
    if key.toolchain == "system" {
        format!("{}/{}{}", key.name, key.version, key.versionsuffix)
    } else {
        format!(
            "{}/{}-{}{}",
            key.name, key.version, key.toolchain, key.versionsuffix
        )
    }
}

/// The builds a plan makes: the easystack entries, and with candidates
/// their dependency closure, less what `installed` already has.
fn plan_builds(
    candidates: &[Candidate],
    starts: &[String],
    installed: &BTreeSet<String>,
) -> Vec<PlanBuild> {
    let by_file: BTreeMap<String, &Candidate> = candidates
        .iter()
        .map(|candidate| (basename(&candidate.easyconfig_path), candidate))
        .collect();
    let mut builds: BTreeMap<String, PlanBuild> = BTreeMap::new();
    let mut start_keys = Vec::new();
    let mut start_files = BTreeSet::new();
    for start in starts {
        let file = basename(start);
        if !start_files.insert(file.clone()) {
            continue;
        }
        if let Some(candidate) = by_file.get(&file) {
            start_keys.push(ModuleKey::of(candidate));
            continue;
        }
        let Some(id) = BuildId::from_filename(&file) else {
            continue;
        };
        let module = crate::arch_support::module_from_filename(&file).unwrap_or_default();
        if installed.contains(&module) {
            continue;
        }
        builds.insert(
            file.clone(),
            PlanBuild {
                file,
                id,
                recipe_maxparallel: None,
                required_by: BTreeSet::new(),
            },
        );
    }
    if start_keys.is_empty() {
        return builds.into_values().collect();
    }
    let is_installed = |key: &ModuleKey| installed.contains(&module_string(key));
    let closure = closure_against_installed(candidates, &start_keys, Choice::Newest, &is_installed);
    let in_closure: BTreeSet<ModuleKey> = closure.to_build.iter().cloned().collect();
    let start_set: BTreeSet<ModuleKey> = start_keys.iter().cloned().collect();
    let uses = reverse_uses(candidates, Choice::Newest);
    let by_key: BTreeMap<ModuleKey, &Candidate> = candidates
        .iter()
        .map(|candidate| (ModuleKey::of(candidate), candidate))
        .collect();
    for key in &closure.to_build {
        let Some(candidate) = by_key.get(key) else {
            continue;
        };
        let file = basename(&candidate.easyconfig_path);
        let required_by = if start_set.contains(key) {
            BTreeSet::new()
        } else {
            plan_requirers(key, &start_set, &in_closure, &uses)
                .iter()
                .filter_map(|requirer| by_key.get(requirer))
                .map(|candidate| basename(&candidate.easyconfig_path))
                .collect()
        };
        let recipe_maxparallel = std::fs::read_to_string(&candidate.easyconfig_path)
            .ok()
            .and_then(|text| recipe_maxparallel(&text));
        builds.insert(
            file.clone(),
            PlanBuild {
                file,
                id: BuildId::from_key(key),
                recipe_maxparallel,
                required_by,
            },
        );
    }
    builds.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zen4(resources: &BotResources) -> &Partition {
        resources
            .partitions
            .iter()
            .find(|p| p.instance == "eessi-bot-mc-aws" && p.target == "x86_64/amd/zen4")
            .expect("mc-aws zen4")
    }

    #[test]
    fn bundled_tables_parse_and_reproduce_every_observation() {
        let profile = MemoryProfile::bundled().expect("bundled build memory");
        assert!(profile.contradictions().is_empty());
        let observed: usize = profile.entries().iter().map(|e| e.observations.len()).sum();
        assert!(observed >= 10, "{observed}");
        for entry in profile.entries() {
            assert!(!entry.sources.is_empty(), "{} has no source", entry.name);
            for observation in &entry.observations {
                assert!(
                    observation.source.contains("github.com/EESSI/"),
                    "{}",
                    observation.source
                );
            }
        }
        let resources = BotResources::bundled().expect("bundled bot resources");
        for partition in &resources.partitions {
            assert!(
                !partition.sources.is_empty(),
                "{} has no source",
                partition.label()
            );
        }
        assert!(resources.hooks_source.contains("608fb40c"));
    }

    #[test]
    fn hook_limits_follow_post_ready_hook() {
        let resources = BotResources::bundled().unwrap();
        let a64fx = resources
            .partitions
            .iter()
            .find(|p| p.target == "aarch64/a64fx")
            .unwrap();
        let zen4 = zen4(&resources);
        assert_eq!(resources.parallel("libxc", zen4, None).level, 8);
        assert_eq!(resources.parallel("libxc", a64fx, None).level, 6);
        assert_eq!(resources.parallel("zlib", a64fx, None).level, 12);
        assert_eq!(resources.parallel("zlib", zen4, None).level, 16);
        assert_eq!(resources.parallel("MBX", a64fx, None).level, 1);
        // QuantumESPRESSO has an A64FX entry only, so zen4 is left alone.
        assert_eq!(resources.parallel("QuantumESPRESSO", zen4, None).level, 16);
        assert_eq!(resources.parallel("libxc", zen4, Some(4)).level, 4);
    }

    #[test]
    fn version_toolchain_and_target_families_have_boundaries() {
        assert!(in_family("7.0.0", "7.0", &['.', '-']));
        assert!(in_family("7.0", "7.0", &['.', '-']));
        assert!(!in_family("7.01", "7.0", &['.', '-']));
        assert!(in_family("GCC-15.2.0", "GCC-15.2", &['.', '-']));
        assert!(!in_family("GCCcore-15.2.0", "GCC-15.2", &['.', '-']));
        assert!(in_family("x86_64/amd/zen4", "x86_64", &['/']));
        assert!(!in_family("x86_64/amd/zen4", "x86_64/amd/zen", &['/']));
        let id =
            BuildId::from_filename("hipBLASLt-0.12.1-rocm-compilers-19.0.0-ROCm-6.4.1.eb").unwrap();
        assert_eq!(id.name, "hipBLASLt");
        assert_eq!(id.version, "0.12.1");
        assert_eq!(id.toolchain, "rocm-compilers-19.0.0-ROCm-6.4.1");
    }

    #[test]
    fn a_table_that_contradicts_its_own_observation_is_refused() {
        let text = r#"
[[entry]]
name = "libxc"
versions = ["7.0"]
per_process_gib = 1.0
reason = "cc1"
sources = ["https://github.com/EESSI/software-layer/pull/1772"]

[[entry.observation]]
easyconfig = "libxc-7.0.0-GCC-15.2.0.eb"
target = "x86_64/amd/zen4"
parallel = 8
memory_gib = 32.0
outcome = "oom"
source = "https://github.com/EESSI/software-layer/pull/1772#issuecomment-6420263033"
"#;
        let error = MemoryProfile::parse(text).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("contradicts its observations"), "{text}");
        assert!(text.contains("estimate 8.0 GiB against 32.0 GiB"), "{text}");
    }

    #[test]
    fn maxparallel_is_read_from_the_recipe_top_level() {
        assert_eq!(recipe_maxparallel("name = 'x'\nmaxparallel = 6\n"), Some(6));
        assert_eq!(recipe_maxparallel("maxparallel=2  # tests\n"), Some(2));
        assert_eq!(recipe_maxparallel("    maxparallel = 6\n"), None);
        assert_eq!(recipe_maxparallel("name = 'x'\n"), None);
    }

    #[test]
    fn a_remedy_names_the_largest_parallel_level_that_fits() {
        let profile = MemoryProfile::bundled().unwrap();
        let entry = profile
            .entries()
            .iter()
            .find(|e| e.name == "libxc" && !e.toolchains.is_empty())
            .unwrap();
        assert_eq!(fitting_parallel(entry, 8, 32.0), Some(7));
        let hip = profile
            .entries()
            .iter()
            .find(|e| e.name == "hipBLASLt")
            .unwrap();
        assert_eq!(fitting_parallel(hip, 8, 27.9), None);
    }

    #[test]
    fn citations_keep_the_repository() {
        assert_eq!(
            cite("https://github.com/EESSI/software-layer/pull/1772#issuecomment-6420263033")
                .as_deref(),
            Some("software-layer #1772")
        );
        assert_eq!(
            cite("https://github.com/EESSI/software-layer-scripts/issues/70").as_deref(),
            Some("software-layer-scripts #70")
        );
        assert_eq!(cite("https://example.org/x"), None);
    }
}
