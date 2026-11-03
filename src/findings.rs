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
//!
//! [`summarize`] also records a reach class for each module.
//! [`justifies_backport`] is true only for `cross-user` and `network-facing`.
//! The built-in names are the packages named for those two classes. Every
//! other name stays `unclassified` until a site file passed to
//! [`ReachTable::overlay`] says otherwise. [`render_backport`] is that list.

use crate::dep_filter::FilterDeps;
use crate::rebuild_set::{
    RebuildDocument, StackEasystack, CLASS_MUST_REBUILD, CLASS_NO_REBUILD, CLASS_REVIEW,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Schema of [`FindingsDocument`]. Bumped when a field changes meaning.
///
/// Version 2 adds `reach`, `backport`, `python_patch_in_place`, and
/// `fixed_in` on each ranked module, plus `backport_modules` and
/// `reachability_file` on the document.
pub const FINDINGS_SCHEMA_VERSION: u32 = 2;

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

/// A service another user on the same node can reach.
pub const REACH_CROSS_USER: &str = "cross-user";
/// A component that accepts network input.
pub const REACH_NETWORK: &str = "network-facing";
/// A hostile file in the job of the user who runs it.
///
/// The built-in table assigns this to nobody. A site file does.
pub const REACH_LOCAL: &str = "local-only";
/// Not reachable on Linux x86_64.
///
/// The built-in table assigns this to nobody. A site file does.
pub const REACH_NOT_REACHABLE: &str = "not-reachable";
/// No built-in name and no site file names this package or module.
pub const REACH_UNCLASSIFIED: &str = "unclassified";

/// Built-in package names, already normalized with [`reach_key`].
///
/// Jupyter server, JupyterLab, and notebook are cross-user on a shared node.
/// cURL, urllib3, aiohttp, tornado, cryptography, and git are network-facing.
const BUILTIN_REACH: &[(&str, &str)] = &[
    ("aiohttp", REACH_NETWORK),
    ("cryptography", REACH_NETWORK),
    ("curl", REACH_NETWORK),
    ("git", REACH_NETWORK),
    ("jupyter_lab", REACH_CROSS_USER),
    ("jupyter_server", REACH_CROSS_USER),
    ("jupyterlab", REACH_CROSS_USER),
    ("notebook", REACH_CROSS_USER),
    ("tornado", REACH_NETWORK),
    ("urllib3", REACH_NETWORK),
];

/// Lower is a stronger class. Cross-user wins over network-facing.
fn reach_rank(class: &str) -> u8 {
    match class {
        REACH_CROSS_USER => 0,
        REACH_NETWORK => 1,
        REACH_LOCAL => 2,
        REACH_NOT_REACHABLE => 3,
        _ => 4,
    }
}

/// `true` for the two classes that justify a backport.
pub fn justifies_backport(class: &str) -> bool {
    class == REACH_CROSS_USER || class == REACH_NETWORK
}

/// Normalize a package or module name for the reach table.
///
/// ASCII case is folded and `-` becomes `_`, so `cURL` and `jupyter-server`
/// match the built-in keys. The whole name has to match: `curl` does not
/// match `libcurl`, and `jupyterlab` does not match `jupyterlab_server`.
pub fn reach_key(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('-', "_")
}

/// Why a reachability file could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReachabilityError {
    /// One line is not `name class`.
    #[error("line {line}: {message}")]
    Line {
        /// Line number, counting from 1.
        line: usize,
        /// What is wrong with the line.
        message: String,
    },
}

/// Package and module names mapped to a reach class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachTable {
    names: BTreeMap<String, String>,
    site: BTreeMap<String, String>,
    /// Path of the site file. `None` when only the built-in names are used.
    pub source: Option<String>,
}

impl ReachTable {
    /// The built-in names and no site file.
    pub fn builtin() -> Self {
        let mut names = BTreeMap::new();
        for (name, class) in BUILTIN_REACH {
            names.insert((*name).to_string(), (*class).to_string());
        }
        Self {
            names,
            site: BTreeMap::new(),
            source: None,
        }
    }

    /// Built-in names, with `text` replacing or adding names.
    ///
    /// `source` is the path recorded on the findings document. A blank line
    /// or a line whose first non-whitespace character is `#` is skipped.
    /// Each other line is a name and one class, separated by whitespace.
    /// A name may appear once. The class is `cross-user`, `network-facing`,
    /// `local-only`, or `not-reachable`.
    pub fn overlay(
        &self,
        text: &str,
        source: impl Into<String>,
    ) -> Result<Self, ReachabilityError> {
        let site = parse_reachability(text)?;
        let mut names = self.names.clone();
        for (name, class) in &site {
            names.insert(name.clone(), class.clone());
        }
        Ok(Self {
            names,
            site,
            source: Some(source.into()),
        })
    }

    /// Names the site file set, in order.
    pub fn site_names(&self) -> Vec<(&str, &str)> {
        self.site
            .iter()
            .map(|(name, class)| (name.as_str(), class.as_str()))
            .collect()
    }

