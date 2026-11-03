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
//!
//! The hook ops are `divide_by_factor` and `set_maximum` from
//! `eb_hooks.py`. A divide factor that is not an integer makes Python's
//! `//` return a float (`16 // 2.5` is `6.0`); the command warns and uses
//! the numeric floor for the estimate. `--hooks` reads the rows from an
//! `eb_hooks.py` instead of the table, through [`crate::eb_hooks`].
//! `data/job-memory.toml` holds `mib_per_job` for `limit_by_memory`. A row
//! with no number is unmeasured. `--node` prints the hook parallelism
//! beside that rule.

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

/// The per-package `mib_per_job` table shipped with eb-stack.
pub const BUNDLED_JOB_MEMORY: &str = include_str!("../data/job-memory.toml");

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
    /// `divide_by_factor`: `max(1, parallel // factor)`.
    Divide(DivideFactor),
    /// `set_maximum`: `min(parallel, maximum)`.
    Maximum(u32),
}

/// A `divide_by_factor` argument.
///
/// An integer literal (`divide = 2`) makes Python's `//` return an int.
/// A float (`divide = 2.5`, or `divide = 2.0`) makes `//` return a float:
/// `16 // 2.5` is `6.0` and `16 // 2.0` is `8.0`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DivideFactor {
    bits: u64,
    integer_literal: bool,
}

impl std::fmt::Debug for DivideFactor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DivideFactor")
            .field("value", &self.value())
            .field("integer_literal", &self.integer_literal)
            .finish()
    }
}

impl DivideFactor {
    pub(crate) fn from_integer(value: u32) -> Self {
        Self {
            bits: f64::from(value).to_bits(),
            integer_literal: true,
        }
    }

    pub(crate) fn from_float(value: f64) -> Self {
        Self {
            bits: value.to_bits(),
            integer_literal: false,
        }
    }

    /// The factor as a number.
    pub fn value(self) -> f64 {
        f64::from_bits(self.bits)
    }

    /// The table wrote an integer, `divide = 2`, not a float.
    pub fn integer_literal(self) -> bool {
        self.integer_literal
    }

    fn integer(self) -> Option<u32> {
        self.integer_literal
            .then(|| u32::try_from(self.value() as u64).ok())
            .flatten()
    }
}

/// What `divide_by_factor` returns, and whether Python's result is a float.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DividedParallel {
    /// `max(1, floor(parallel / factor))`.
    pub level: u32,
    /// `max` kept a float. True when the factor is a float and the
    /// quotient is at least 1. Below 1, `max(1, 0.0)` is the int 1.
    pub float_result: bool,
}

/// `divide_by_factor` from `eb_hooks.py`: `max(1, parallel // factor)`.
///
/// An integer factor uses integer division, so the result is an int.
/// A float factor uses floor division and the result is a float when the
/// quotient is at least 1. `level` is that number as a count of jobs.
pub fn divide_by_factor(parallel: u32, factor: DivideFactor) -> DividedParallel {
    let value = factor.value();
    if factor.integer_literal() {
        let divisor = factor.integer().unwrap_or(1).max(1);
        return DividedParallel {
            level: (parallel / divisor).max(1),
            float_result: false,
        };
    }
    if !value.is_finite() || value <= 0.0 {
        return DividedParallel {
            level: 1,
            float_result: false,
        };
    }
    let quotient = (f64::from(parallel) / value).floor();
    if quotient < 1.0 {
        return DividedParallel {
            level: 1,
            float_result: false,
        };
    }
    let level = if quotient >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        quotient as u32
    };
    DividedParallel {
        level,
        float_result: true,
    }
}

/// `set_maximum` from `eb_hooks.py`: `min(parallel, maximum)`.
pub fn set_maximum(parallel: u32, maximum: u32) -> u32 {
    parallel.min(maximum)
}

/// `limit_by_memory(parallel, mib_per_job)` with the memory passed in.
///
/// `max(1, min(parallel, memory_mib // mib_per_job))`. `memory_mib` is
/// what [`job_memory_mib`] reads on the machine a build is running on, or
/// the memory of a node size the report is asked about. `mib_per_job` is
/// the peak RSS of the worst compile. A zero `mib_per_job` is not a
/// budget; the table parser refuses it, and this returns 1.
pub fn limit_by_memory(parallel: u32, mib_per_job: u32, memory_mib: u64) -> u32 {
    if mib_per_job == 0 {
        return 1;
    }
    let by_memory = memory_mib / u64::from(mib_per_job);
    let by_memory = u32::try_from(by_memory).unwrap_or(u32::MAX);
    parallel.min(by_memory).max(1)
}

/// The core cap with a memory ceiling on top:
/// `min(core_cap, memory_mib // mib_per_job)`.
///
/// `core_cap` is the level the hook leaves. Unlike [`limit_by_memory`]
/// the result is not raised to 1: 0 says one job needs more memory than
/// the node has. Flooring the MiB first does not change the quotient,
/// so this is also `memory_bytes // (mib_per_job * 2**20)`. A zero
/// `mib_per_job` is not a budget; the table parser refuses it, and this
/// returns `core_cap`.
pub fn memory_cap(core_cap: u32, mib_per_job: u32, memory_mib: u64) -> u32 {
    let Some(jobs) = memory_mib.checked_div(u64::from(mib_per_job)) else {
        return core_cap;
    };
    core_cap.min(u32::try_from(jobs).unwrap_or(u32::MAX))
}

impl LimitOp {
    fn apply(self, cores: u32) -> u32 {
        match self {
            Self::Divide(factor) => divide_by_factor(cores, factor).level,
            Self::Maximum(value) => set_maximum(cores, value),
        }
    }
}

