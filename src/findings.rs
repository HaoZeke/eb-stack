//! Vulnerability scanner findings per installed module, with the rebuild
//! closure of each module.
//!
//! A scanner exports one row per finding: the severity, the advisory id, the
//! package and version it matched, the version that fixes it, and the modules
//! that carry the package. [`parse_findings_csv`] reads that export.
//! [`summarize`] groups the rows by module and joins them with a
//! [`RebuildDocument`] computed over the same installed list, so every module
//! with a finding gets two costs:
//!
//! * `in_place`: the patched module, bundles that ship it as an extension, and
//!   builddependency-only dependents left in review. This is what a backported
//!   patch that keeps the version forces.
//! * `bump`: the patched module and every installed module that depends on it,
//!   at runtime or at build time, within its toolchain generation. A new
//!   version is a new module path, and EasyBuild writes dependency versions
//!   into each dependent's module file and run path, so this is what a version
//!   bump forces.

use crate::rebuild_set::{
    RebuildDocument, StackEasystack, CLASS_MUST_REBUILD, CLASS_NO_REBUILD, CLASS_REVIEW,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Schema of [`FindingsDocument`]. Bumped when a field changes meaning.
pub const FINDINGS_SCHEMA_VERSION: u32 = 1;

/// Severities in descending order, as scanners spell them.
pub const SEVERITIES: [&str; 5] = ["Critical", "High", "Medium", "Low", "Negligible"];

/// Weight of one finding of each severity in [`ModuleFindings::score`].
///
/// Critical 10, High 5, Medium 2, Low 1, Negligible and unknown 0.
pub fn severity_weight(severity: &str) -> u64 {
    match severity_rank(severity) {
        Some(0) => 10,
        Some(1) => 5,
        Some(2) => 2,
        Some(3) => 1,
        _ => 0,
    }
}

/// Position in [`SEVERITIES`], ignoring ASCII case. `None` for anything else.
pub fn severity_rank(severity: &str) -> Option<usize> {
    SEVERITIES
        .iter()
        .position(|known| known.eq_ignore_ascii_case(severity.trim()))
}

/// Why a findings export could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FindingsError {
    /// The header lacks a required column.
    #[error("findings: no column {0} in the header")]
    MissingColumn(String),
    /// A quoted field is not closed before the end of the file.
    #[error("findings: line {0}: quoted field is not closed")]
    UnclosedQuote(usize),
    /// A record has fewer fields than the header.
    #[error("findings: line {line}: {found} fields, the header has {expected}")]
    ShortRecord {
        /// Line the record starts on, counting from 1.
        line: usize,
        /// Fields in the record.
        found: usize,
        /// Fields in the header.
        expected: usize,
    },
    /// The file has no header.
    #[error("findings: empty file")]
    Empty,
}

/// One row of the scanner export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// Scanner severity, as written.
    pub severity: String,
    /// CVE or advisory id.
    pub vuln_id: String,
    /// Package the scanner matched.
    pub package: String,
    /// Installed version of that package.
    pub version: String,
    /// Version that fixes it. Empty when the scanner names none.
    pub fixed_in: String,
    /// `fixed`, `not-fixed`, or empty.
    pub fix_state: String,
    /// Modules that carry the package, `Name/VersionFull`.
    pub modules: Vec<String>,
}

const COLUMNS_REQUIRED: [&str; 4] = ["severity", "vuln_id", "package", "used_in"];