    /// Built-in names and the class the table uses for each, in name order.
    pub fn builtin_applied(&self) -> Vec<(&'static str, &str)> {
        let mut rows: Vec<(&str, &str)> = BUILTIN_REACH
            .iter()
            .map(|(name, builtin)| {
                let class = self
                    .names
                    .get(*name)
                    .map(String::as_str)
                    .unwrap_or(*builtin);
                (*name, class)
            })
            .collect();
        rows.sort_by(|left, right| left.0.cmp(right.0).then(left.1.cmp(right.1)));
        rows
    }

    /// The class of `module` given the scanner packages found in it.
    ///
    /// The module's software name (the text before `/`) and each package are
    /// looked up. The strongest class wins. No hit is [`REACH_UNCLASSIFIED`].
    pub fn class_of(&self, module: &str, packages: &[&str]) -> String {
        let mut best: Option<&str> = None;
        let mut consider = |name: &str| {
            let key = reach_key(name);
            if key.is_empty() {
                return;
            }
            let Some(class) = self.names.get(&key) else {
                return;
            };
            let class = class.as_str();
            best = Some(match best {
                Some(current) if reach_rank(current) <= reach_rank(class) => current,
                _ => class,
            });
        };
        if let Some(name) = module.split('/').next() {
            consider(name);
        }
        for package in packages {
            consider(package);
        }
        best.unwrap_or(REACH_UNCLASSIFIED).to_string()
    }
}

/// Reach class of each easyconfig filename.
///
/// The name is the toolchain split, then the same fallback as
/// [`crate::gap::software_name`]. The class is [`ReachTable::class_of`] on
/// that name. Filenames stay in the order `files` gives them only as map
/// keys; a [`std::collections::BTreeMap`] sorts them.
pub fn reach_for_easyconfigs(table: &ReachTable, files: &[String]) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|file| {
            let owned = crate::gap::software_name(file).unwrap_or_else(|| {
                file.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(file.as_str())
                    .to_string()
            });
            let class = table.class_of(&owned, &[owned.as_str()]);
            (file.clone(), class)
        })
        .collect()
}

fn parse_reachability(text: &str) -> Result<BTreeMap<String, String>, ReachabilityError> {
    let mut site = BTreeMap::new();
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(class) = parts.next() else {
            return Err(ReachabilityError::Line {
                line: line_no,
                message: format!("expected a name and a class, found `{line}`"),
            });
        };
        if parts.next().is_some() {
            return Err(ReachabilityError::Line {
                line: line_no,
                message: format!("expected a name and a class, found `{line}`"),
            });
        }
        let class = canonical_class(class).ok_or_else(|| ReachabilityError::Line {
            line: line_no,
            message: format!(
                "unknown class `{class}` (cross-user, network-facing, local-only, not-reachable)"
            ),
        })?;
        let key = reach_key(name);
        if key.is_empty() {
            return Err(ReachabilityError::Line {
                line: line_no,
                message: "the name is empty".to_string(),
            });
        }
        if site.insert(key, class.to_string()).is_some() {
            return Err(ReachabilityError::Line {
                line: line_no,
                message: format!("`{name}` is already listed"),
            });
        }
    }
    Ok(site)
}

fn canonical_class(class: &str) -> Option<&'static str> {
    match class.trim().to_ascii_lowercase().as_str() {
        REACH_CROSS_USER => Some(REACH_CROSS_USER),
        REACH_NETWORK => Some(REACH_NETWORK),
        REACH_LOCAL => Some(REACH_LOCAL),
        REACH_NOT_REACHABLE => Some(REACH_NOT_REACHABLE),
        _ => None,
    }
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
    /// The file is JSON and is not a Trivy, Grype, or OSV-scanner export.
    #[error("findings: {0}")]
    Scanner(String),
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
    /// Commit URLs the advisory names. Empty when the export names none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fix_commits: Vec<String>,
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
    let fix_commits = index("fix_commits");
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
            fix_commits: split_list(&get(fix_commits)),
        });
    }
    Ok(findings)
}