/// Render a divide factor the way the table wrote it.
pub fn format_factor(factor: DivideFactor) -> String {
    let value = factor.value();
    if factor.integer_literal() {
        return factor.integer().unwrap_or(0).to_string();
    }
    if is_whole(value) {
        return format!("{value:.1}");
    }
    let text = format!("{value}");
    if text.contains('.') {
        text
    } else {
        format!("{value:.1}")
    }
}

fn is_whole(value: f64) -> bool {
    value.is_finite() && value == value.trunc()
}

/// How `16 // factor` comes out in Python, for the warning text.
fn float_example(factor: DivideFactor) -> String {
    let shown = format_factor(factor);
    let raw = if factor.value() > 0.0 {
        (16.0_f64 / factor.value()).floor()
    } else {
        0.0
    };
    let divided = divide_by_factor(16, factor);
    if divided.float_result {
        format!("16 // {shown} is {raw:.1}, a float")
    } else {
        format!("16 // {shown} is {raw:.1}, a float, and max(1, {raw:.1}) is 1, an int")
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
    /// Line of the `(op, argument)` tuple in the `eb_hooks.py` the row was
    /// read from. `None` for a row of the bot resource table.
    pub line: Option<usize>,
}

/// The bot job table and the EESSI hook limits.
#[derive(Debug, Clone, PartialEq)]
pub struct BotResources {
    /// EasyBuild's default `max-parallel`.
    pub max_parallel: u32,
    /// EasyBuild's `--parallel` for the job. `None`, which is what the bot
    /// passes, lets EasyBuild count the cores.
    pub parallel: Option<u32>,
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
    #[serde(default)]
    divide: Option<FactorInput>,
    #[serde(default)]
    maximum: Option<u32>,
}

/// A TOML number, keeping an integer distinct from a float.
///
/// `divide = 2` is an integer. `divide = 2.0` and `divide = 2.5` are
/// floats. Python's `//` follows that distinction.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FactorInput {
    Int(u32),
    Float(f64),
}

/// The parallel level a build gets in one job, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parallel {
    /// The level EasyBuild runs with.
    pub level: u32,
    /// CPUs the job holds.
    pub cores: u32,
    /// EasyBuild's `--parallel`, when the job sets one.
    pub requested: Option<u32>,
    /// EasyBuild's `max-parallel`, when it lowered `--parallel` or the
    /// cores.
    pub max_parallel: Option<u32>,
    /// The hook limit that matched. It applies only when its result is
    /// below the level EasyBuild chose.
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
            let who = limit.software.as_deref().unwrap_or("every build");
            let op = match (limit.divide, limit.maximum) {
                (Some(FactorInput::Int(factor)), None) if factor > 0 => {
                    LimitOp::Divide(DivideFactor::from_integer(factor))
                }
                (Some(FactorInput::Float(factor)), None) if factor.is_finite() && factor > 0.0 => {
                    LimitOp::Divide(DivideFactor::from_float(factor))
                }
                (None, Some(value)) if value > 0 => LimitOp::Maximum(value),
                _ => bail!(
                    "hook limit for {who} on {} needs one positive divide or maximum",
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
                line: None,
            });
        }
        Ok(Self {
            max_parallel: raw.easybuild.max_parallel,
            parallel: None,
            partitions,
            limits,
            hooks_source: raw.hooks.source,
        })
    }

    /// The hook row for `name` on `target`, as `post_ready_hook` picks it.
    ///
    /// A software entry decides on its own: its row for the target, else
    /// its `*` row, else none. A CPU-target key overrides `*`. Only
    /// software without an entry gets the target's own limit, which on
    /// A64FX is `divide_by_factor` by 4.
    pub fn matching_limit(&self, name: &str, target: &str) -> Option<&HookLimit> {
        let software: Vec<&HookLimit> = self
            .limits
            .iter()
            .filter(|limit| limit.software.as_deref() == Some(name))
            .collect();
        if !software.is_empty() {
            return software
                .iter()
                .copied()
                .find(|limit| limit.target == target)
                .or_else(|| software.iter().copied().find(|limit| limit.target == "*"));
        }
        self.limits
            .iter()
            .find(|limit| limit.software.is_none() && limit.target == target)
    }

    /// The hook limit for `name` on `target`, as `post_ready_hook` picks it.
    pub fn hook_limit(&self, name: &str, target: &str) -> Option<LimitOp> {
        self.matching_limit(name, target).map(|limit| limit.op)
    }

    /// One line per divide factor that is not an integer literal.
    ///
    /// `eb_hooks.py` computes `max(1, parallel // factor)`. A non-integer
    /// factor makes `//` return a float: `16 // 2.5` is `6.0`. A whole
    /// number written as a float does the same: `16 // 2.0` is `8.0`. A
    /// row read from an `eb_hooks.py` names its file and line.
    pub fn factor_warnings(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for limit in &self.limits {
            let LimitOp::Divide(factor) = limit.op else {
                continue;
            };
            if factor.integer_literal() {
                continue;
            }
            let who = limit.software.as_deref().unwrap_or("every build");
            let shown = format_factor(factor);
            let kind = if is_whole(factor.value()) {
                format!("{shown}, a float rather than an integer")
            } else {
                format!("{shown}, which is not an integer")
            };
            let at = limit
                .line
                .map(|line| format!("{}:{line}: ", self.hooks_source))
                .unwrap_or_default();
            lines.push(format!(
                "warning: {at}PARALLELISM_LIMITS divide_by_factor for {who} on {} uses {kind}; eb_hooks.py computes max(1, parallel // factor), and {}. Use an integer factor.",
                limit.target,
                float_example(factor)
            ));
        }
        lines
    }

    /// The level EasyBuild's `set_parallel` gives a build before the hook.
    ///
    /// `--parallel` replaces the core count, so it may be above the cores.
    /// `max-parallel` caps either. The second value is that cap when it
    /// lowered the level.
    fn easybuild_level(&self, cores: u32) -> (u32, Option<u32>) {
        let asked = self.parallel.unwrap_or(cores);
        if self.max_parallel < asked {
            (self.max_parallel, Some(self.max_parallel))
        } else {
            (asked, None)
        }
    }

    /// The parallel level `name` builds with on `partition`.
    ///
    /// `post_ready_hook` applies its op to the cores it detects, not to
    /// `--parallel` or `max-parallel`, and keeps the result only when it
    /// is below the level EasyBuild chose.
    pub fn parallel(&self, name: &str, partition: &Partition, recipe: Option<u32>) -> Parallel {
        let (mut level, max_parallel) = self.easybuild_level(partition.cores);
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
            requested: self.parallel,
            max_parallel,
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
    let cores = parallel.cores;
    let mut parts = vec![match parallel.requested {
        Some(requested) => format!("--parallel {requested} on {cores} cores"),
        None => format!("{cores} cores"),
    }];
    if let Some(max_parallel) = parallel.max_parallel {
        parts.push(format!("max-parallel {max_parallel}"));
    }
    if let Some(recipe) = parallel.recipe {
        parts.push(format!("maxparallel = {recipe} in the easyconfig"));
    }
    let Some(op) = parallel.hook else {
        parts.push(format!("no eb_hooks.py limit for {name}"));
        return parts.join(", ");
    };
    let mut hook = match op {
        LimitOp::Divide(factor) => {
            let shown = format_factor(factor);
            let divided = divide_by_factor(cores, factor);
            let mut text = if divided.float_result {
                format!(
                    "eb_hooks.py divides {name} by {shown} ({cores} // {shown} is {:.1}, a float)",
                    divided.level as f64
                )
            } else {
                format!("eb_hooks.py divides {name} by {shown}")
            };
            if let Some(requested) = parallel.requested {
                text.push_str(&format!(
                    " on the {cores} cores, not on --parallel {requested}"
                ));
            }
            text
        }
        LimitOp::Maximum(value) => format!("eb_hooks.py caps {name} at {value}"),
    };
    let hooked = op.apply(cores);
    if hooked > parallel.level {
        if op != LimitOp::Maximum(hooked) {
            hook.push_str(&format!(", which gives {hooked}"));
        }
        hook.push_str(&format!(
            ", not below {}, so it does not apply",
            parallel.level
        ));
    }
    parts.push(hook);
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

/// One package's `mib_per_job`. `None` means the peak has not been measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobMemory {
    /// EasyBuild name, `libxc`.
    pub name: String,
    /// Version families, `7.0`. Empty covers every version.
    pub versions: Vec<String>,
    /// MiB the worst compile needs, plus headroom. Absent when unmeasured.
    pub mib_per_job: Option<u32>,
    /// Why the number is there, or why it is not.
    pub note: String,
}

