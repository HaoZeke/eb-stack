//! Plan builds around a scratch inode quota.
//!
//! A native EasyBuild job keeps its build tree on the scratch file system
//! while it runs. A regression-test-heavy code unpacks several hundred
//! thousand files; seven such jobs at once put a campaign over a quota of
//! three million inodes, and an admission check that only looks at the
//! free count at start time admits one job too many. This module gives the
//! planner and the controller one footprint model so the cap is decided
//! when the shards are planned, and the check at admission time counts
//! what the running jobs have reserved.
//!
//! Three pieces:
//!
//! - A footprint table, `footprints.json`: per software name, the peak
//!   build-tree inode count. [`derive`] takes the peak over ledger records
//!   whose `build.inodes_peak` names the count a build reached, a static
//!   table for packages the ledger has not seen, and [`CLASSES`] for the
//!   rest. [`Footprints::of_module`] answers for a module or an easyconfig
//!   filename.
//! - A shard footprint, [`shard_footprint`]: a shard builds its modules one
//!   after another, so one build tree is on disk at a time, and the shard
//!   costs its largest module footprint plus [`DEFAULT_LINE_OVERHEAD`]
//!   inodes per line for the tarballs, the EasyBuild tmpdir, and the logs
//!   the line leaves on scratch until the job ends. [`max_concurrent`] is
//!   the most shards whose footprints fit under the quota minus a margin
//!   at once, taking the largest first.
//! - An admission decision, [`decide`]: the inodes the quota still allows,
//!   minus what the jobs that are still running have reserved, must cover
//!   this job's footprint and the margin. The reservation is a file per
//!   job under one directory; a reservation whose job the caller does not
//!   list as live is stale and dropped.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Build-tree inodes assumed for a package no record, table, or class names.
pub const DEFAULT_FOOTPRINT: u64 = 60_000;

/// Inodes one build-list line keeps on scratch beside its build tree: the
/// source tarballs, the EasyBuild tmpdir, and the logs, until the job ends.
pub const DEFAULT_LINE_OVERHEAD: u64 = 1_000;

/// Inodes kept free under the quota by default.
pub const DEFAULT_MARGIN: u64 = 100_000;

/// Exit status of `stack admit` when the job is refused.
pub const REFUSED_EXIT: i32 = 78;

/// Footprint classes for packages no record or table names: codes with a
/// large regression-test tree, GPU codes, and vendor toolchains.
pub const CLASSES: &[(&str, u64)] = &[
    ("CP2K", 350_000),
    ("LAMMPS", 150_000),
    ("PyTorch", 150_000),
    ("SeisSol", 150_000),
    ("NVHPC", 150_000),
    ("intel", 150_000),
    ("intel-compilers", 150_000),
];

/// Peak build-tree inodes per software name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footprints {
    /// The footprint of a package `packages` does not name.
    pub default: u64,
    /// Footprint per software name.
    pub packages: BTreeMap<String, u64>,
}

impl Default for Footprints {
    fn default() -> Self {
        Self::builtin()
    }
}

impl Footprints {
    /// [`CLASSES`] over [`DEFAULT_FOOTPRINT`].
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            default: DEFAULT_FOOTPRINT,
            packages: CLASSES
                .iter()
                .map(|(name, inodes)| ((*name).to_string(), *inodes))
                .collect(),
        }
    }

    /// The footprint of a software name.
    #[must_use]
    pub fn of_package(&self, name: &str) -> u64 {
        self.packages.get(name).copied().unwrap_or(self.default)
    }

    /// The footprint of a module `Name/VersionFull`, or of an easyconfig
    /// filename `Name-version-toolchain.eb` when the line was planned
    /// opaque. A filename matches the longest table name it starts with,
    /// followed by `-`, so `SciPy-bundle-2026.03.eb` is `SciPy-bundle`.
    #[must_use]
    pub fn of_module(&self, module: &str) -> u64 {
        if let Some((name, _)) = module.split_once('/') {
            return self.of_package(name);
        }
        let file = module.strip_suffix(".eb").unwrap_or(module);
        self.packages
            .iter()
            .filter(|(name, _)| {
                file.strip_prefix(name.as_str())
                    .is_some_and(|rest| rest.starts_with('-'))
            })
            .max_by_key(|(name, _)| name.len())
            .map(|(_, inodes)| *inodes)
            .unwrap_or(self.default)
    }

    /// Read a `footprints.json`: the document `stack footprints` writes
    /// (`footprints` object and `default`), or a flat object of
    /// `name: inodes`, which a hand-kept file is.
    pub fn from_json(value: &Value) -> Result<Self> {
        let Value::Object(top) = value else {
            bail!("footprints is not a JSON object");
        };
        let default = match top.get("default") {
            None | Some(Value::Null) => DEFAULT_FOOTPRINT,
            Some(value) => value
                .as_u64()
                .ok_or_else(|| anyhow!("footprints default is not an integer"))?,
        };
        let (table, flat) = match top.get("footprints") {
            Some(Value::Object(table)) => (table, false),
            Some(_) => bail!("footprints is not a JSON object"),
            None => (top, true),
        };
        let mut packages = BTreeMap::new();
        for (name, inodes) in table {
            if flat && matches!(name.as_str(), "default" | "schema_version" | "generated_at") {
                continue;
            }
            let inodes = inodes
                .as_u64()
                .ok_or_else(|| anyhow!("footprint of {name} is not an integer"))?;
            packages.insert(name.clone(), inodes);
        }
        Ok(Self { default, packages })
    }
}