/// Read the scanner export: comma-separated, a header row, RFC 4180 quoting.
///
/// Required columns are `severity`, `vuln_id`, `package`, and `used_in`.
/// `version`, `fixed_in`, and `fix_state` are read when present. Column names
/// ignore ASCII case. `used_in` holds one or more modules separated by `;`.
/// Blank lines are skipped.
pub fn parse_findings_csv(text: &str) -> Result<Vec<Finding>, FindingsError> {
    let records = split_records(text)?;
    let mut iter = records.into_iter();
    let Some((_, header)) = iter.next() else {
        return Err(FindingsError::Empty);
    };
    let header: Vec<String> = header
        .iter()
        .map(|name| {
            name.trim()
                .trim_start_matches('\u{feff}')
                .to_ascii_lowercase()
        })
        .collect();
    let column = |name: &str| header.iter().position(|field| field == name);
    for name in COLUMNS_REQUIRED {
        if column(name).is_none() {
            return Err(FindingsError::MissingColumn(name.to_string()));
        }
    }
    let index = |name: &str| column(name);
    let (severity, vuln_id, package, used_in) = (
        index("severity").expect("checked"),
        index("vuln_id").expect("checked"),
        index("package").expect("checked"),
        index("used_in").expect("checked"),
    );
    let version = index("version");
    let fixed_in = index("fixed_in");
    let fix_state = index("fix_state");
    let mut findings = Vec::new();
    for (line, fields) in iter {
        if fields.iter().all(|field| field.trim().is_empty()) {
            continue;
        }
        if fields.len() < header.len() {
            return Err(FindingsError::ShortRecord {
                line,
                found: fields.len(),
                expected: header.len(),
            });
        }
        let get = |at: Option<usize>| {
            at.map(|at| fields[at].trim().to_string())
                .unwrap_or_default()
        };
        let modules = fields[used_in]
            .split(';')
            .map(str::trim)
            .filter(|module| !module.is_empty())
            .map(str::to_string)
            .collect();
        findings.push(Finding {
            severity: get(Some(severity)),
            vuln_id: get(Some(vuln_id)),
            package: get(Some(package)),
            version: get(version),
            fixed_in: get(fixed_in),
            fix_state: get(fix_state),
            modules,
        });
    }
    Ok(findings)
}

/// Records with the line each one starts on.
fn split_records(text: &str) -> Result<Vec<(usize, Vec<String>)>, FindingsError> {
    let mut records = Vec::new();
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut line = 1;
    let mut start = 1;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push(c);
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => fields.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => {
                fields.push(std::mem::take(&mut field));
                records.push((start, std::mem::take(&mut fields)));
                line += 1;
                start = line;
            }
            _ => field.push(c),
        }
    }
    if quoted {
        return Err(FindingsError::UnclosedQuote(start));
    }
    if !field.is_empty() || !fields.is_empty() {
        fields.push(field);
        records.push((start, fields));
    }
    Ok(records)
}

/// The easyconfig filename EasyBuild gives a module: `Name/Version` becomes
/// `Name-Version.eb`. `None` when there is no `/` or either side is empty.
pub fn easyconfig_filename(module: &str) -> Option<String> {
    let (name, version) = module.trim().split_once('/')?;
    if name.is_empty() || version.is_empty() {
        return None;
    }
    Some(format!("{name}-{version}.eb"))
}

/// An easystack whose entries are the installed modules, for a stack that was
/// installed without one. A module whose name gives no filename is left out.
pub fn installed_easystack(installed: &BTreeSet<String>) -> StackEasystack {
    let mut text = String::from("easyconfigs:\n");
    for module in installed {
        if let Some(file) = easyconfig_filename(module) {
            text.push_str(&format!("  - '{}'\n", file.replace('\'', "''")));
        }
    }
    StackEasystack {
        path: "installed modules".to_string(),
        text,
    }
}

/// Unique advisory ids per severity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SeverityCounts {
    /// Critical.
    pub critical: usize,
    /// High.
    pub high: usize,
    /// Medium.
    pub medium: usize,
    /// Low.
    pub low: usize,
    /// Negligible.
    pub negligible: usize,
    /// A severity outside [`SEVERITIES`].
    pub other: usize,
}

impl SeverityCounts {
    fn add(&mut self, severity: &str) {
        match severity_rank(severity) {
            Some(0) => self.critical += 1,
            Some(1) => self.high += 1,
            Some(2) => self.medium += 1,
            Some(3) => self.low += 1,
            Some(4) => self.negligible += 1,
            _ => self.other += 1,
        }
    }
}

/// What a fix of one module costs in rebuilds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModuleClosure {
    /// Must-rebuild rows: the module and bundles that ship it.
    pub must_rebuild: usize,
    /// Builddependency-only dependents left in review.
    pub review: usize,
    /// Runtime dependents, direct or transitive.
    pub runtime: usize,
    /// `must_rebuild + review`: what a patch that keeps the version forces.
    pub in_place: usize,
    /// The module plus every installed dependent: what a version bump forces.
    pub bump: usize,
    /// Installed dependents, sorted.
    pub dependents: Vec<String>,
    /// Dependents counted in `in_place`: review rows and bundles, sorted.
    pub in_place_dependents: Vec<String>,
}