/// The per-package `mib_per_job` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobMemoryTable {
    packages: Vec<JobMemory>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJobMemory {
    #[serde(default)]
    package: Vec<RawJobPackage>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJobPackage {
    name: String,
    #[serde(default)]
    versions: Vec<String>,
    mib_per_job: Option<u32>,
    note: String,
}

impl JobMemoryTable {
    /// The table shipped in `data/job-memory.toml`.
    pub fn bundled() -> Result<Self> {
        Self::parse(BUNDLED_JOB_MEMORY).context("data/job-memory.toml")
    }

    /// Read a table from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse {}", path.display()))
    }

    /// Parse the TOML document. A package with no `mib_per_job` is
    /// unmeasured, and its note must say so. A number has to be positive.
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawJobMemory =
            toml::from_str(text).context("job memory table is not valid TOML")?;
        let mut packages = Vec::new();
        let mut seen = BTreeSet::new();
        for package in raw.package {
            let name = package.name.trim().to_string();
            let note = package.note.trim().to_string();
            if name.is_empty() || note.is_empty() {
                bail!("a job memory row needs a name and a note");
            }
            if !seen.insert(name.clone()) {
                bail!("job memory table repeats {name}");
            }
            match package.mib_per_job {
                None => {
                    if !note.to_ascii_lowercase().contains("unmeasured") {
                        bail!(
                            "{name}: a package with no mib_per_job is unmeasured, and the note must say so"
                        );
                    }
                }
                Some(0) => bail!("{name}: mib_per_job must be positive"),
                Some(_) => {}
            }
            packages.push(JobMemory {
                name,
                versions: package.versions,
                mib_per_job: package.mib_per_job,
                note,
            });
        }
        Ok(Self { packages })
    }

    /// Every package, in file order.
    pub fn packages(&self) -> &[JobMemory] {
        &self.packages
    }

    /// The row for `name`, if the table has one.
    pub fn get(&self, name: &str) -> Option<&JobMemory> {
        self.packages.iter().find(|package| package.name == name)
    }
}

/// One node size the parallelism report compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSize {
    /// CPUs the job holds.
    pub cores: u32,
    /// Memory the job can use, in MiB.
    pub memory_mib: u64,
    /// CPU target, `x86_64/amd/zen4`. Absent applies only `*` hook rows.
    pub target: Option<String>,
}