/// Read a scanner export.
///
/// A file that starts with `{` or `[` is Trivy (`Results`), Grype
/// (`matches`), or OSV-scanner (`results` of packages). Anything else is
/// the comma-separated export [`parse_findings_csv`] reads. A JSON row has
/// no EasyBuild module column: `installed` supplies `used_in` by matching
/// the package name and version to `Name/version-toolchain`.
pub fn parse_findings(
    text: &str,
    installed: &BTreeSet<String>,
) -> Result<Vec<Finding>, FindingsError> {
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return parse_findings_json(trimmed, installed);
    }
    parse_findings_csv(text)
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
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
    /// `cross-user`, `network-facing`, `local-only`, `not-reachable`, or
    /// `unclassified`.
    pub reach: String,
    /// [`justifies_backport`] of [`Self::reach`].
    pub backport: bool,
    /// The module is the Python interpreter, and every named fix stays on
    /// the installed minor. Applying that patch release under the same
    /// module path costs [`ModuleClosure::in_place`]. Renaming the
    /// easyconfig to the new patch version costs [`ModuleClosure::bump`].
    pub python_patch_in_place: bool,
    /// Fix versions the export names for this module, sorted and unique.
    pub fixed_in: Vec<String>,
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
    /// Ranked modules whose reach class justifies a backport.
    pub backport_modules: usize,
    /// Site reachability file. Omitted when the built-in names were used alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachability_file: Option<String>,
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
///
/// `reach` classifies each module. A site file is an overlay on the built-in
/// names; [`ReachTable::builtin`] leaves every other module unclassified.
pub fn summarize(
    findings: &[Finding],
    installed: &BTreeSet<String>,
    document: &RebuildDocument,
    reach: &ReachTable,
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
        let package_names: Vec<String> = packages
            .get(module)
            .map(|set| set.iter().map(|name| name.to_string()).collect())
            .unwrap_or_default();
        let package_refs: Vec<&str> = package_names.iter().map(String::as_str).collect();
        let reach_class = reach.class_of(module, &package_refs);
        ranked.push(ModuleFindings {
            module: module.to_string(),
            easyconfig,
            highest: SEVERITIES.get(highest).unwrap_or(&"unknown").to_string(),
            counts,
            findings: by_id.len(),
            fixed,
            not_fixed,
            unknown_fix,
            packages: package_names,
            score,
            closure,
            value_per_rebuild,
            backport: justifies_backport(&reach_class),
            reach: reach_class,
            python_patch_in_place: python_patch_in_place(module, findings),
            fixed_in: fixed_versions(module, findings),
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

    let backport_modules = ranked.iter().filter(|row| row.backport).count();
    FindingsDocument {
        schema_version: FINDINGS_SCHEMA_VERSION,
        generated_at: document.generated_at.clone(),
        installed_source: document.installed_source.clone(),
        rows: findings.len(),
        ids: ids.len(),
        modules: per_module.len(),
        totals,
        ranked,
        backport_modules,
        reachability_file: reach.source.clone(),
        not_installed: not_installed.into_iter().collect(),
        not_in_graph,
    }
}

/// Major and minor of `version`, dropping a toolchain suffix.
///
/// `3.11.5-GCCcore-15.2.0` is `(3, 11)`. A version with fewer than two
/// numeric components has no minor to compare.
fn version_minor(version: &str) -> Option<(u64, u64)> {
    let mut nums = Vec::new();
    for part in crate::version::parse_version_parts(version) {
        if let crate::version::Part::Num(value) = part {
            nums.push(value);
            if nums.len() == 2 {
                return Some((nums[0], nums[1]));
            }
        }
    }
    None
}

/// Python interpreters take a patch release in place when the fix stays on
/// the same minor. A different minor, a missing fix version, or any other
/// module is false.
fn python_patch_in_place(module: &str, findings: &[Finding]) -> bool {
    let Some(software) = module.split('/').next() else {
        return false;
    };
    if reach_key(software) != "python" {
        return false;
    }
    let module_version = module.split_once('/').map(|(_, rest)| rest).unwrap_or("");
    let mut saw = false;
    for finding in findings {
        if !finding.modules.iter().any(|item| item == module) {
            continue;
        }
        let fixed = finding.fixed_in.trim();
        if fixed.is_empty() {
            continue;
        }
        let installed = if finding.version.trim().is_empty() {
            module_version
        } else {
            finding.version.trim()
        };
        let (Some(have), Some(want)) = (version_minor(installed), version_minor(fixed)) else {
            return false;
        };
        if have != want {
            return false;
        }
        saw = true;
    }
    saw
}

fn fixed_versions(module: &str, findings: &[Finding]) -> Vec<String> {
    let mut versions = BTreeSet::new();
    for finding in findings {
        if finding.modules.iter().any(|item| item == module) {
            let fixed = finding.fixed_in.trim();
            if !fixed.is_empty() {
                versions.insert(fixed.to_string());
            }
        }
    }
    versions.into_iter().collect()
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
         score per in-place rebuild. Reach and the backport list are in `backport.md`. \
         `fix-cost.md` prices each advisory on its own: the bump closure, the \
         in-place patch, and the fraction of the stack each strategy rebuilds.\n\n",
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
        "\n| Rank | Module | Reach | Highest | C | H | M | L | Fixed | Score | In place | Bump | Value |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for (rank, row) in document.ranked.iter().enumerate() {
        let (in_place, bump) = cost_pair(row);
        out.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} | {} | {} | {}/{} | {} | {} | {} | {:.2} |\n",
            rank + 1,
            row.module,
            row.reach,
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
        "rank,module,easyconfig,highest,critical,high,medium,low,negligible,findings,fixed,not_fixed,unknown_fix,score,must_rebuild,review,runtime,in_place,bump,value_per_rebuild,packages,dependents,reach,backport,python_patch_in_place,fixed_in\n",
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
            row.reach.clone(),
            row.backport.to_string(),
            row.python_patch_in_place.to_string(),
            row.fixed_in.join(";"),
        ];
        let line: Vec<String> = fields.iter().map(|field| csv_field(field)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

fn cost_pair(row: &ModuleFindings) -> (String, String) {
    row.closure
        .as_ref()
        .map(|closure| (closure.in_place.to_string(), closure.bump.to_string()))
        .unwrap_or_else(|| ("-".to_string(), "-".to_string()))
}

/// The backport list: modules whose reach class justifies a backport, the
/// modules a site file recorded and left off that list, and the modules
/// still unclassified.
pub fn render_backport(
    document: &FindingsDocument,
    reach: &ReachTable,
    findings: &[Finding],
    filter: &FilterDeps,
) -> String {
    let mut out = String::new();
    out.push_str("# Backport list\n\n");
    out.push_str(&format!(
        "Computed {} against the modules installed for `{}`.\n\n",
        document.generated_at, document.installed_source
    ));
    out.push_str(
        "A module is on this list when its reach class is `cross-user` or \
         `network-facing`. Those two classes justify a backport. `local-only` \
         and `not-reachable` are recorded below and left off the list. A module \
         the built-in names and the site file do not mention is `unclassified`. \
         That is not a judgment about the finding.\n\n",
    );
    out.push_str(
        "In place is the patched module, the bundles that ship it, and the \
         review dependents. Bump is the module and every installed dependent \
         in its toolchain generation. `rebuilds/*-cve-patches.yml` still lists \
         every must-rebuild module from the export.\n\n",
    );
    match &document.reachability_file {
        Some(path) => {
            out.push_str(&format!(
                "Classes come from the built-in names, with `{path}` replacing or adding names.\n\n"
            ));
        }
        None => {
            out.push_str(
                "Classes come from the built-in names. `--reachability` adds a name or replaces one.\n\n",
            );
        }
    }
    out.push_str("| Name | Class |\n|---|---|\n");
    for (name, class) in reach.builtin_applied() {
        out.push_str(&format!("| `{name}` | {class} |\n"));
    }
    let site = reach.site_names();
    if !site.is_empty() {
        out.push_str("\nSite file:\n\n| Name | Class |\n|---|---|\n");
        for (name, class) in site {
            out.push_str(&format!("| `{name}` | {class} |\n"));
        }
    }
    out.push_str("\n## Backport\n\n");
    let chosen: Vec<&ModuleFindings> = document.ranked.iter().filter(|row| row.backport).collect();
    if chosen.is_empty() {
        out.push_str("No module in this export is `cross-user` or `network-facing`.\n");
    } else {
        out.push_str("| Module | Reach | Highest | In place | Bump | Packages |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for row in chosen {
            let (in_place, bump) = cost_pair(row);
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                row.module,
                row.reach,
                row.highest,
                in_place,
                bump,
                row.packages.join(", ")
            ));
        }
    }
    out.push_str("\n## Recorded, not a backport\n\n");
    let recorded: Vec<&ModuleFindings> = document
        .ranked
        .iter()
        .filter(|row| row.reach == REACH_LOCAL || row.reach == REACH_NOT_REACHABLE)
        .collect();
    if recorded.is_empty() {
        out.push_str("No module is `local-only` or `not-reachable`.\n");
    } else {
        out.push_str("| Module | Reach | Highest | In place | Bump | Packages |\n");
        out.push_str("|---|---|---|---|---|---|\n");
        for row in recorded {
            let (in_place, bump) = cost_pair(row);
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                row.module,
                row.reach,
                row.highest,
                in_place,
                bump,
                row.packages.join(", ")
            ));
        }
    }
    out.push_str("\n## Unclassified\n\n");
    let open: Vec<&ModuleFindings> = document
        .ranked
        .iter()
        .filter(|row| row.reach == REACH_UNCLASSIFIED)
        .collect();
    if open.is_empty() {
        out.push_str("Every module with a finding has a reach class.\n");
    } else {
        out.push_str(
            "These modules are not on the backport list. Add a line to the \
             `--reachability` file to classify one.\n\n",
        );
        for row in open {
            let (in_place, bump) = cost_pair(row);
            out.push_str(&format!(
                "- `{}` ({}, in place {in_place}, bump {bump})\n",
                row.module, row.highest
            ));
        }
    }
    out.push_str("\n## Python patch releases\n\n");
    let interpreters: Vec<&ModuleFindings> = document
        .ranked
        .iter()
        .filter(|row| row.python_patch_in_place)
        .collect();
    if interpreters.is_empty() {
        out.push_str(
            "No Python interpreter in this export has a fix that stays on the installed minor.\n",
        );
    } else {
        out.push_str(
            "The ABI is stable within a minor, so the patch release can be applied \
             under the same module path. That costs the in-place column. Renaming \
             the easyconfig to the new patch version costs the bump column. This \
             note does not by itself put the module on the backport list.\n\n",
        );
        for row in interpreters {
            let (in_place, bump) = cost_pair(row);
            out.push_str(&format!(
                "- `{}` fixed in {} (in place {in_place}, bump {bump})\n",
                row.module,
                row.fixed_in.join(", ")
            ));
        }
    }
    append_os_errata(&mut out, document, filter);
    append_scaffolds(&mut out, document, findings, filter);
    append_curl_note(&mut out, document, findings, filter);
    out
}