/// The software name of a module `Name/VersionFull`.
#[must_use]
pub fn package_of_module(module: &str) -> &str {
    module.split('/').next().unwrap_or(module)
}

/// Where a package's footprint came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The peak `build.inodes_peak` over ledger records, with the module
    /// that reached it and the records counted.
    Ledger {
        /// The module whose record reached the peak.
        module: String,
        /// How many ledger records carried a peak for the package.
        records: usize,
    },
    /// A static table line.
    Table,
    /// Both; the larger won.
    LedgerAndTable {
        /// The module whose record reached the ledger peak.
        module: String,
        /// How many ledger records carried a peak for the package.
        records: usize,
    },
    /// A [`CLASSES`] entry.
    Class,
}

impl Source {
    fn label(&self) -> &'static str {
        match self {
            Self::Ledger { .. } => "ledger",
            Self::Table => "table",
            Self::LedgerAndTable { .. } => "ledger+table",
            Self::Class => "class",
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Self::Ledger { module, records } | Self::LedgerAndTable { module, records } => json!({
                "from": self.label(),
                "module": module,
                "records": records,
            }),
            Self::Table | Self::Class => json!({"from": self.label()}),
        }
    }
}

/// `build.inodes_peak` of a record, when `build` is an object carrying an
/// integer of that name.
#[must_use]
pub fn inodes_peak(record: &Map<String, Value>) -> Option<u64> {
    record
        .get("build")
        .and_then(Value::as_object)
        .and_then(|build| build.get("inodes_peak"))
        .and_then(Value::as_u64)
}

/// Peak `build.inodes_peak` per software name over ledger records: the
/// peak, the module that reached it, and the records that carried a value.
#[must_use]
pub fn ledger_peaks(entries: &[crate::ledger::Entry]) -> BTreeMap<String, (u64, String, usize)> {
    let mut peaks: BTreeMap<String, (u64, String, usize)> = BTreeMap::new();
    for entry in entries {
        let Some(peak) = inodes_peak(&entry.record) else {
            continue;
        };
        let module = entry.module();
        let name = package_of_module(module).to_string();
        let slot = peaks.entry(name).or_insert((0, module.to_string(), 0));
        slot.2 += 1;
        if peak > slot.0 || slot.2 == 1 {
            slot.0 = peak;
            slot.1 = module.to_string();
        }
    }
    peaks
}

/// Parse a static footprint table: `Name<TAB>inodes` per line (a space
/// also separates), `#` comments and blank lines skipped. A later line for
/// the same name replaces an earlier one.
pub fn parse_table(text: &str) -> Result<BTreeMap<String, u64>, String> {
    let mut out = BTreeMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('\t')
            .or_else(|| line.split_once(char::is_whitespace))
            .ok_or_else(|| format!("line {}: want name<TAB>inodes, got {line}", i + 1))?;
        let inodes: u64 = value.trim().parse().map_err(|_| {
            format!(
                "line {}: {} is not a whole number of inodes",
                i + 1,
                value.trim()
            )
        })?;
        out.insert(name.trim().to_string(), inodes);
    }
    Ok(out)
}

/// Derive the footprint table: the ledger peak and the static table where
/// either names a package (the larger when both do), then [`CLASSES`] for
/// the rest, over [`DEFAULT_FOOTPRINT`].
#[must_use]
pub fn derive(
    entries: &[crate::ledger::Entry],
    table: &BTreeMap<String, u64>,
) -> (Footprints, BTreeMap<String, Source>) {
    let peaks = ledger_peaks(entries);
    let mut footprints = Footprints::builtin();
    let mut sources: BTreeMap<String, Source> = footprints
        .packages
        .keys()
        .map(|name| (name.clone(), Source::Class))
        .collect();
    for (name, inodes) in table {
        footprints.packages.insert(name.clone(), *inodes);
        sources.insert(name.clone(), Source::Table);
    }
    for (name, (peak, module, records)) in peaks {
        let source = match table.get(&name) {
            Some(tabled) => {
                footprints.packages.insert(name.clone(), peak.max(*tabled));
                Source::LedgerAndTable { module, records }
            }
            None => {
                footprints.packages.insert(name.clone(), peak);
                Source::Ledger { module, records }
            }
        };
        sources.insert(name, source);
    }
    (footprints, sources)
}