/// Parse `CORES:MEMORY_MIB` or `CORES:MEMORY_MIB:TARGET`.
pub fn parse_node_size(text: &str) -> Result<NodeSize> {
    let Some((cores, rest)) = text.split_once(':') else {
        bail!("--node {text} is CORES:MEMORY_MIB or CORES:MEMORY_MIB:TARGET");
    };
    let (memory, target) = match rest.split_once(':') {
        Some((memory, target)) => (memory, Some(target.to_string())),
        None => (rest, None),
    };
    let cores: u32 = cores
        .parse()
        .with_context(|| format!("--node {text} cores are not an integer"))?;
    let memory_mib: u64 = memory
        .parse()
        .with_context(|| format!("--node {text} memory is not an integer number of MiB"))?;
    if cores == 0 || memory_mib == 0 {
        bail!("--node {text} needs positive cores and memory");
    }
    if target.as_deref().is_some_and(str::is_empty) {
        bail!("--node {text} has an empty target");
    }
    Ok(NodeSize {
        cores,
        memory_mib,
        target,
    })
}

/// MiB from a cgroup v2 `memory.max` byte count. `max` and a blank file
/// are not a limit.
fn memory_max_mib(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() || text == "max" {
        return None;
    }
    let bytes: u64 = text.parse().ok()?;
    Some(bytes / (1024 * 1024))
}

/// The cgroup v2 path from `/proc/self/cgroup` (`0::/path`), relative to
/// the cgroup root. `.` and empty components are dropped. `..` is refused
/// so the walk stays under the cgroup filesystem.
fn cgroup_v2_relative(text: &str) -> Option<String> {
    let path = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("0::"))?;
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        parts.push(part);
    }
    Some(parts.join("/"))
}

/// Smallest cgroup v2 `memory.max` on `cgroup_text`'s path, in MiB.
///
/// `sysfs` is `/sys/fs/cgroup` on a host. The walk starts at the process
/// cgroup and climbs to `sysfs`, and the smallest numeric `memory.max`
/// wins. `max` is unlimited and does not count. No numeric file is
/// `None`, which is when the caller uses MemTotal.
pub fn smallest_cgroup_memory_mib(cgroup_text: &str, sysfs: &Path) -> Option<u64> {
    let relative = cgroup_v2_relative(cgroup_text)?;
    let mut path = if relative.is_empty() {
        sysfs.to_path_buf()
    } else {
        sysfs.join(relative)
    };
    let mut best: Option<u64> = None;
    loop {
        if let Ok(text) = std::fs::read_to_string(path.join("memory.max")) {
            if let Some(mib) = memory_max_mib(&text) {
                best = Some(best.map_or(mib, |have| have.min(mib)));
            }
        }
        if path == sysfs {
            break;
        }
        match path.parent() {
            Some(parent) if parent.starts_with(sysfs) => path = parent.to_path_buf(),
            _ => break,
        }
    }
    best
}

/// `MemTotal` from `/proc/meminfo`, in MiB.
///
/// The line is kilobytes. Dividing by 1024 matches EasyBuild's
/// `get_total_memory()`.
pub fn memtotal_mib(meminfo: &str) -> Option<u64> {
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
        return Some(kb / 1024);
    }
    None
}

/// Memory for `limit_by_memory`, from a cgroup listing, a cgroup
/// filesystem, and a meminfo file.
///
/// The smallest cgroup v2 `memory.max` on the process path wins. With
/// none, MemTotal.
pub fn job_memory_mib_from(cgroup_text: &str, sysfs: &Path, meminfo: &str) -> Option<u64> {
    if let Some(mib) = smallest_cgroup_memory_mib(cgroup_text, sysfs) {
        return Some(mib);
    }
    memtotal_mib(meminfo)
}

/// Memory for `limit_by_memory` on this process.
///
/// The smallest cgroup v2 `memory.max` on `/proc/self/cgroup`'s path,
/// under `/sys/fs/cgroup`. A Slurm job's limit is the one on that path.
/// With no cgroup v2 limit, MemTotal, the same figure EasyBuild's
/// `get_total_memory()` reads.
pub fn job_memory_mib() -> Option<u64> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    job_memory_mib_from(&cgroup, Path::new("/sys/fs/cgroup"), &meminfo)
}

fn node_partition(node: &NodeSize) -> Partition {
    Partition {
        instance: String::new(),
        node: String::new(),
        target: node.target.clone().unwrap_or_default(),
        accel: None,
        cores: node.cores,
        memory_gib: 1.0,
        sources: Vec::new(),
    }
}

/// EasyBuild's level before the hook on `cores`, and the setting that
/// decided it when that is not the cores.
fn easybuild_bound(resources: &BotResources, cores: u32) -> (u32, Option<String>) {
    let (level, capped) = resources.easybuild_level(cores);
    let bound = match (capped, resources.parallel) {
        (Some(max_parallel), _) => Some(format!("max-parallel {max_parallel}")),
        (None, Some(requested)) => Some(format!("--parallel {requested}")),
        (None, None) => None,
    };
    (level, bound)
}

fn current_clause(resources: &BotResources, name: &str, node: &NodeSize) -> (u32, String) {
    let partition = node_partition(node);
    let got = resources.parallel(name, &partition, None);
    let (_, bound) = easybuild_bound(resources, node.cores);
    let then = bound
        .clone()
        .unwrap_or_else(|| format!("{} cores", node.cores));
    let key = resources
        .matching_limit(name, &partition.target)
        .map(|limit| limit.target.as_str());
    let on = key.map(|key| format!(" on {key}")).unwrap_or_default();
    let text = match got.hook {
        Some(LimitOp::Divide(factor)) => {
            let divided = divide_by_factor(node.cores, factor);
            let mut text = format!(
                "divide_by_factor{on}, max(1, {} // {})",
                node.cores,
                format_factor(factor)
            );
            if divided.level != got.level {
                if divided.float_result {
                    text.push_str(&format!(
                        " = {:.1}, a float, then {then}",
                        divided.level as f64
                    ));
                } else {
                    text.push_str(&format!(" = {}, then {then}", divided.level));
                }
            } else if divided.float_result {
                text.push_str(&format!(" = {:.1}, a float", divided.level as f64));
            }
            text
        }
        Some(LimitOp::Maximum(value)) => {
            let raw = set_maximum(node.cores, value);
            let mut text = format!("set_maximum{on}, min({}, {value})", node.cores);
            if raw != got.level {
                text.push_str(&format!(" = {raw}, then {then}"));
            }
            text
        }
        None => match bound {
            Some(bound) => format!("no hook limit, {bound}"),
            None => "no hook limit".to_string(),
        },
    };
    (got.level, text)
}