fn software_name(module: &str) -> &str {
    module.split('/').next().unwrap_or(module)
}

/// Software version, without the toolchain suffix of a module string.
fn software_version(module: &str, stated: &str) -> String {
    let stated = stated.trim();
    if !stated.is_empty() {
        return stated.to_string();
    }
    let rest = module.split_once('/').map(|(_, rest)| rest).unwrap_or("");
    let mut version = String::new();
    for (index, part) in rest.split('-').enumerate() {
        if part.is_empty() || (index > 0 && !part.starts_with(|c: char| c.is_ascii_digit())) {
            break;
        }
        if !version.is_empty() {
            version.push('-');
        }
        version.push_str(part);
    }
    version
}

fn patch_token(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn module_is_filtered(filter: &FilterDeps, module: &str) -> bool {
    let name = software_name(module);
    let version = software_version(module, "");
    filter.dep_is_filtered(name, &version)
}

fn curl_package(name: &str) -> bool {
    matches!(reach_key(name).as_str(), "curl" | "libcurl")
}

const CURL_FILTER_NOTE: &str = "\
--filter-deps=cURL changes only builds that have not been installed yet. \
It drops the EasyBuild cURL module and takes cURL from the host or the EESSI \
compat layer, so later cURL fixes come from the operating-system errata stream. \
The cost is the newer protocol features that the EasyBuild cURL build carries. \
Decide it for the new stack. It does not rewrite modules that are already installed.";

fn append_os_errata(out: &mut String, document: &FindingsDocument, filter: &FilterDeps) {
    out.push_str("\n## OS errata\n\n");
    let rows: Vec<&ModuleFindings> = document
        .ranked
        .iter()
        .filter(|row| row.backport && module_is_filtered(filter, &row.module))
        .collect();
    if rows.is_empty() {
        out.push_str("No backport module is dropped by `--filter-deps`.\n");
        return;
    }
    out.push_str(
        "`--filter-deps` drops these modules. Their reach class would justify a \
         backport, and the fix is the operating-system errata stream rather than \
         a site patch. No scaffold is written for them. `backport` on the findings \
         document stays the reach class.\n\n",
    );
    for row in rows {
        out.push_str(&format!("- `{}`\n", row.module));
    }
}

fn append_scaffolds(
    out: &mut String,
    document: &FindingsDocument,
    findings: &[Finding],
    filter: &FilterDeps,
) {
    out.push_str("\n## Patch scaffolds\n\n");
    out.push_str(
        "Each scaffold is an in-place patch: the same module name and version. \
         The checksum line is filled with the sha256 of the patch file once that \
         file exists. Checksums stay in source order, then patch order.\n\n",
    );
    let mut wrote = false;
    let mut seen = BTreeSet::new();
    for row in document.ranked.iter().filter(|row| row.backport) {
        if module_is_filtered(filter, &row.module) {
            continue;
        }
        for finding in findings {
            if !finding.modules.iter().any(|module| module == &row.module) {
                continue;
            }
            let id = finding.vuln_id.trim();
            if id.is_empty() || !seen.insert((row.module.clone(), id.to_string())) {
                continue;
            }
            wrote = true;
            let name = scaffold_name(&row.module, &finding.package);
            let version = software_version(&row.module, &finding.version);
            let filename = format!("{}-{}_{}.patch", name, version, patch_token(id));
            out.push_str(&format!("### `{filename}`\n\n"));
            if let Some(file) = easyconfig_filename(&row.module) {
                out.push_str(&format!("Edit `{file}`.\n\n"));
            }
            out.push_str("```python\n");
            out.push_str(&format!("patches = ['{filename}']\n"));
            out.push_str("checksums = [\n");
            out.push_str("    # existing source checksums stay ahead of the patch checksums\n");
            out.push_str(&format!("    # {filename}\n"));
            out.push_str("    '<sha256 of the patch file>',\n");
            out.push_str("]\n```\n\n");
            if finding.fixed_in.trim().is_empty() {
                out.push_str("The export names no fixed version.\n\n");
            } else {
                out.push_str(&format!("Fixed in {}.\n\n", finding.fixed_in.trim()));
            }
            if finding.fix_commits.is_empty() {
                out.push_str("The advisory names no fix commit URL.\n\n");
            } else {
                out.push_str("Upstream fix commit:\n\n");
                for url in &finding.fix_commits {
                    out.push_str(&format!("- {url}\n"));
                }
                out.push('\n');
            }
        }
    }
    if !wrote {
        out.push_str("No in-place patch was chosen.\n");
    }
}

fn scaffold_name(module: &str, package: &str) -> String {
    let software = software_name(module);
    if package.is_empty() || reach_key(software) == reach_key(package) {
        software.to_string()
    } else {
        package.to_string()
    }
}

fn append_curl_note(
    out: &mut String,
    document: &FindingsDocument,
    findings: &[Finding],
    filter: &FilterDeps,
) {
    let hit = findings.iter().any(|finding| {
        curl_package(&finding.package)
            || finding
                .modules
                .iter()
                .any(|module| curl_package(software_name(module)))
    }) || document.ranked.iter().any(|row| {
        curl_package(software_name(&row.module))
            || row.packages.iter().any(|package| curl_package(package))
    });
    if !hit {
        return;
    }
    out.push_str("\n## cURL and --filter-deps\n\n");
    out.push_str(CURL_FILTER_NOTE);
    out.push('\n');
    if filter.dep_is_filtered("cURL", "")
        || filter.dep_is_filtered("curl", "")
        || filter.dep_is_filtered("libcurl", "")
    {
        out.push_str(
            "\ncURL is in the filter-deps list for this run, so these findings are on the OS errata stream.\n",
        );
    }
}

fn parse_findings_json(
    text: &str,
    installed: &BTreeSet<String>,
) -> Result<Vec<Finding>, FindingsError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| FindingsError::Scanner(format!("scanner JSON: {error}")))?;
    let raw = if value
        .get("matches")
        .and_then(|item| item.as_array())
        .is_some()
    {
        parse_grype(&value)
    } else if value
        .get("Results")
        .and_then(|item| item.as_array())
        .is_some()
    {
        parse_trivy(&value)
    } else if value
        .get("results")
        .and_then(|item| item.as_array())
        .is_some_and(|results| {
            results.iter().any(|result| {
                result
                    .get("packages")
                    .and_then(|item| item.as_array())
                    .is_some()
            })
        })
    {
        parse_osv_scanner(&value)
    } else {
        return Err(FindingsError::Scanner(
            "scanner JSON is not Trivy (Results), Grype (matches), or OSV-scanner (results)"
                .to_string(),
        ));
    };
    Ok(raw
        .into_iter()
        .map(|mut finding| {
            if finding.modules.is_empty() {
                finding.modules = modules_for(&finding.package, &finding.version, installed);
            }
            finding
        })
        .collect())
}