/// Arguments for `stack footprints`.
pub struct FootprintsRequest {
    /// Ledger JSON Lines files.
    pub ledgers: Vec<PathBuf>,
    /// Static tables, `Name<TAB>inodes` per line; a later file wins a name.
    pub tables: Vec<PathBuf>,
    /// The `footprints.json` to write.
    pub out: PathBuf,
    /// RFC 3339 time written into the file; absent is now.
    pub generated_at: Option<String>,
}

/// What `stack footprints` wrote.
pub struct FootprintsRun {
    /// Packages in the table.
    pub packages: usize,
    /// Packages whose footprint a ledger record set.
    pub from_ledger: usize,
    /// Packages whose footprint a static table set.
    pub from_table: usize,
    /// Packages taken from [`CLASSES`].
    pub from_class: usize,
    /// Ledger records that carried `build.inodes_peak`.
    pub records: usize,
}

impl FootprintsRun {
    /// One line for the CLI.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "packages={} ledger={} table={} class={} records={}",
            self.packages, self.from_ledger, self.from_table, self.from_class, self.records
        )
    }
}

/// Read ledgers and tables and write `footprints.json`.
pub fn run_stack_footprints(args: &FootprintsRequest) -> Result<FootprintsRun> {
    let mut entries = Vec::new();
    for path in &args.ledgers {
        entries.extend(crate::ledger::read_log(path)?);
    }
    let mut table = BTreeMap::new();
    for path in &args.tables {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--table {}: not readable", path.display()))?;
        let parsed = parse_table(&text).map_err(|error| {
            crate::cli_parse::usage(format!("--table {}: {error}", path.display()))
        })?;
        table.extend(parsed);
    }
    let stamp = crate::site_commands::generated_at(args.generated_at.clone(), true)?;
    let (footprints, sources) = derive(&entries, &table);
    let records = entries
        .iter()
        .filter(|entry| inodes_peak(&entry.record).is_some())
        .count();
    let count = |want: fn(&Source) -> bool| sources.values().filter(|s| want(s)).count();
    let document = json!({
        "schema_version": 1,
        "generated_at": stamp,
        "default": footprints.default,
        "ledgers": args.ledgers.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "tables": args.tables.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "footprints": footprints.packages,
        "sources": sources
            .iter()
            .map(|(name, source)| (name.clone(), source.to_json()))
            .collect::<Map<String, Value>>(),
    });
    crate::write_json_pretty(&args.out, &document)?;
    Ok(FootprintsRun {
        packages: footprints.packages.len(),
        from_ledger: count(|s| matches!(s, Source::Ledger { .. } | Source::LedgerAndTable { .. })),
        from_table: count(|s| matches!(s, Source::Table)),
        from_class: count(|s| matches!(s, Source::Class)),
        records,
    })
}

/// Read a `footprints.json`.
pub fn read_footprints(path: &Path) -> Result<Footprints> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("--inode-footprints {}: not readable", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("--inode-footprints {}: not JSON", path.display()))?;
    Footprints::from_json(&value).with_context(|| format!("--inode-footprints {}", path.display()))
}

/// The inodes one shard holds on scratch while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardFootprint {
    /// `peak + overhead`.
    pub inodes: u64,
    /// The largest module footprint in the shard.
    pub peak: u64,
    /// The module with that footprint, when the shard builds anything.
    pub peak_module: Option<String>,
    /// The line overhead summed over the shard's lines.
    pub overhead: u64,
}

impl ShardFootprint {
    fn to_json(&self) -> Value {
        json!({
            "inodes": self.inodes,
            "peak": self.peak,
            "peak_module": self.peak_module,
            "overhead": self.overhead,
        })
    }
}

/// The footprint of a shard that builds `modules` over `lines` build-list
/// lines: the largest module footprint plus `line_overhead` per line.
/// Modules build one after another, so one build tree is on disk at a time.
#[must_use]
pub fn shard_footprint(
    modules: &[String],
    lines: usize,
    footprints: &Footprints,
    line_overhead: u64,
) -> ShardFootprint {
    let mut peak = 0;
    let mut peak_module = None;
    for module in modules {
        let inodes = footprints.of_module(module);
        if peak_module.is_none() || inodes > peak {
            peak = inodes;
            peak_module = Some(module.clone());
        }
    }
    let overhead = line_overhead.saturating_mul(lines as u64);
    ShardFootprint {
        inodes: peak.saturating_add(overhead),
        peak,
        peak_module,
        overhead,
    }
}

/// The most shards that fit under `budget` at once: the largest `k`
/// footprints sum to at most `budget`.
#[must_use]
pub fn max_concurrent(footprints: &[u64], budget: u64) -> usize {
    let mut sorted = footprints.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut sum = 0u64;
    let mut fit = 0;
    for inodes in sorted {
        sum = sum.saturating_add(inodes);
        if sum > budget {
            break;
        }
        fit += 1;
    }
    fit
}