fn memory_cap_clause(core_cap: u32, mib_per_job: u32, node: &NodeSize) -> String {
    let cap = memory_cap(core_cap, mib_per_job, node.memory_mib);
    let formula = format!("min({core_cap}, {} // {mib_per_job})", node.memory_mib);
    if cap == 0 {
        format!("memory cap 0 ({formula}), not one {mib_per_job} MiB job fits")
    } else {
        format!("memory cap {cap} ({formula})")
    }
}

fn memory_clause(mib_per_job: Option<u32>, node: &NodeSize, resources: &BotResources) -> String {
    let Some(mib_per_job) = mib_per_job else {
        return "memory unmeasured".to_string();
    };
    let raw = limit_by_memory(node.cores, mib_per_job, node.memory_mib);
    let (curr, bound) = easybuild_bound(resources, node.cores);
    let effective = raw.min(curr);
    let formula = format!(
        "max(1, min({}, {} // {mib_per_job}))",
        node.cores, node.memory_mib
    );
    if effective != raw {
        let bound = bound.unwrap_or_else(|| format!("{} cores", node.cores));
        format!("memory {effective} ({formula} = {raw}, then {bound})")
    } else {
        format!("memory {effective} ({formula})")
    }
}

fn report_names(resources: &BotResources, jobs: &JobMemoryTable) -> Vec<String> {
    let mut names = BTreeSet::new();
    for limit in &resources.limits {
        if let Some(name) = &limit.software {
            names.insert(name.clone());
        }
    }
    for package in jobs.packages() {
        names.insert(package.name.clone());
    }
    names.into_iter().collect()
}

fn job_memory_footer(jobs: &JobMemoryTable) -> String {
    let unmeasured: Vec<String> = jobs
        .packages()
        .iter()
        .filter(|package| package.mib_per_job.is_none())
        .map(|package| {
            if package.versions.is_empty() {
                package.name.clone()
            } else {
                format!("{} {}", package.name, package.versions.join(", "))
            }
        })
        .collect();
    if unmeasured.is_empty() {
        if jobs.packages().is_empty() {
            "job memory: the table has no packages, so every memory rule is unmeasured\n"
                .to_string()
        } else {
            "job memory: every package in the table has an mib_per_job\n".to_string()
        }
    } else {
        format!(
            "job memory: {} unmeasured, no mib_per_job\n",
            unmeasured.join("; ")
        )
    }
}

/// Parallelism under the current hook and under `limit_by_memory`.
///
/// Each node is a core count, a memory in MiB, and an optional CPU
/// target. The hook column is what `post_ready_hook` would leave, including
/// EasyBuild's `--parallel` and `max-parallel`. The memory column is
/// `limit_by_memory(cores, mib_per_job)` when the table has a number, and
/// `unmeasured` when it does not. With a number, the hook column is
/// followed by its [`memory_cap`]. No number is invented.
pub fn render_parallelism(
    resources: &BotResources,
    jobs: &JobMemoryTable,
    nodes: &[NodeSize],
) -> String {
    let mut out =
        String::from("parallelism under the current eb_hooks.py limit and under limit_by_memory\n");
    let names = report_names(resources, jobs);
    let requested = resources
        .parallel
        .map(|parallel| format!(", --parallel {parallel}"))
        .unwrap_or_default();
    for node in nodes {
        match &node.target {
            Some(target) => out.push_str(&format!(
                "node {} cores, {} MiB, {target}{requested}\n",
                node.cores, node.memory_mib
            )),
            None => out.push_str(&format!(
                "node {} cores, {} MiB{requested}\n",
                node.cores, node.memory_mib
            )),
        }
        for name in &names {
            let (level, clause) = current_clause(resources, name, node);
            let mib = jobs.get(name).and_then(|package| package.mib_per_job);
            let cap = mib
                .map(|mib| format!("; {}", memory_cap_clause(level, mib, node)))
                .unwrap_or_default();
            out.push_str(&format!(
                "  {name}: current {level} ({clause}){cap}; {}\n",
                memory_clause(mib, node, resources)
            ));
        }
        let sentinel = "\u{0}";
        if resources
            .matching_limit(sentinel, node.target.as_deref().unwrap_or(""))
            .is_some()
        {
            let (level, clause) = current_clause(resources, sentinel, node);
            out.push_str(&format!(
                "  (other builds): current {level} ({clause}); memory unmeasured\n"
            ));
        }
    }
    out.push_str(&job_memory_footer(jobs));
    out
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
    /// Node sizes, `CORES:MEMORY_MIB` or `CORES:MEMORY_MIB:TARGET`.
    /// Prints the hook parallelism beside `limit_by_memory`.
    pub nodes: Vec<String>,
    /// Override `data/job-memory.toml`.
    pub job_memory: Option<PathBuf>,
    /// An EESSI `eb_hooks.py` whose `PARALLELISM_LIMITS` replace the hook
    /// rows of the bot resource table.
    pub hooks: Option<PathBuf>,
    /// EasyBuild's `--parallel` for the job. `None` counts the cores.
    pub parallel: Option<u32>,
    /// EasyBuild's `--max-parallel` for the job, in place of the table's.
    pub max_parallel: Option<u32>,
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
    /// Divide factors that are not integers. Also included in `stdout`.
    pub factor_warnings: Vec<String>,
    /// `--hooks` statements that add to a name and drop the sum, such as
    /// `msg+ "..."`. Also included in `stdout`.
    pub hook_warnings: Vec<String>,
}