/// One installed module with at least one finding.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ModuleFindings {
    /// `Name/VersionFull`.
    pub module: String,
    /// Easyconfig filename derived from the module name.
    pub easyconfig: String,
    /// Highest severity among its findings.
    pub highest: String,
    /// Unique advisory ids per severity.
    pub counts: SeverityCounts,
    /// Unique advisory ids.
    pub findings: usize,
    /// Ids whose fix state is `fixed`.
    pub fixed: usize,
    /// Ids whose fix state is `not-fixed`.
    pub not_fixed: usize,
    /// Ids with no fix state.
    pub unknown_fix: usize,
    /// Packages the scanner matched in this module, sorted.
    pub packages: Vec<String>,
    /// Sum of [`severity_weight`] over the unique ids.
    pub score: u64,
    /// Rebuild cost. `None` when the easyconfig is not in the deployed graph.
    pub closure: Option<ModuleClosure>,
    /// `score / in_place`, rounded to two decimals. Zero without a closure.
    pub value_per_rebuild: f64,
}

/// Cost of fixing every module with a finding at this severity or above.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeverityTotal {
    /// The severity.
    pub severity: String,
    /// Unique advisory ids at exactly this severity.
    pub ids: usize,
    /// Installed modules with a finding at exactly this severity.
    pub modules: usize,
    /// Installed modules whose highest finding is this severity.
    pub modules_highest: usize,
    /// Installed modules with a finding at this severity or above.
    pub modules_at_or_above: usize,
    /// Union of [`ModuleClosure::in_place`] over those modules.
    pub in_place_at_or_above: usize,
    /// Union of [`ModuleClosure::bump`] over those modules.
    pub bump_at_or_above: usize,
}

/// Findings of one installed stack, joined with its rebuild closure.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingsDocument {
    /// [`FINDINGS_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// When it was computed.
    pub generated_at: String,
    /// Where the installed list came from.
    pub installed_source: String,
    /// Rows in the export.
    pub rows: usize,
    /// Unique advisory ids in the export, over every module.
    pub ids: usize,
    /// Installed modules with a finding.
    pub modules: usize,
    /// Per severity, from Critical down.
    pub totals: Vec<SeverityTotal>,
    /// Modules ranked by `value_per_rebuild`, then score, then name.
    pub ranked: Vec<ModuleFindings>,
    /// Modules named in the export that are not installed, sorted.
    pub not_installed: Vec<String>,
    /// Installed modules with findings whose easyconfig is not in the graph.
    pub not_in_graph: Vec<String>,
}