/// The inode budget of a shard plan, for `plan.json`.
pub struct PlanBudget {
    /// `--inode-quota`, when given.
    pub quota: Option<u64>,
    /// `--inode-margin`.
    pub margin: u64,
    /// The footprints file, when given.
    pub footprints_file: Option<PathBuf>,
    /// Its SHA-256, when given.
    pub footprints_sha256: Option<String>,
    /// The footprint table in force.
    pub footprints: Footprints,
    /// `--inode-line-overhead`.
    pub line_overhead: u64,
}

impl PlanBudget {
    /// `quota - margin`, when a quota is given.
    #[must_use]
    pub fn budget(&self) -> Option<u64> {
        self.quota.map(|quota| quota.saturating_sub(self.margin))
    }

    /// The `inode_budget` object of `plan.json`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "quota": self.quota,
            "margin": self.margin,
            "budget": self.budget(),
            "footprints_file": self.footprints_file.as_ref().map(|p| p.display().to_string()),
            "footprints_sha256": self.footprints_sha256,
            "default_footprint": self.footprints.default,
            "line_overhead": self.line_overhead,
        })
    }

    /// The footprint of each shard, the prelude's, and the cap. The cap is
    /// the shard count when no quota is given. An error names a shard that
    /// does not fit under the budget on its own.
    pub fn apply(
        &self,
        shards: &[crate::shard::Shard],
        prelude: &[String],
        shard_name: impl Fn(usize) -> String,
    ) -> Result<(Vec<Value>, Value, usize)> {
        let footprints: Vec<ShardFootprint> = shards
            .iter()
            .map(|shard| {
                shard_footprint(
                    &shard.modules,
                    shard.lines.len(),
                    &self.footprints,
                    self.line_overhead,
                )
            })
            .collect();
        let prelude_footprint = shard_footprint(
            prelude,
            usize::from(!prelude.is_empty()),
            &self.footprints,
            self.line_overhead,
        );
        let cap = match self.budget() {
            None => shards.len(),
            Some(budget) => {
                if let Some((s, big)) = footprints
                    .iter()
                    .enumerate()
                    .find(|(_, f)| f.inodes > budget)
                {
                    bail!(
                        "{} needs {} inodes ({} for {} plus {} line overhead); the quota {} minus the margin {} leaves {}; nothing written",
                        shard_name(s),
                        big.inodes,
                        big.peak,
                        big.peak_module.as_deref().unwrap_or("no module"),
                        big.overhead,
                        self.quota.unwrap_or(0),
                        self.margin,
                        budget
                    );
                }
                let sizes: Vec<u64> = footprints.iter().map(|f| f.inodes).collect();
                max_concurrent(&sizes, budget)
            }
        };
        Ok((
            footprints.iter().map(ShardFootprint::to_json).collect(),
            prelude_footprint.to_json(),
            cap,
        ))
    }
}

/// What the quota file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quota {
    /// Inodes the quota still allows.
    pub available: u64,
    /// `inodes_used`, when given.
    pub used: Option<u64>,
    /// `inodes_soft`, when given.
    pub soft: Option<u64>,
    /// `inodes_hard`, when given.
    pub hard: Option<u64>,
}

/// Read a quota document: `inodes_available`, or `inodes_soft` minus
/// `inodes_used` (the soft limit is the one GPFS enforces after its grace
/// period). `inodes_hard` is carried through.
pub fn parse_quota(value: &Value) -> Result<Quota> {
    let Value::Object(top) = value else {
        bail!("quota is not a JSON object");
    };
    let field = |name: &str| -> Result<Option<u64>> {
        match top.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_u64()
                .map(Some)
                .ok_or_else(|| anyhow!("quota {name} is not a whole number")),
        }
    };
    let used = field("inodes_used")?;
    let soft = field("inodes_soft")?;
    let hard = field("inodes_hard")?;
    let available = match (field("inodes_available")?, soft, used) {
        (Some(available), _, _) => available,
        (None, Some(soft), Some(used)) => soft.saturating_sub(used),
        _ => bail!("quota needs inodes_available, or inodes_soft and inodes_used"),
    };
    Ok(Quota {
        available,
        used,
        soft,
        hard,
    })
}

/// One job's reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    /// The job id.
    pub job: String,
    /// Inodes reserved.
    pub footprint: u64,
    /// When it was written, RFC 3339.
    pub ts: String,
}

/// A job id is a file name: non-empty, no `/`, not `.` or `..`, and only
/// letters, digits, `.`, `_`, and `-`.
pub fn check_job(job: &str) -> Result<()> {
    if job.is_empty() || job == "." || job == ".." || job.starts_with('.') {
        bail!("--job {job:?} is not a job id");
    }
    if !job
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("--job {job:?} may hold only letters, digits, '.', '_' and '-'");
    }
    Ok(())
}