/// Estimate each plan build's peak on each bot node and warn where it
/// reaches the job's memory.
pub fn run_oom_check(request: &OomCheckRequest) -> Result<OomCheckReport> {
    if request.easystacks.is_empty() && request.nodes.is_empty() && request.hooks.is_none() {
        return Err(usage(
            "pass at least one --easystack, --node to compare parallelism, or --hooks to check an eb_hooks.py",
        ));
    }
    let mut nodes = Vec::new();
    for text in &request.nodes {
        nodes.push(parse_node_size(text).map_err(usage)?);
    }
    for (flag, value) in [
        ("--parallel", request.parallel),
        ("--max-parallel", request.max_parallel),
    ] {
        if value == Some(0) {
            return Err(usage(format!("{flag} must be a positive integer")));
        }
    }
    let mut resources = match &request.resources {
        Some(path) => BotResources::load(path)?,
        None => BotResources::bundled()?,
    };
    resources.parallel = request.parallel;
    if let Some(max_parallel) = request.max_parallel {
        resources.max_parallel = max_parallel;
    }
    let jobs = match &request.job_memory {
        Some(path) => JobMemoryTable::load(path)?,
        None => JobMemoryTable::bundled()?,
    };
    let mut stdout = String::new();
    let mut hook_warnings = Vec::new();
    if let Some(path) = &request.hooks {
        let file = path.display().to_string();
        let text = std::fs::read_to_string(path).with_context(|| format!("--hooks {file}"))?;
        let hooks = crate::eb_hooks::parse_hooks(&text, &file)?;
        stdout.push_str(&format!(
            "hooks: {} {} rows from {file}\n",
            hooks.limits.len(),
            crate::eb_hooks::LIMITS_NAME
        ));
        hook_warnings = hooks
            .dropped_sums
            .iter()
            .map(|sum| sum.message(&file))
            .collect();
        resources.limits = hooks.limits;
        resources.hooks_source = file;
    }
    let factor_warnings = resources.factor_warnings();
    if request.easystacks.is_empty() {
        for line in factor_warnings.iter().chain(&hook_warnings) {
            stdout.push_str(line);
            stdout.push('\n');
        }
        if !nodes.is_empty() {
            stdout.push_str(&render_parallelism(&resources, &jobs, &nodes));
        }
        return Ok(OomCheckReport {
            stdout,
            warnings: Vec::new(),
            fits: 0,
            builds: 0,
            partitions: 0,
            unknown_arches: Vec::new(),
            factor_warnings,
            hook_warnings,
        });
    }
    let profile = match &request.profile {
        Some(path) => MemoryProfile::load(path)?,
        None => MemoryProfile::bundled()?,
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

    for arch in &unknown_arches {
        stdout.push_str(&format!(
            "{arch} has no bot node in the bot resource table, so it is not checked\n"
        ));
    }
    for line in factor_warnings.iter().chain(&hook_warnings) {
        stdout.push_str(line);
        stdout.push('\n');
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
    if !nodes.is_empty() {
        stdout.push('\n');
        stdout.push_str(&render_parallelism(&resources, &jobs, &nodes));
    }
    Ok(OomCheckReport {
        stdout,
        warnings,
        fits,
        builds: builds.len(),
        partitions: partitions.len(),
        unknown_arches,
        factor_warnings,
        hook_warnings,
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

    fn parse_factor(literal: &str) -> DivideFactor {
        let text = format!(
            "[easybuild]\nmax_parallel = 1\n[hooks]\n[[hooks.limit]]\nsoftware = \"pkg\"\ntarget = \"*\"\ndivide = {literal}\n"
        );
        let resources =
            BotResources::parse(&text).unwrap_or_else(|error| panic!("{literal}: {error}"));
        match resources.limits[0].op {
            LimitOp::Divide(factor) => factor,
            other => panic!("{literal} is not a divide: {other:?}"),
        }
    }

    fn python_divide(parallel: u32, literal: &str) -> (String, String) {
        let script = format!(
            "value = max(1, {parallel} // {literal})\nprint(repr(value))\nprint(type(value).__name__)\n"
        );
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg(&script)
            .output()
            .expect("python3");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).expect("utf8");
        let mut lines = text.lines();
        (
            lines.next().unwrap_or("").to_string(),
            lines.next().unwrap_or("").to_string(),
        )
    }

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
        assert!(resources.factor_warnings().is_empty());
        let jobs = JobMemoryTable::bundled().expect("bundled job memory");
        let libxc = jobs.get("libxc").expect("libxc row");
        assert_eq!(libxc.mib_per_job, None);
        assert!(libxc.note.to_ascii_lowercase().contains("unmeasured"));
        assert!(libxc.versions.iter().any(|family| family == "7.0"));
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
    fn the_hook_applies_to_the_cores_not_to_easybuild_parallel() {
        let mut resources = BotResources::bundled().unwrap();
        let zen4 = zen4(&resources).clone();
        let a64fx = resources
            .partitions
            .iter()
            .find(|p| p.target == "aarch64/a64fx")
            .unwrap()
            .clone();

        resources.parallel = Some(12);
        let libxc = resources.parallel("libxc", &zen4, None);
        // 16 // 2, not 12 // 2.
        assert_eq!(libxc.level, 8);
        assert_eq!(
            parallel_reason("libxc", &libxc),
            "--parallel 12 on 16 cores, eb_hooks.py divides libxc by 2 on the 16 cores, not on --parallel 12"
        );
        assert_eq!(resources.parallel("zlib", &zen4, None).level, 12);

        resources.parallel = Some(4);
        let libxc = resources.parallel("libxc", &zen4, None);
        assert_eq!(libxc.level, 4);
        assert_eq!(
            parallel_reason("libxc", &libxc),
            "--parallel 4 on 16 cores, eb_hooks.py divides libxc by 2 on the 16 cores, not on --parallel 4, which gives 8, not below 4, so it does not apply"
        );
        let rocblas = resources.parallel("rocBLAS", &zen4, None);
        assert_eq!(
            parallel_reason("rocBLAS", &rocblas),
            "--parallel 4 on 16 cores, eb_hooks.py caps rocBLAS at 8, not below 4, so it does not apply"
        );

        // EasyBuild takes --parallel as given, above the cores, up to
        // max-parallel. The hook still starts from the cores.
        resources.parallel = Some(32);
        resources.max_parallel = 64;
        assert_eq!(resources.parallel("zlib", &zen4, None).level, 32);
        assert_eq!(resources.parallel("libxc", &zen4, None).level, 8);
        resources.max_parallel = 16;
        let zlib = resources.parallel("zlib", &zen4, None);
        assert_eq!(zlib.level, 16);
        assert_eq!(
            parallel_reason("zlib", &zlib),
            "--parallel 32 on 16 cores, max-parallel 16, no eb_hooks.py limit for zlib"
        );

        resources.parallel = None;
        let llvm = resources.parallel("LLVM", &a64fx, None);
        assert_eq!(llvm.level, 16);
        assert_eq!(
            parallel_reason("LLVM", &llvm),
            "48 cores, max-parallel 16, eb_hooks.py divides LLVM by 2, which gives 24, not below 16, so it does not apply"
        );
        resources.max_parallel = 48;
        let llvm = resources.parallel("LLVM", &a64fx, None);
        assert_eq!(llvm.level, 24);
        assert_eq!(
            parallel_reason("LLVM", &llvm),
            "48 cores, eb_hooks.py divides LLVM by 2"
        );
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
    fn divide_by_factor_matches_python_floor_division() {
        let cases = ["2", "2.0", "2.5", "20.5", "4", "0.5"];
        for literal in cases {
            let factor = parse_factor(literal);
            for parallel in [1_u32, 16, 48] {
                let got = divide_by_factor(parallel, factor);
                let (python_repr, python_type) = python_divide(parallel, literal);
                let rendered = if got.float_result {
                    format!("{:.1}", got.level as f64)
                } else {
                    got.level.to_string()
                };
                assert_eq!(
                    python_repr, rendered,
                    "max(1, {parallel} // {literal}) is {python_repr}, model says {rendered}"
                );
                assert_eq!(
                    python_type == "float",
                    got.float_result,
                    "{parallel} // {literal} is a {python_type}"
                );
            }
        }
        assert_eq!(set_maximum(48, 6), 6);
        assert_eq!(set_maximum(4, 6), 4);
        assert_eq!(limit_by_memory(16, 8192, 32768), 4);
        assert_eq!(limit_by_memory(16, 100_000, 32768), 1);
        assert_eq!(limit_by_memory(48, 100, 30_016), 48);
    }

    #[test]
    fn the_memory_cap_is_not_raised_to_one_and_matches_the_byte_quotient() {
        assert_eq!(memory_cap(8, 8192, 32_768), 4);
        assert_eq!(memory_cap(8, 8192, 131_072), 8);
        assert_eq!(memory_cap(8, 8192, 4096), 0);
        assert_eq!(memory_cap(6, 8192, 30_016), 3);
        assert_eq!(memory_cap(8, 0, 32_768), 8);
        const BYTES_PER_MIB: u64 = 1 << 20;
        let gib = 1024 * BYTES_PER_MIB;
        for bytes in [
            0,
            BYTES_PER_MIB - 1,
            32 * gib,
            32 * gib + 12_345,
            30_016 * BYTES_PER_MIB + BYTES_PER_MIB - 1,
        ] {
            for mib_per_job in [1, 5, 6554, 8192] {
                let by_bytes = bytes / (u64::from(mib_per_job) * BYTES_PER_MIB);
                let by_mib = memory_cap(u32::MAX, mib_per_job, bytes / BYTES_PER_MIB);
                assert_eq!(u64::from(by_mib), by_bytes, "{bytes} // {mib_per_job} MiB");
            }
        }
    }

    #[test]
    fn a_non_integer_divide_warns_and_uses_the_numeric_floor() {
        let text = r#"
[easybuild]
max_parallel = 16

[[partition]]
instance = "eessi-bot-mc-aws"
node = "amd-zen4"
target = "x86_64/amd/zen4"
cores = 16
memory_gib = 32.0

[hooks]
[[hooks.limit]]
software = "libxc"
target = "*"
divide = 2.5

[[hooks.limit]]
software = "LLVM"
target = "*"
divide = 2.0

[[hooks.limit]]
target = "aarch64/a64fx"
divide = 4
"#;
        let resources = BotResources::parse(text).unwrap();
        let warnings = resources.factor_warnings();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("libxc on * uses 2.5, which is not an integer"),
            "{warnings:?}"
        );
        assert!(
            warnings[0].contains("16 // 2.5 is 6.0, a float"),
            "{warnings:?}"
        );
        assert!(
            warnings[0].contains("Use an integer factor."),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("LLVM on * uses 2.0, a float rather than an integer"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("16 // 2.0 is 8.0, a float"),
            "{warnings:?}"
        );
        let zen4 = zen4(&resources);
        assert_eq!(resources.parallel("libxc", zen4, None).level, 6);
        assert_eq!(resources.parallel("LLVM", zen4, None).level, 8);
        // The integer A64FX row is not a warning. zlib has no software
        // entry, so it takes that row: 48 // 4 = 12. libxc has only '*',
        // so on A64FX it still divides by 2.5: floor(48 / 2.5) = 19.
        assert!(resources
            .factor_warnings()
            .iter()
            .all(|line| !line.contains("every build")));
        assert_eq!(
            resources
                .hook_limit("zlib", "aarch64/a64fx")
                .map(|op| op.apply(48)),
            Some(12)
        );
        assert_eq!(
            resources
                .hook_limit("libxc", "aarch64/a64fx")
                .map(|op| op.apply(48)),
            Some(19)
        );
    }

    #[test]
    fn cpu_target_key_overrides_the_wildcard() {
        let resources = BotResources::bundled().unwrap();
        let libxc_star = resources
            .matching_limit("libxc", "x86_64/amd/zen4")
            .unwrap();
        assert_eq!(libxc_star.target, "*");
        assert!(matches!(libxc_star.op, LimitOp::Divide(_)));
        let libxc_a64fx = resources.matching_limit("libxc", "aarch64/a64fx").unwrap();
        assert_eq!(libxc_a64fx.target, "aarch64/a64fx");
        assert!(matches!(libxc_a64fx.op, LimitOp::Maximum(6)));
        let global = resources.matching_limit("zlib", "aarch64/a64fx").unwrap();
        assert!(global.software.is_none());
        assert!(matches!(global.op, LimitOp::Divide(_)));
        assert!(resources
            .matching_limit("zlib", "x86_64/amd/zen4")
            .is_none());
        // QuantumESPRESSO has an A64FX row only, so zen4 is left alone.
        assert!(resources
            .matching_limit("QuantumESPRESSO", "x86_64/amd/zen4")
            .is_none());
        assert!(resources
            .matching_limit("QuantumESPRESSO", "aarch64/a64fx")
            .is_some());
    }

    #[test]
    fn limit_by_memory_report_names_an_unmeasured_package() {
        let resources = BotResources::bundled().unwrap();
        let jobs = JobMemoryTable::bundled().unwrap();
        let nodes = vec![
            NodeSize {
                cores: 16,
                memory_mib: 32_768,
                target: Some("x86_64/amd/zen4".to_string()),
            },
            NodeSize {
                cores: 48,
                memory_mib: 30_016,
                target: Some("aarch64/a64fx".to_string()),
            },
        ];
        let text = render_parallelism(&resources, &jobs, &nodes);
        assert!(
            text.contains(
                "  libxc: current 8 (divide_by_factor on *, max(1, 16 // 2)); memory unmeasured\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "  libxc: current 6 (set_maximum on aarch64/a64fx, min(48, 6)); memory unmeasured\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("  (other builds): current 12 (divide_by_factor on aarch64/a64fx, max(1, 48 // 4)); memory unmeasured\n"),
            "{text}"
        );
        assert!(
            text.contains("job memory: libxc 7.0 unmeasured, no mib_per_job\n"),
            "{text}"
        );
        assert!(!text.contains("8192"), "{text}");
        let fixture = JobMemoryTable::parse(
            r#"
[[package]]
name = "libxc"
versions = ["7.0"]
mib_per_job = 8192
note = "fixture for the report arithmetic, not a measurement of libxc"
"#,
        )
        .unwrap();
        let one = render_parallelism(&resources, &fixture, &nodes[..1]);
        assert!(
            one.contains(
                "  libxc: current 8 (divide_by_factor on *, max(1, 16 // 2)); memory cap 4 (min(8, 32768 // 8192)); memory 4 (max(1, min(16, 32768 // 8192)))\n"
            ),
            "{one}"
        );
        assert!(
            one.contains("job memory: every package in the table has an mib_per_job\n"),
            "{one}"
        );
    }

    #[test]
    fn cgroup_v2_memory_is_the_smallest_max_else_memtotal() {
        let root = std::env::temp_dir().join(format!(
            "eb-stack-cgroup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("leaf")).unwrap();
        std::fs::write(root.join("memory.max"), "17179869184\n").unwrap();
        std::fs::write(root.join("leaf/memory.max"), "8589934592\n").unwrap();
        let cgroup = "0::/leaf\n";
        assert_eq!(smallest_cgroup_memory_mib(cgroup, &root), Some(8192));
        std::fs::write(root.join("leaf/memory.max"), "max\n").unwrap();
        assert_eq!(smallest_cgroup_memory_mib(cgroup, &root), Some(16384));
        std::fs::write(root.join("memory.max"), "max\n").unwrap();
        let meminfo = "MemTotal:       32768000 kB\n";
        assert_eq!(job_memory_mib_from(cgroup, &root, meminfo), Some(32_000));
        assert_eq!(
            job_memory_mib_from("12:memory:/user.slice\n", &root, meminfo),
            Some(32_000)
        );
        assert_eq!(smallest_cgroup_memory_mib("0::/../leaf\n", &root), None);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(memtotal_mib(meminfo), Some(32_000));
        assert!(job_memory_mib().is_some_and(|mib| mib > 0));
    }

    #[test]
    fn a_job_memory_row_without_the_unmeasured_mark_is_refused() {
        let error = JobMemoryTable::parse(
            r#"
[[package]]
name = "libxc"
note = "to be filled in later"
"#,
        )
        .unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("unmeasured"), "{text}");
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