/// Easyconfig filenames of the installed modules the findings name, sorted.
pub fn changed_easyconfigs(findings: &[Finding], installed: &BTreeSet<String>) -> Vec<String> {
    let mut out = BTreeSet::new();
    for finding in findings {
        for module in &finding.modules {
            if installed.contains(module) {
                if let Some(file) = easyconfig_filename(module) {
                    out.insert(file);
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Group `findings` by installed module and attach the closure `document`
/// computed for those modules' easyconfigs.
pub fn summarize(
    findings: &[Finding],
    installed: &BTreeSet<String>,
    document: &RebuildDocument,
) -> FindingsDocument {
    let closures = closures_by_changed(document);
    let missing: BTreeSet<&str> = document.missing.iter().map(String::as_str).collect();

    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut not_installed = BTreeSet::new();
    // module -> vuln id -> (severity rank, severity, fix state)
    let mut per_module: BTreeMap<&str, BTreeMap<&str, (usize, &str, &str)>> = BTreeMap::new();
    let mut packages: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for finding in findings {
        ids.insert(finding.vuln_id.as_str());
        let rank = severity_rank(&finding.severity).unwrap_or(SEVERITIES.len());
        for module in &finding.modules {
            if !installed.contains(module) {
                not_installed.insert(module.clone());
                continue;
            }
            let row = (rank, finding.severity.as_str(), finding.fix_state.as_str());
            let slot = per_module
                .entry(module.as_str())
                .or_default()
                .entry(finding.vuln_id.as_str())
                .or_insert(row);
            // The same id listed twice for one module keeps its worst row.
            if rank < slot.0 {
                *slot = row;
            }
            packages
                .entry(module.as_str())
                .or_default()
                .insert(finding.package.as_str());
        }
    }

    let mut ranked = Vec::new();
    let mut not_in_graph = Vec::new();
    for (module, by_id) in &per_module {
        let easyconfig = easyconfig_filename(module).unwrap_or_default();
        let mut counts = SeverityCounts::default();
        let (mut fixed, mut not_fixed, mut unknown_fix, mut score) = (0, 0, 0, 0);
        let mut highest = SEVERITIES.len();
        for (rank, severity, state) in by_id.values() {
            counts.add(severity);
            score += severity_weight(severity);
            highest = highest.min(*rank);
            match state.trim().to_ascii_lowercase().as_str() {
                "fixed" => fixed += 1,
                "not-fixed" | "not_fixed" | "wont-fix" => not_fixed += 1,
                _ => unknown_fix += 1,
            }
        }
        let closure = if missing.contains(easyconfig.as_str()) {
            None
        } else {
            closures.get(&easyconfig).cloned()
        };
        if closure.is_none() {
            not_in_graph.push(module.to_string());
        }
        let value_per_rebuild = closure
            .as_ref()
            .map(|closure| round2(score as f64 / closure.in_place.max(1) as f64))
            .unwrap_or(0.0);
        ranked.push(ModuleFindings {
            module: module.to_string(),
            easyconfig,
            highest: SEVERITIES.get(highest).unwrap_or(&"unknown").to_string(),
            counts,
            findings: by_id.len(),
            fixed,
            not_fixed,
            unknown_fix,
            packages: packages
                .get(module)
                .map(|set| set.iter().map(|name| name.to_string()).collect())
                .unwrap_or_default(),
            score,
            closure,
            value_per_rebuild,
        });
    }
    ranked.sort_by(|left, right| {
        right
            .value_per_rebuild
            .total_cmp(&left.value_per_rebuild)
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| left.module.cmp(&right.module))
    });

    let totals = SEVERITIES
        .iter()
        .enumerate()
        .map(|(level, severity)| severity_total(level, severity, &per_module, &ranked))
        .collect();

    FindingsDocument {
        schema_version: FINDINGS_SCHEMA_VERSION,
        generated_at: document.generated_at.clone(),
        installed_source: document.installed_source.clone(),
        rows: findings.len(),
        ids: ids.len(),
        modules: per_module.len(),
        totals,
        ranked,
        not_installed: not_installed.into_iter().collect(),
        not_in_graph,
    }
}

fn severity_total(
    level: usize,
    severity: &str,
    per_module: &BTreeMap<&str, BTreeMap<&str, (usize, &str, &str)>>,
    ranked: &[ModuleFindings],
) -> SeverityTotal {
    let mut ids = BTreeSet::new();
    let mut modules = 0;
    let mut modules_highest = 0;
    let mut at_or_above = BTreeSet::new();
    for (module, by_id) in per_module {
        let mut here = false;
        let mut best = SEVERITIES.len();
        for (id, (rank, _, _)) in by_id {
            best = best.min(*rank);
            if *rank == level {
                here = true;
                ids.insert(*id);
            }
        }
        if here {
            modules += 1;
        }
        if best == level {
            modules_highest += 1;
        }
        if best <= level {
            at_or_above.insert(*module);
        }
    }
    let mut in_place = BTreeSet::new();
    let mut bump = BTreeSet::new();
    for row in ranked {
        if !at_or_above.contains(row.module.as_str()) {
            continue;
        }
        in_place.insert(row.module.clone());
        bump.insert(row.module.clone());
        if let Some(closure) = &row.closure {
            in_place.extend(closure.in_place_dependents.iter().cloned());
            bump.extend(closure.dependents.iter().cloned());
        }
    }
    SeverityTotal {
        severity: severity.to_string(),
        ids: ids.len(),
        modules,
        modules_highest,
        modules_at_or_above: at_or_above.len(),
        in_place_at_or_above: in_place.len(),
        bump_at_or_above: bump.len(),
    }
}

fn closures_by_changed(document: &RebuildDocument) -> BTreeMap<String, ModuleClosure> {
    let mut classes: BTreeMap<String, BTreeMap<String, &str>> = BTreeMap::new();
    for row in &document.rows {
        for changed in row.changed.split(", ") {
            let slot = classes.entry(changed.to_string()).or_default();
            if row.edge_kind == "changed" {
                continue;
            }
            let class = row.class.as_str();
            let entry = slot.entry(row.dependent.clone()).or_insert(class);
            if class_strength(class) > class_strength(entry) {
                *entry = class;
            }
        }
    }
    classes
        .into_iter()
        .map(|(changed, dependents)| {
            let mut closure = ModuleClosure {
                must_rebuild: 1,
                ..ModuleClosure::default()
            };
            for (dependent, class) in &dependents {
                match *class {
                    CLASS_MUST_REBUILD => closure.must_rebuild += 1,
                    CLASS_REVIEW => closure.review += 1,
                    CLASS_NO_REBUILD => closure.runtime += 1,
                    _ => {}
                }
                if *class != CLASS_NO_REBUILD {
                    closure.in_place_dependents.push(dependent.clone());
                }
                closure.dependents.push(dependent.clone());
            }
            closure.in_place = closure.must_rebuild + closure.review;
            closure.bump = 1 + dependents.len();
            (changed, closure)
        })
        .collect()
}

fn class_strength(class: &str) -> u8 {
    match class {
        CLASS_MUST_REBUILD => 2,
        CLASS_REVIEW => 1,
        _ => 0,
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The ranked table and the per-severity totals as Markdown.
pub fn render_markdown(document: &FindingsDocument) -> String {
    let mut out = String::new();
    out.push_str("# Findings and rebuild cost\n\n");
    out.push_str(&format!(
        "Computed {} against the modules installed for `{}`. {} rows, {} unique ids, {} installed modules with a finding.\n\n",
        document.generated_at, document.installed_source, document.rows, document.ids, document.modules
    ));
    out.push_str(
        "In place is the patched module, bundles that ship it, and builddependency-only \
         dependents: what a backported patch that keeps the version rebuilds. Bump is the \
         module and every installed dependent in its toolchain generation: what a new \
         version rebuilds. Score weighs Critical 10, High 5, Medium 2, Low 1. Value is \
         score per in-place rebuild.\n\n",
    );
    out.push_str("| Severity | Ids | Modules | Highest here | Modules at or above | In place at or above | Bump at or above |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for total in &document.totals {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            total.severity,
            total.ids,
            total.modules,
            total.modules_highest,
            total.modules_at_or_above,
            total.in_place_at_or_above,
            total.bump_at_or_above
        ));
    }
    out.push_str(
        "\n| Rank | Module | Highest | C | H | M | L | Fixed | Score | In place | Bump | Value |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for (rank, row) in document.ranked.iter().enumerate() {
        let (in_place, bump) = row
            .closure
            .as_ref()
            .map(|closure| (closure.in_place.to_string(), closure.bump.to_string()))
            .unwrap_or_else(|| ("-".to_string(), "-".to_string()));
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {}/{} | {} | {} | {} | {:.2} |\n",
            rank + 1,
            row.module,
            row.highest,
            row.counts.critical,
            row.counts.high,
            row.counts.medium,
            row.counts.low,
            row.fixed,
            row.findings,
            row.score,
            in_place,
            bump,
            row.value_per_rebuild
        ));
    }
    if !document.not_in_graph.is_empty() {
        out.push_str("\nInstalled, with findings, and not in the easyconfig graph:\n\n");
        for module in &document.not_in_graph {
            out.push_str(&format!("- `{module}`\n"));
        }
    }
    if !document.not_installed.is_empty() {
        out.push_str("\nNamed by the findings and not installed:\n\n");
        for module in &document.not_installed {
            out.push_str(&format!("- `{module}`\n"));
        }
    }
    out
}

/// One line per ranked module, comma-separated with a header.
pub fn render_csv(document: &FindingsDocument) -> String {
    let mut out = String::from(
        "rank,module,easyconfig,highest,critical,high,medium,low,negligible,findings,fixed,not_fixed,unknown_fix,score,must_rebuild,review,runtime,in_place,bump,value_per_rebuild,packages,dependents\n",
    );
    for (rank, row) in document.ranked.iter().enumerate() {
        let closure = row.closure.clone().unwrap_or_default();
        let cost = |value: usize| {
            if row.closure.is_some() {
                value.to_string()
            } else {
                String::new()
            }
        };
        let fields = [
            (rank + 1).to_string(),
            row.module.clone(),
            row.easyconfig.clone(),
            row.highest.clone(),
            row.counts.critical.to_string(),
            row.counts.high.to_string(),
            row.counts.medium.to_string(),
            row.counts.low.to_string(),
            row.counts.negligible.to_string(),
            row.findings.to_string(),
            row.fixed.to_string(),
            row.not_fixed.to_string(),
            row.unknown_fix.to_string(),
            row.score.to_string(),
            cost(closure.must_rebuild),
            cost(closure.review),
            cost(closure.runtime),
            cost(closure.in_place),
            cost(closure.bump),
            format!("{:.2}", row.value_per_rebuild),
            row.packages.join(";"),
            closure.dependents.join(";"),
        ];
        let line: Vec<String> = fields.iter().map(|field| csv_field(field)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rebuild_set::{RebuildCounts, RebuildRow, REBUILD_SET_SCHEMA_VERSION};

    const CSV: &str = "severity,vuln_id,package,version,fixed_in,fix_state,used_in\n\
Critical,CVE-1,zlib,1.3.1,1.3.2,fixed,zlib/1.3.1-GCCcore-15.2.0\n\
High,CVE-2,zlib,1.3.1,,,zlib/1.3.1-GCCcore-15.2.0;Gone/1.0\n\
Low,CVE-2,zlib,1.3.1,,,zlib/1.3.1-GCCcore-15.2.0\n\
Medium,\"GHSA-a,b\",\"py \"\"x\"\"\",2.0,2.1,not-fixed,PyApp/1.0-GCCcore-15.2.0\n\
\n";

    fn row(changed: &str, dependent: &str, edge: &str, class: &str) -> RebuildRow {
        RebuildRow {
            changed: changed.to_string(),
            dependent: dependent.to_string(),
            dependent_easyconfig: String::new(),
            edge_kind: edge.to_string(),
            class: class.to_string(),
            reason: String::new(),
            eb_version: None,
        }
    }

    fn document(rows: Vec<RebuildRow>, missing: Vec<String>) -> RebuildDocument {
        RebuildDocument {
            schema_version: REBUILD_SET_SCHEMA_VERSION,
            generated_at: "2026-10-30T00:00:00Z".to_string(),
            installed_source: "fixture".to_string(),
            prefix: None,
            symbols_checked: false,
            easystacks: Vec::new(),
            changed: Vec::new(),
            missing,
            unresolved: Vec::new(),
            notes: Vec::new(),
            counts: RebuildCounts {
                no_rebuild: 0,
                review: 0,
                must_rebuild: 0,
            },
            rows,
        }
    }

    #[test]
    fn the_export_reads_quoted_fields_and_splits_modules() {
        let findings = parse_findings_csv(CSV).expect("parse");
        assert_eq!(findings.len(), 4);
        assert_eq!(
            findings[1].modules,
            ["zlib/1.3.1-GCCcore-15.2.0", "Gone/1.0"]
        );
        assert_eq!(findings[3].vuln_id, "GHSA-a,b");
        assert_eq!(findings[3].package, "py \"x\"");
        assert_eq!(findings[3].fix_state, "not-fixed");
        assert_eq!(findings[0].fixed_in, "1.3.2");
    }

    #[test]
    fn the_export_names_a_missing_column_and_a_short_record() {
        assert_eq!(
            parse_findings_csv("severity,package,used_in\nHigh,zlib,zlib/1\n"),
            Err(FindingsError::MissingColumn("vuln_id".to_string()))
        );
        assert_eq!(
            parse_findings_csv("severity,vuln_id,package,used_in\nHigh,CVE-1\n"),
            Err(FindingsError::ShortRecord {
                line: 2,
                found: 2,
                expected: 4
            })
        );
        assert_eq!(
            parse_findings_csv("severity,vuln_id,package,used_in\nHigh,\"CVE-1,zlib,a/1\n"),
            Err(FindingsError::UnclosedQuote(2))
        );
        assert_eq!(parse_findings_csv(""), Err(FindingsError::Empty));
    }

    #[test]
    fn a_module_name_maps_to_its_easyconfig_filename() {
        assert_eq!(
            easyconfig_filename("FFmpeg/7.0.2-GCCcore-13.3.0").as_deref(),
            Some("FFmpeg-7.0.2-GCCcore-13.3.0.eb")
        );
        assert_eq!(
            easyconfig_filename("OpenSSL/3").as_deref(),
            Some("OpenSSL-3.eb")
        );
        assert_eq!(easyconfig_filename("noslash"), None);
        assert_eq!(easyconfig_filename("Name/"), None);
        let installed: BTreeSet<String> = ["zlib/1.3.1".to_string(), "bad".to_string()].into();
        assert_eq!(
            installed_easystack(&installed).text,
            "easyconfigs:\n  - 'zlib-1.3.1.eb'\n"
        );
    }

    #[test]
    fn modules_are_ranked_by_score_per_in_place_rebuild() {
        let findings = parse_findings_csv(CSV).expect("parse");
        let installed: BTreeSet<String> = [
            "zlib/1.3.1-GCCcore-15.2.0",
            "PyApp/1.0-GCCcore-15.2.0",
            "libpng/1.6.50-GCCcore-15.2.0",
            "CMake/4.1.2-GCCcore-15.2.0",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(
            changed_easyconfigs(&findings, &installed),
            [
                "PyApp-1.0-GCCcore-15.2.0.eb",
                "zlib-1.3.1-GCCcore-15.2.0.eb"
            ]
        );
        let zlib = "zlib-1.3.1-GCCcore-15.2.0.eb";
        let rows = vec![
            row(
                zlib,
                "zlib/1.3.1-GCCcore-15.2.0",
                "changed",
                CLASS_MUST_REBUILD,
            ),
            row(
                zlib,
                "libpng/1.6.50-GCCcore-15.2.0",
                "runtime",
                CLASS_NO_REBUILD,
            ),
            row(
                &format!("PyApp-1.0-GCCcore-15.2.0.eb, {zlib}"),
                "CMake/4.1.2-GCCcore-15.2.0",
                "builddependency",
                CLASS_REVIEW,
            ),
        ];
        let summary = summarize(
            &findings,
            &installed,
            &document(rows, vec!["PyApp-1.0-GCCcore-15.2.0.eb".to_string()]),
        );
        assert_eq!(summary.rows, 4);
        assert_eq!(summary.ids, 3);
        assert_eq!(summary.modules, 2);
        assert_eq!(summary.not_installed, ["Gone/1.0"]);
        assert_eq!(summary.not_in_graph, ["PyApp/1.0-GCCcore-15.2.0"]);

        let first = &summary.ranked[0];
        assert_eq!(first.module, "zlib/1.3.1-GCCcore-15.2.0");
        assert_eq!(first.highest, "Critical");
        // CVE-2 is listed twice for zlib; the High row wins over the Low row.
        assert_eq!(first.counts.critical, 1);
        assert_eq!(first.counts.high, 1);
        assert_eq!(first.counts.low, 0);
        assert_eq!((first.fixed, first.unknown_fix), (1, 1));
        assert_eq!(first.score, 15);
        let closure = first.closure.as_ref().expect("closure");
        assert_eq!(
            (closure.must_rebuild, closure.review, closure.runtime),
            (1, 1, 1)
        );
        assert_eq!((closure.in_place, closure.bump), (2, 3));
        assert_eq!(first.value_per_rebuild, 7.5);

        let pyapp = &summary.ranked[1];
        assert!(pyapp.closure.is_none());
        assert_eq!(pyapp.not_fixed, 1);
        assert_eq!(pyapp.value_per_rebuild, 0.0);

        let critical = &summary.totals[0];
        assert_eq!(
            (critical.ids, critical.modules, critical.modules_at_or_above),
            (1, 1, 1)
        );
        assert_eq!(
            (critical.in_place_at_or_above, critical.bump_at_or_above),
            (2, 3)
        );
        let medium = &summary.totals[2];
        assert_eq!(medium.modules_highest, 1);
        assert_eq!(medium.modules_at_or_above, 2);
        assert_eq!(medium.bump_at_or_above, 4);

        let csv = render_csv(&summary);
        assert!(csv.starts_with("rank,module,"));
        assert!(csv.contains(
            "1,zlib/1.3.1-GCCcore-15.2.0,zlib-1.3.1-GCCcore-15.2.0.eb,Critical,1,1,0,0,0,2,1,0,1,15,1,1,1,2,3,7.50,zlib,"
        ));
        let markdown = render_markdown(&summary);
        assert!(markdown.contains("| 1 | `zlib/1.3.1-GCCcore-15.2.0` | Critical |"));
        assert!(markdown.contains("- `Gone/1.0`"));
    }
}