/// Read every `*.json` reservation under `dir`. A missing directory is no
/// reservations. A file that is not a reservation is an error, because a
/// reservation that cannot be read hides inodes.
pub fn read_reservations(dir: &Path) -> Result<Vec<Reservation>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("read reservation {}", path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("reservation {} is not JSON", path.display()))?;
        let job = value
            .get("job")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("reservation {} has no job", path.display()))?;
        let footprint = value
            .get("footprint")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("reservation {} has no footprint", path.display()))?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if stem != job {
            bail!("reservation {} names job {job}, not {stem}", path.display());
        }
        out.push(Reservation {
            job: job.to_string(),
            footprint,
            ts: value
                .get("ts")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    out.sort_by(|a, b| a.job.cmp(&b.job));
    Ok(out)
}

/// The admission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Whether the job may start.
    pub admitted: bool,
    /// Inodes the quota still allows.
    pub available: u64,
    /// Inodes the live jobs have reserved, this job's own left out.
    pub reserved: u64,
    /// This job's footprint.
    pub footprint: u64,
    /// The margin kept free.
    pub margin: u64,
    /// `available - reserved - footprint - margin`; negative when refused.
    pub headroom: i64,
    /// Reservations counted, by job.
    pub counted: Vec<Reservation>,
    /// Reservations of jobs not in the live list, dropped.
    pub dropped: Vec<Reservation>,
}

/// Decide admission. `live` is the jobs still running, as the caller's
/// scheduler lists them; absent, every reservation but this job's own is
/// counted and none is dropped. This job's own earlier reservation is never
/// counted, so a job that asks again is judged on the others alone.
#[must_use]
pub fn decide(
    available: u64,
    reservations: &[Reservation],
    job: &str,
    live: Option<&[String]>,
    footprint: u64,
    margin: u64,
) -> Decision {
    let mut counted = Vec::new();
    let mut dropped = Vec::new();
    for reservation in reservations {
        if reservation.job == job {
            continue;
        }
        match live {
            Some(live) if !live.iter().any(|id| id == &reservation.job) => {
                dropped.push(reservation.clone());
            }
            _ => counted.push(reservation.clone()),
        }
    }
    let reserved = counted.iter().map(|r| r.footprint).sum::<u64>();
    let headroom =
        i128::from(available) - i128::from(reserved) - i128::from(footprint) - i128::from(margin);
    let headroom =
        i64::try_from(headroom).unwrap_or(if headroom < 0 { i64::MIN } else { i64::MAX });
    Decision {
        admitted: headroom >= 0,
        available,
        reserved,
        footprint,
        margin,
        headroom,
        counted,
        dropped,
    }
}

/// Arguments for `stack admit`.
pub struct AdmitRequest {
    /// The quota document, or `-` for stdin.
    pub quota_json: PathBuf,
    /// This job's footprint, from `plan.json`.
    pub footprint: u64,
    /// This job's id.
    pub job: String,
    /// The reservation directory.
    pub reservations: PathBuf,
    /// Inodes kept free.
    pub margin: u64,
    /// Jobs still running, from the caller's scheduler; absent counts every
    /// other reservation.
    pub live_jobs: Option<Vec<String>>,
    /// RFC 3339 time written into the reservation; absent is now.
    pub ts: Option<String>,
}

/// What `stack admit` decided and wrote.
pub struct AdmitReport {
    /// The decision.
    pub decision: Decision,
    /// The reservation written, when admitted.
    pub reservation: Option<PathBuf>,
    /// The one JSON line to print.
    pub line: String,
}