fn modules_for(package: &str, version: &str, installed: &BTreeSet<String>) -> Vec<String> {
    let mut hits = Vec::new();
    for spec in installed {
        let Some((name, rest)) = spec.split_once('/') else {
            continue;
        };
        if reach_key(name) != reach_key(package) {
            continue;
        }
        if version.is_empty()
            || rest == version
            || rest
                .strip_prefix(version)
                .is_some_and(|tail| tail.is_empty() || tail.starts_with('-'))
        {
            hits.push(spec.clone());
        }
    }
    hits.sort();
    hits.dedup();
    hits
}

fn parse_trivy(value: &serde_json::Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(results) = value.get("Results").and_then(|item| item.as_array()) else {
        return out;
    };
    for result in results {
        let Some(vulns) = result
            .get("Vulnerabilities")
            .and_then(|item| item.as_array())
        else {
            continue;
        };
        for vuln in vulns {
            let Some(id) = json_text(vuln, "VulnerabilityID") else {
                continue;
            };
            out.push(Finding {
                severity: json_text(vuln, "Severity").unwrap_or_default(),
                vuln_id: id,
                package: json_text(vuln, "PkgName").unwrap_or_default(),
                version: json_text(vuln, "InstalledVersion").unwrap_or_default(),
                fixed_in: json_text(vuln, "FixedVersion")
                    .unwrap_or_default()
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                fix_state: String::new(),
                modules: Vec::new(),
                fix_commits: json_urls(vuln, "References")
                    .into_iter()
                    .filter(|url| commit_url_like(url))
                    .collect(),
            });
        }
    }
    out
}