/// Read the quota and the reservations, decide, write the reservation on
/// admit, and drop stale reservations.
pub fn run_stack_admit(args: &AdmitRequest) -> Result<AdmitReport> {
    check_job(&args.job).map_err(|error| crate::cli_parse::usage(error.to_string()))?;
    let quota_text = if args.quota_json == Path::new("-") {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .context("read --quota-json from stdin")?;
        text
    } else {
        std::fs::read_to_string(&args.quota_json)
            .with_context(|| format!("--quota-json {}: not readable", args.quota_json.display()))?
    };
    let quota_value: Value = serde_json::from_str(&quota_text)
        .with_context(|| format!("--quota-json {}: not JSON", args.quota_json.display()))?;
    let quota = parse_quota(&quota_value)
        .with_context(|| format!("--quota-json {}", args.quota_json.display()))?;
    let stamp = crate::site_commands::generated_at(args.ts.clone(), true)?;
    let reservations = read_reservations(&args.reservations)?;
    let decision = decide(
        quota.available,
        &reservations,
        &args.job,
        args.live_jobs.as_deref(),
        args.footprint,
        args.margin,
    );
    for stale in &decision.dropped {
        let path = args.reservations.join(format!("{}.json", stale.job));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("drop stale {}", path.display()))
            }
        }
    }
    let reservation = if decision.admitted {
        std::fs::create_dir_all(&args.reservations)
            .with_context(|| format!("create {}", args.reservations.display()))?;
        let path = args.reservations.join(format!("{}.json", args.job));
        let record = json!({
            "job": args.job,
            "footprint": args.footprint,
            "margin": args.margin,
            "ts": stamp,
        });
        crate::write_json_pretty(&path, &record)?;
        Some(path)
    } else {
        None
    };
    let line = serde_json::to_string(&json!({
        "job": args.job,
        "admitted": decision.admitted,
        "available": decision.available,
        "used": quota.used,
        "soft": quota.soft,
        "hard": quota.hard,
        "reserved": decision.reserved,
        "footprint": decision.footprint,
        "margin": decision.margin,
        "headroom": decision.headroom,
        "live_jobs": args.live_jobs,
        "counted": decision
            .counted
            .iter()
            .map(|r| json!({"job": r.job, "footprint": r.footprint}))
            .collect::<Vec<_>>(),
        "dropped": decision.dropped.iter().map(|r| r.job.clone()).collect::<Vec<_>>(),
        "reservation": reservation.as_ref().map(|p| p.display().to_string()),
        "ts": stamp,
    }))?;
    Ok(AdmitReport {
        decision,
        reservation,
        line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::seal_record;

    fn entry(module: &str, inodes: Option<u64>, index: usize) -> crate::ledger::Entry {
        let mut record = json!({
            "ts": "2026-11-03T10:00:00Z",
            "actor": "jenkins/run/1",
            "stack": "2025",
            "target": "x86_64/amd/genoa",
            "module": module,
            "location": format!("/sw/{module}"),
            "action": "install",
        });
        if let Some(inodes) = inodes {
            record["build"] = json!({"job": "snellius/4411", "inodes_peak": inodes});
        }
        seal_record(&record, index, "unused").unwrap()
    }

    #[test]
    fn the_ledger_peak_is_the_max_over_records_and_names_the_module() {
        let entries = [
            entry("CP2K/2025.2-foss-2025a", Some(410_000), 0),
            entry("CP2K/2025.1-foss-2025a", Some(395_000), 1),
            entry("zlib/1.3.1-GCCcore-14.2.0", Some(1_200), 2),
            entry("HDF5/1.14.6-gompi-2025a", None, 3),
        ];
        let peaks = ledger_peaks(&entries);
        assert_eq!(
            peaks["CP2K"],
            (410_000, "CP2K/2025.2-foss-2025a".to_string(), 2)
        );
        assert_eq!(peaks["zlib"].0, 1_200);
        assert!(!peaks.contains_key("HDF5"));
    }

    #[test]
    fn derive_prefers_the_ledger_and_the_table_over_the_classes() {
        let entries = [
            entry("CP2K/2025.2-foss-2025a", Some(410_000), 0),
            entry(
                "LAMMPS/2Aug2023-foss-2025a-kokkos-CUDA-12.8.0",
                Some(90_000),
                1,
            ),
        ];
        let table = parse_table("# name\tinodes\nSeisSol\t200000\nLAMMPS 120000\n").unwrap();
        let (footprints, sources) = derive(&entries, &table);
        assert_eq!(footprints.of_package("CP2K"), 410_000);
        assert_eq!(
            sources["CP2K"],
            Source::Ledger {
                module: "CP2K/2025.2-foss-2025a".into(),
                records: 1
            }
        );
        // The larger of the ledger peak and the table wins.
        assert_eq!(footprints.of_package("LAMMPS"), 120_000);
        assert!(matches!(sources["LAMMPS"], Source::LedgerAndTable { .. }));
        assert_eq!(footprints.of_package("SeisSol"), 200_000);
        assert_eq!(sources["SeisSol"], Source::Table);
        assert_eq!(footprints.of_package("PyTorch"), 150_000);
        assert_eq!(sources["PyTorch"], Source::Class);
        assert_eq!(footprints.of_package("zlib"), DEFAULT_FOOTPRINT);
        assert!(parse_table("CP2K\tmany\n").is_err());
        assert!(parse_table("CP2K\n").is_err());
    }

    #[test]
    fn a_module_or_an_easyconfig_filename_resolves_to_its_package() {
        let mut footprints = Footprints::builtin();
        footprints.packages.insert("SciPy-bundle".into(), 70_000);
        footprints.packages.insert("SciPy".into(), 65_000);
        assert_eq!(footprints.of_module("CP2K/2025.2-foss-2025a"), 350_000);
        assert_eq!(footprints.of_module("CP2K-2025.2-foss-2025a.eb"), 350_000);
        assert_eq!(
            footprints.of_module("SciPy-bundle-2026.03-gfbf-2026a.eb"),
            70_000
        );
        assert_eq!(footprints.of_module("SciPy-1.16.0-gfbf-2026a.eb"), 65_000);
        assert_eq!(footprints.of_module("CP2Kplus-1.eb"), DEFAULT_FOOTPRINT);
        assert_eq!(footprints.of_module("zlib/1.3.1"), DEFAULT_FOOTPRINT);
    }

    #[test]
    fn footprints_json_reads_the_written_document_and_a_flat_map() {
        let written = json!({
            "schema_version": 1,
            "generated_at": "2026-11-07T00:00:00Z",
            "default": 50000,
            "footprints": {"CP2K": 410000},
            "sources": {"CP2K": {"from": "ledger"}},
        });
        let parsed = Footprints::from_json(&written).unwrap();
        assert_eq!(parsed.default, 50_000);
        assert_eq!(parsed.packages["CP2K"], 410_000);
        assert_eq!(parsed.packages.len(), 1);
        let flat = Footprints::from_json(&json!({"CP2K": 400000, "default": 80000})).unwrap();
        assert_eq!(flat.default, 80_000);
        assert_eq!(flat.packages["CP2K"], 400_000);
        assert!(Footprints::from_json(&json!({"CP2K": "lots"})).is_err());
        assert!(Footprints::from_json(&json!([1])).is_err());
    }

    #[test]
    fn a_shard_costs_its_largest_module_plus_the_line_overhead() {
        let footprints = Footprints::builtin();
        let modules = [
            "CP2K/2025.2-foss-2025a",
            "zlib/1.3.1",
            "Libint/2.9.0-GCC-14.2.0",
        ]
        .map(String::from);
        let footprint = shard_footprint(&modules, 2, &footprints, DEFAULT_LINE_OVERHEAD);
        assert_eq!(footprint.peak, 350_000);
        assert_eq!(
            footprint.peak_module.as_deref(),
            Some("CP2K/2025.2-foss-2025a")
        );
        assert_eq!(footprint.overhead, 2_000);
        assert_eq!(footprint.inodes, 352_000);
        let empty = shard_footprint(&[], 1, &footprints, DEFAULT_LINE_OVERHEAD);
        assert_eq!(empty.peak, 0);
        assert_eq!(empty.peak_module, None);
        assert_eq!(empty.inodes, 1_000);
    }

    #[test]
    fn the_cap_is_the_most_largest_shards_that_fit() {
        // Seven lanes: three CP2K-sized, one GPU-sized, three small. A
        // budget of 2.9M fits all seven; 1,053,000 fits the three largest
        // and no more; 1,204,000 fits those and the GPU lane; 300k fits
        // not even the largest, so nothing starts.
        let sizes = [351_000, 61_000, 351_000, 151_000, 61_000, 351_000, 61_000];
        assert_eq!(max_concurrent(&sizes, 2_900_000), 7);
        assert_eq!(max_concurrent(&sizes, 1_053_000), 3);
        assert_eq!(max_concurrent(&sizes, 1_204_000), 4);
        assert_eq!(max_concurrent(&sizes, 300_000), 0);
        assert_eq!(max_concurrent(&[], 10), 0);
    }

    #[test]
    fn the_plan_budget_refuses_a_shard_that_never_fits() {
        let budget = PlanBudget {
            quota: Some(400_000),
            margin: 100_000,
            footprints_file: None,
            footprints_sha256: None,
            footprints: Footprints::builtin(),
            line_overhead: DEFAULT_LINE_OVERHEAD,
        };
        let shards = vec![crate::shard::Shard {
            lines: vec![0],
            modules: vec!["CP2K/2025.2-foss-2025a".into()],
            weight: 1,
        }];
        let error = budget
            .apply(&shards, &[], |s| format!("shard-{:02}", s + 1))
            .unwrap_err()
            .to_string();
        assert!(error.contains("shard-01 needs 351000 inodes"), "{error}");
        assert!(error.contains("leaves 300000"), "{error}");
        let wide = PlanBudget {
            quota: Some(3_000_000),
            ..budget
        };
        let (per_shard, prelude, cap) = wide
            .apply(&shards, &["zlib/1.3.1".into()], |s| format!("shard-{s}"))
            .unwrap();
        assert_eq!(per_shard[0]["inodes"], 351_000);
        assert_eq!(prelude["inodes"], 61_000);
        assert_eq!(cap, 1);
    }

    fn reservation(job: &str, footprint: u64) -> Reservation {
        Reservation {
            job: job.into(),
            footprint,
            ts: "2026-11-07T00:00:00Z".into(),
        }
    }

    #[test]
    fn admission_counts_live_reservations_and_the_margin() {
        let held = [reservation("4401", 351_000), reservation("4402", 151_000)];
        let live = ["4401".to_string(), "4402".to_string()];
        // 1,000,000 - 502,000 - 351,000 - 100,000 = 47,000: admitted.
        let yes = decide(1_000_000, &held, "4403", Some(&live), 351_000, 100_000);
        assert!(yes.admitted);
        assert_eq!(yes.reserved, 502_000);
        assert_eq!(yes.headroom, 47_000);
        assert_eq!(yes.counted.len(), 2);
        assert!(yes.dropped.is_empty());
        // 900,000 leaves -53,000: refused.
        let no = decide(900_000, &held, "4403", Some(&live), 351_000, 100_000);
        assert!(!no.admitted);
        assert_eq!(no.headroom, -53_000);
    }

    #[test]
    fn a_reservation_of_a_job_no_longer_live_is_dropped() {
        let held = [reservation("4401", 351_000), reservation("4402", 151_000)];
        let live = ["4402".to_string()];
        let decision = decide(600_000, &held, "4403", Some(&live), 151_000, 100_000);
        assert!(decision.admitted, "{decision:?}");
        assert_eq!(decision.reserved, 151_000);
        assert_eq!(decision.dropped, vec![reservation("4401", 351_000)]);
        // Without a live list every other reservation counts and none is dropped.
        let all = decide(600_000, &held, "4403", None, 151_000, 100_000);
        assert!(!all.admitted);
        assert_eq!(all.reserved, 502_000);
        assert!(all.dropped.is_empty());
    }

    #[test]
    fn a_jobs_own_reservation_is_not_counted_against_it() {
        let held = [reservation("4401", 351_000), reservation("4403", 351_000)];
        let live = ["4401".to_string(), "4403".to_string()];
        let again = decide(900_000, &held, "4403", Some(&live), 351_000, 100_000);
        assert!(again.admitted, "{again:?}");
        assert_eq!(again.reserved, 351_000);
        assert_eq!(again.counted.len(), 1);
        assert!(again.dropped.is_empty());
    }

    #[test]
    fn the_quota_document_gives_available_inodes() {
        let quota = parse_quota(&json!({
            "inodes_used": 2_700_000,
            "inodes_soft": 3_000_000,
            "inodes_hard": 4_000_000,
        }))
        .unwrap();
        assert_eq!(quota.available, 300_000);
        assert_eq!(quota.hard, Some(4_000_000));
        let direct = parse_quota(&json!({"inodes_available": 12})).unwrap();
        assert_eq!(direct.available, 12);
        let over = parse_quota(&json!({"inodes_used": 5, "inodes_soft": 3})).unwrap();
        assert_eq!(over.available, 0);
        assert!(parse_quota(&json!({"inodes_used": 5})).is_err());
        assert!(parse_quota(&json!({"inodes_available": -1})).is_err());
    }

    #[test]
    fn a_job_id_is_a_file_name() {
        assert!(check_job("4411").is_ok());
        assert!(check_job("shard-01_4411").is_ok());
        assert!(check_job("").is_err());
        assert!(check_job("../x").is_err());
        assert!(check_job("a/b").is_err());
        assert!(check_job(".hidden").is_err());
    }

    #[test]
    fn admit_writes_the_reservation_and_drops_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        let reservations = dir.path().join("reservations");
        std::fs::create_dir_all(&reservations).unwrap();
        std::fs::write(
            reservations.join("4401.json"),
            r#"{"job":"4401","footprint":351000,"ts":"2026-11-07T00:00:00Z"}"#,
        )
        .unwrap();
        std::fs::write(
            reservations.join("4399.json"),
            r#"{"job":"4399","footprint":351000,"ts":"2026-11-06T00:00:00Z"}"#,
        )
        .unwrap();
        let quota = dir.path().join("quota.json");
        std::fs::write(
            &quota,
            r#"{"inodes_used":2000000,"inodes_soft":3000000,"inodes_hard":4000000}"#,
        )
        .unwrap();
        let request = AdmitRequest {
            quota_json: quota.clone(),
            footprint: 351_000,
            job: "4403".into(),
            reservations: reservations.clone(),
            margin: 100_000,
            live_jobs: Some(vec!["4401".into()]),
            ts: Some("2026-11-07T01:00:00Z".into()),
        };
        let report = run_stack_admit(&request).unwrap();
        assert!(report.decision.admitted, "{}", report.line);
        assert_eq!(report.decision.reserved, 351_000);
        assert!(!reservations.join("4399.json").exists(), "stale dropped");
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(reservations.join("4403.json")).unwrap())
                .unwrap();
        assert_eq!(written["footprint"], 351_000);
        assert_eq!(written["ts"], "2026-11-07T01:00:00Z");
        let line: Value = serde_json::from_str(&report.line).unwrap();
        assert_eq!(line["admitted"], true);
        assert_eq!(line["dropped"], json!(["4399"]));
        assert_eq!(line["headroom"], 198_000);

        // A second job asks with 4401 and 4403 live: 1,000,000 - 702,000 -
        // 351,000 - 100,000 < 0, refused, and no file is written.
        let refused = run_stack_admit(&AdmitRequest {
            job: "4404".into(),
            live_jobs: Some(vec!["4401".into(), "4403".into()]),
            ..request
        })
        .unwrap();
        assert!(!refused.decision.admitted);
        assert!(refused.reservation.is_none());
        assert!(!reservations.join("4404.json").exists());
        let line: Value = serde_json::from_str(&refused.line).unwrap();
        assert_eq!(line["admitted"], false);
        assert_eq!(line["reserved"], 702_000);
    }

    #[test]
    fn a_reservation_file_naming_another_job_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("4401.json"),
            r#"{"job":"4402","footprint":1}"#,
        )
        .unwrap();
        let error = read_reservations(dir.path()).unwrap_err().to_string();
        assert!(error.contains("names job 4402, not 4401"), "{error}");
        assert!(read_reservations(&dir.path().join("absent"))
            .unwrap()
            .is_empty());
    }
}