fn parse_grype(value: &serde_json::Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(matches) = value.get("matches").and_then(|item| item.as_array()) else {
        return out;
    };
    for matched in matches {
        let vuln = matched
            .get("vulnerability")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let artifact = matched
            .get("artifact")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        let Some(id) = json_text(&vuln, "id") else {
            continue;
        };
        let fixed_in = vuln
            .pointer("/fix/versions")
            .and_then(|item| item.as_array())
            .and_then(|items| items.first())
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string();
        out.push(Finding {
            severity: json_text(&vuln, "severity").unwrap_or_default(),
            vuln_id: id,
            package: json_text(&artifact, "name").unwrap_or_default(),
            version: json_text(&artifact, "version").unwrap_or_default(),
            fixed_in,
            fix_state: String::new(),
            modules: Vec::new(),
            fix_commits: json_urls(&vuln, "urls")
                .into_iter()
                .filter(|url| commit_url_like(url))
                .collect(),
        });
    }
    out
}

fn parse_osv_scanner(value: &serde_json::Value) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(results) = value.get("results").and_then(|item| item.as_array()) else {
        return out;
    };
    for result in results {
        let Some(packages) = result.get("packages").and_then(|item| item.as_array()) else {
            continue;
        };
        for package in packages {
            let meta = package
                .get("package")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let name = json_text(&meta, "name").unwrap_or_default();
            let version = json_text(&meta, "version").unwrap_or_default();
            let Some(vulns) = package
                .get("vulnerabilities")
                .or_else(|| package.get("vulns"))
                .and_then(|item| item.as_array())
            else {
                continue;
            };
            for vuln in vulns {
                let Some(id) = json_text(vuln, "id") else {
                    continue;
                };
                out.push(Finding {
                    severity: String::new(),
                    vuln_id: id,
                    package: name.clone(),
                    version: version.clone(),
                    fixed_in: osv_fixed_versions(vuln)
                        .into_iter()
                        .next()
                        .unwrap_or_default(),
                    fix_state: String::new(),
                    modules: Vec::new(),
                    fix_commits: osv_fix_commits(vuln),
                });
            }
        }
    }
    out
}

fn json_text(value: &serde_json::Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
}

fn json_urls(value: &serde_json::Value, field: &str) -> Vec<String> {
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

fn commit_url_like(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("/commit/") || lower.contains("/commits/")
}

fn osv_fixed_versions(document: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(affected) = document.get("affected").and_then(|value| value.as_array()) else {
        return out;
    };
    for entry in affected {
        let Some(ranges) = entry.get("ranges").and_then(|value| value.as_array()) else {
            continue;
        };
        for range in ranges {
            let kind = range
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if kind != "SEMVER" && kind != "ECOSYSTEM" {
                continue;
            }
            let Some(events) = range.get("events").and_then(|value| value.as_array()) else {
                continue;
            };
            for event in events {
                if let Some(fixed) = event.get("fixed").and_then(|value| value.as_str()) {
                    let fixed = fixed.trim();
                    if !fixed.is_empty() {
                        out.push(fixed.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn osv_fix_commits(document: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(references) = document
        .get("references")
        .and_then(|value| value.as_array())
    {
        for reference in references {
            let Some(url) = reference.get("url").and_then(|value| value.as_str()) else {
                continue;
            };
            if commit_url_like(url) {
                out.push(url.to_string());
            }
        }
    }
    if let Some(affected) = document.get("affected").and_then(|value| value.as_array()) {
        for entry in affected {
            let Some(ranges) = entry.get("ranges").and_then(|value| value.as_array()) else {
                continue;
            };
            for range in ranges {
                if range.get("type").and_then(|value| value.as_str()) != Some("GIT") {
                    continue;
                }
                let Some(repo) = range.get("repo").and_then(|value| value.as_str()) else {
                    continue;
                };
                let Some(events) = range.get("events").and_then(|value| value.as_array()) else {
                    continue;
                };
                for event in events {
                    let Some(sha) = event.get("fixed").and_then(|value| value.as_str()) else {
                        continue;
                    };
                    if (7..=40).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                        let repo = repo.trim().trim_end_matches('/').trim_end_matches(".git");
                        out.push(format!("{repo}/commit/{sha}"));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
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
            reach: BTreeMap::new(),
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
            &ReachTable::builtin(),
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
        assert_eq!(first.reach, REACH_UNCLASSIFIED);
        assert!(!first.backport);
        assert!(!first.python_patch_in_place);
        assert_eq!(first.fixed_in, ["1.3.2"]);
        assert_eq!(summary.backport_modules, 0);
        assert!(summary.reachability_file.is_none());

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
        assert!(markdown.contains("| 1 | `zlib/1.3.1-GCCcore-15.2.0` | unclassified | Critical |"));
        assert!(markdown.contains("- `Gone/1.0`"));
        let backport = render_backport(
            &summary,
            &ReachTable::builtin(),
            &findings,
            &FilterDeps::empty(),
        );
        assert!(backport.contains("## Unclassified"));
        assert!(backport.contains("`zlib/1.3.1-GCCcore-15.2.0`"));
        assert!(backport.contains("No module in this export is `cross-user` or `network-facing`."));
    }

    #[test]
    fn reach_classes_match_whole_names_and_a_site_file_replaces_one() {
        let table = ReachTable::builtin();
        assert_eq!(
            table.class_of("cURL/8.17.0-GCCcore-15.2.0", &["libcurl"]),
            REACH_NETWORK
        );
        assert_eq!(table.class_of("libcurl/8.17.0", &[]), REACH_UNCLASSIFIED);
        assert_eq!(
            table.class_of("jupyter-server/2.19.0", &[]),
            REACH_CROSS_USER
        );
        assert_eq!(
            table.class_of("jupyterlab-server/2.28.0", &[]),
            REACH_UNCLASSIFIED
        );
        assert_eq!(
            table.class_of("git/2.52.0-GCCcore-15.2.0", &[]),
            REACH_NETWORK
        );
        assert_eq!(
            table.class_of("SciPy-bundle/2026.05", &["urllib3"]),
            REACH_NETWORK
        );
        assert_eq!(
            table.class_of("Bundle/1", &["curl", "notebook"]),
            REACH_CROSS_USER
        );

        let over = table
            .overlay(
                "# site\n\nCURL local-only\njupyterlab-server cross-user\n",
                "site.txt",
            )
            .expect("overlay");
        assert_eq!(over.class_of("cURL/8.17.0", &["curl"]), REACH_LOCAL);
        assert_eq!(
            over.class_of("jupyterlab-server/2.28.0", &[]),
            REACH_CROSS_USER
        );
        assert_eq!(
            over.class_of("Bundle/1", &["curl", "git"]),
            REACH_NETWORK,
            "git stays network-facing when curl is reclassified"
        );
        assert_eq!(
            table.overlay("curl nope\n", "site.txt").unwrap_err(),
            ReachabilityError::Line {
                line: 1,
                message:
                    "unknown class `nope` (cross-user, network-facing, local-only, not-reachable)"
                        .to_string(),
            }
        );
        assert_eq!(
            table
                .overlay("curl network-facing\ncurl local-only\n", "site.txt")
                .unwrap_err(),
            ReachabilityError::Line {
                line: 2,
                message: "`curl` is already listed".to_string(),
            }
        );
    }

    #[test]
    fn a_python_patch_release_on_the_same_minor_stays_in_place() {
        let csv = "severity,vuln_id,package,version,fixed_in,fix_state,used_in\n\
High,CVE-9,Python,3.11.5,3.11.7,fixed,Python/3.11.5-GCCcore-15.2.0\n\
High,CVE-8,Python,3.11.5,3.12.0,fixed,Python/3.12.0-GCCcore-15.2.0\n\
Low,CVE-7,Python,3.10.1,,not-fixed,Python/3.10.1-GCCcore-15.2.0\n";
        let findings = parse_findings_csv(csv).expect("parse");
        let installed: BTreeSet<String> = [
            "Python/3.11.5-GCCcore-15.2.0",
            "Python/3.12.0-GCCcore-15.2.0",
            "Python/3.10.1-GCCcore-15.2.0",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let same = "Python-3.11.5-GCCcore-15.2.0.eb";
        let summary = summarize(
            &findings,
            &installed,
            &document(
                vec![row(
                    same,
                    "Python/3.11.5-GCCcore-15.2.0",
                    "changed",
                    CLASS_MUST_REBUILD,
                )],
                vec![
                    "Python-3.12.0-GCCcore-15.2.0.eb".to_string(),
                    "Python-3.10.1-GCCcore-15.2.0.eb".to_string(),
                ],
            ),
            &ReachTable::builtin(),
        );
        let by_module = |name: &str| {
            summary
                .ranked
                .iter()
                .find(|row| row.module == name)
                .unwrap_or_else(|| panic!("{name}"))
        };
        let patch = by_module("Python/3.11.5-GCCcore-15.2.0");
        assert!(patch.python_patch_in_place);
        assert_eq!(patch.fixed_in, ["3.11.7"]);
        assert_eq!(patch.reach, REACH_UNCLASSIFIED);
        assert!(!by_module("Python/3.12.0-GCCcore-15.2.0").python_patch_in_place);
        assert!(!by_module("Python/3.10.1-GCCcore-15.2.0").python_patch_in_place);
        let backport = render_backport(
            &summary,
            &ReachTable::builtin(),
            &findings,
            &FilterDeps::empty(),
        );
        assert!(backport.contains("`Python/3.11.5-GCCcore-15.2.0` fixed in 3.11.7"));
        assert!(backport.contains("No module in this export is `cross-user` or `network-facing`."));
        assert!(backport.contains("No in-place patch was chosen."));
    }

    #[test]
    fn a_json_export_maps_onto_installed_modules_and_scaffolds_the_patch() {
        let installed: BTreeSet<String> = [
            "jupyter_server/9.9.9-GCCcore-15.2.0".to_string(),
            "cURL/9.8.0-GCCcore-15.2.0".to_string(),
            "zlib/1.3.1-GCCcore-15.2.0".to_string(),
        ]
        .into();
        let text = r#"{
          "Results": [
            {"Vulnerabilities": [
              {
                "VulnerabilityID": "SYNTH-2099-0101",
                "PkgName": "jupyter-server",
                "InstalledVersion": "9.9.9",
                "FixedVersion": "9.9.10",
                "Severity": "CRITICAL",
                "References": ["https://example.invalid/jupyter-server/commit/0123456789abcdef0123456789abcdef01234567"]
              }
            ]},
            {"Vulnerabilities": [
              {
                "VulnerabilityID": "SYNTH-2099-0102",
                "PkgName": "curl",
                "InstalledVersion": "9.8.0",
                "FixedVersion": "9.8.1",
                "Severity": "HIGH"
              }
            ]}
          ]
        }"#;
        let findings = parse_findings(text, &installed).expect("json");
        assert_eq!(findings[0].modules, ["jupyter_server/9.9.9-GCCcore-15.2.0"]);
        assert_eq!(
            findings[0].fix_commits,
            ["https://example.invalid/jupyter-server/commit/0123456789abcdef0123456789abcdef01234567"]
        );
        assert_eq!(findings[1].modules, ["cURL/9.8.0-GCCcore-15.2.0"]);
        let summary = summarize(
            &findings,
            &installed,
            &document(Vec::new(), Vec::new()),
            &ReachTable::builtin(),
        );
        assert_eq!(summary.backport_modules, 2);
        let report = render_backport(
            &summary,
            &ReachTable::builtin(),
            &findings,
            &FilterDeps::empty(),
        );
        assert!(report.contains("### `jupyter_server-9.9.9_SYNTH-2099-0101.patch`"));
        assert!(report.contains("patches = ['jupyter_server-9.9.9_SYNTH-2099-0101.patch']"));
        assert!(report.contains("'<sha256 of the patch file>'"));
        assert!(report.contains("https://example.invalid/jupyter-server/commit/0123456789abcdef0123456789abcdef01234567"));
        assert!(report.contains("### `cURL-9.8.0_SYNTH-2099-0102.patch`"));
        assert!(report.contains("## cURL and --filter-deps"));
        assert!(report.contains("--filter-deps=cURL"));
        let filtered = FilterDeps::parse(&["cURL".into()]).unwrap();
        let held = render_backport(&summary, &ReachTable::builtin(), &findings, &filtered);
        assert!(held.contains("## OS errata"));
        assert!(held.contains("`cURL/9.8.0-GCCcore-15.2.0`"));
        assert!(!held.contains("### `cURL-9.8.0_SYNTH-2099-0102.patch`"));
        assert!(held.contains("cURL is in the filter-deps list"));
        assert!(parse_findings(r#"{"hello":1}"#, &installed).is_err());
    }
}
