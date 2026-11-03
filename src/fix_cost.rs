//! What each security finding costs to fix, two ways.
//!
//! [`report`] reads a scanner export and a [`RebuildDocument`] for the same
//! installed stack. It does not walk the dependency graph again.
//!
//! * A version bump is the patched module plus every installed reverse
//!   dependent. The module path changes, so the deployed stack is no longer
//!   the one that was built. That breaks reproducibility.
//! * An in-place patch keeps the name and version and adds
//!   `<Name>-<version>_<ID>.patch` to `patches`. It rebuilds the patched
//!   module, a bundle that includes the package as an extension or as a
//!   `components` entry, and a dependent that links it statically.
//!
//! A runtime dependency is a shared-library link. A builddependency-only
//! dependent is a static link when the recipe asks for a static build, or
//! when `rebuild-set` already proved it from the install prefix. Otherwise
//! the report says the link cannot be known, and that dependent is not
//! counted in the in-place total.

use crate::domain::Candidate;
use crate::findings::{
    easyconfig_filename, justifies_backport, severity_rank, Finding, ReachTable, REACH_CROSS_USER,
    REACH_LOCAL, REACH_NETWORK, REACH_NOT_REACHABLE, REACH_UNCLASSIFIED, SEVERITIES,
};
use crate::rebuild_set::{RebuildDocument, RebuildRow, CLASS_NO_REBUILD};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Schema of [`FixCostDocument`]. Bumped when a field changes meaning.
pub const FIX_COST_SCHEMA_VERSION: u32 = 1;

/// Recommended route: keep the version and add a patch file.
pub const ROUTE_IN_PLACE: &str = "in-place";
/// Recommended route: take the new version and rebuild the closure.
pub const ROUTE_BUMP: &str = "bump";
/// Recommended route: do not backport.
pub const ROUTE_DEFER: &str = "defer";
/// The easyconfig is not in the deployed graph, so neither cost is known.
pub const ROUTE_UNKNOWN: &str = "unknown";

/// What one recipe says about how it links, read from `configopts` and
/// `buildopts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkHint {
    /// The recipe asks for a static build and not a shared one.
    Static,
    /// The recipe asks for a shared build and not a static one.
    Shared,
    /// The recipe names both, or neither.
    Unknown,
}

/// One installed recipe, as far as the fix-cost report needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipeEvidence {
    /// Easyconfig filename.
    pub easyconfig: String,
    /// Installed module, `Name/VersionFull`.
    pub module: String,
    /// `(name, version)` from `components`.
    pub components: Vec<(String, String)>,
    /// How `configopts` and `buildopts` say this recipe links.
    pub link: LinkHint,
}

/// Recipes read for [`report`], and paths that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EvidenceScan {
    /// One entry per installed easyconfig that was read.
    pub recipes: Vec<RecipeEvidence>,
    /// Unreadable paths, and component entries whose version was not a literal.
    pub notes: Vec<String>,
}

/// Modules one route rebuilds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RouteCost {
    /// How many modules.
    pub modules: usize,
    /// `modules / stack_modules`, four decimal places.
    pub fraction: f64,
    /// Module names, sorted.
    pub names: Vec<String>,
}

/// The version-bump route.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BumpCost {
    /// How many modules.
    pub modules: usize,
    /// `modules / stack_modules`, four decimal places.
    pub fraction: f64,
    /// Module names, sorted.
    pub names: Vec<String>,
    /// A new version is a new module path. The deployed stack changes.
    pub breaks_reproducibility: bool,
}

/// One finding, with both costs and a route.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingRoute {
    /// Advisory id from the export.
    pub vuln_id: String,
    /// Package the scanner matched.
    pub package: String,
    /// Installed version from the export.
    pub version: String,
    /// Worst severity among the export rows for this finding.
    pub severity: String,
    /// Version that fixes it. Empty when the scanner names none.
    pub fixed_in: String,
    /// `fixed`, `not-fixed`, or empty.
    pub fix_state: String,
    /// The class from [`ReachTable`]: `cross-user`, `network-facing`,
    /// `local-only`, `not-reachable`, or `unclassified`.
    pub reach: String,
    /// Installed modules that carry it, sorted.
    pub modules: Vec<String>,
    /// Installed modules named by the export whose easyconfig is not in the graph.
    pub not_in_graph: Vec<String>,
    /// `<Name>-<version>_<ID>.patch`, one per distinct name and version.
    pub patch_files: Vec<String>,
    /// Full reverse-dependency closure.
    pub bump: BumpCost,
    /// Patched module, bundles, and known static linkers.
    pub in_place: RouteCost,
    /// Bundles that ship the package as an extension or a component.
    pub bundles: Vec<String>,
    /// Dependents known to link the package statically.
    pub static_linkers: Vec<String>,
    /// Dependents known not to.
    pub not_static: Vec<String>,
    /// Builddependency-only dependents whose link cannot be known.
    pub static_unknown: Vec<String>,
    /// [`ROUTE_IN_PLACE`], [`ROUTE_BUMP`], [`ROUTE_DEFER`], or [`ROUTE_UNKNOWN`].
    pub recommended: String,
    /// Why that route was chosen.
    pub reason: String,
}

/// Union of each strategy over every finding that has a closure.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FixCostRollup {
    /// Installed modules. This is the denominator.
    pub stack_modules: usize,
    /// Union of the bump closures.
    pub bump_modules: usize,
    /// [`Self::bump_modules`] over [`Self::stack_modules`].
    pub bump_fraction: f64,
    /// Union of the in-place sets.
    pub in_place_modules: usize,
    /// [`Self::in_place_modules`] over [`Self::stack_modules`].
    pub in_place_fraction: f64,
    /// Union of the modules the recommended route rebuilds. A deferred
    /// finding contributes nothing.
    pub recommended_modules: usize,
    /// [`Self::recommended_modules`] over [`Self::stack_modules`].
    pub recommended_fraction: f64,
    /// Union of the dependents whose static link cannot be known.
    pub static_unknown_modules: usize,
}

/// Per-finding costs for one installed stack.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FixCostDocument {
    /// [`FIX_COST_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// When the rebuild set was computed.
    pub generated_at: String,
    /// Where the installed list came from.
    pub installed_source: String,
    /// Installed modules.
    pub stack_modules: usize,
    /// Rows in the scanner export.
    pub rows: usize,
    /// Findings in [`Self::ranked`].
    pub findings: usize,
    /// One row per advisory, package, and version.
    pub ranked: Vec<FindingRoute>,
    /// What fraction of the stack each strategy rebuilds.
    pub rollup: FixCostRollup,
    /// Modules the export names that are not installed.
    pub not_installed: Vec<String>,
    /// Installed modules with a finding whose easyconfig is not in the graph.
    pub not_in_graph: Vec<String>,
    /// Unreadable recipes, and component versions that were not literals.
    pub notes: Vec<String>,
}

/// Read `candidates` in order. A later easyconfig filename replaces an earlier
/// one, which is how a `from-commit` tree replaces the release tree.
pub fn scan_candidates<'a, I>(candidates: I, installed: &BTreeSet<String>) -> EvidenceScan
where
    I: IntoIterator<Item = &'a Candidate>,
{
    let mut chosen: BTreeMap<String, String> = BTreeMap::new();
    for candidate in candidates {
        let path = candidate.easyconfig_path.trim();
        if path.is_empty() {
            continue;
        }
        let file = basename(path);
        if file.is_empty() {
            continue;
        }
        chosen.insert(file.to_string(), path.to_string());
    }
    let mut scan = EvidenceScan::default();
    let mut unparsed = 0usize;
    for (file, path) in &chosen {
        let text = match std::fs::read_to_string(Path::new(path)) {
            Ok(text) => text,
            Err(error) => {
                scan.notes.push(format!("{path}: {error}"));
                continue;
            }
        };
        let Some(module) = module_for_filename(file, installed) else {
            continue;
        };
        let (recipe, skipped) = recipe_from_text(file, &module, &text);
        unparsed += skipped;
        scan.recipes.push(recipe);
    }
    if unparsed > 0 {
        scan.notes.push(format!(
            "{unparsed} component entries have no literal version and were not matched"
        ));
    }
    scan.recipes
        .sort_by(|left, right| left.module.cmp(&right.module));
    scan
}

/// Build the per-finding report from a rebuild set and recipe evidence.
///
/// `evidence` comes from [`scan_candidates`]. An empty scan still prices
/// the graph: extensions and runtime edges are on the rebuild rows, and
/// every builddependency-only dependent stays unknown. `reach` is the same
/// table `rebuild-set --findings` uses for `backport.md`.
pub fn report(
    findings: &[Finding],
    installed: &BTreeSet<String>,
    document: &RebuildDocument,
    evidence: &EvidenceScan,
    reach: &ReachTable,
) -> FixCostDocument {
    let stack = installed.len();
    let rows_by_file = rows_by_changed(&document.rows);
    let hints = link_hints(evidence);
    let bundles = component_bundles(evidence);
    let missing: BTreeSet<&str> = document.missing.iter().map(String::as_str).collect();

    let mut not_installed = BTreeSet::new();
    let mut grouped: BTreeMap<GroupKey, Group> = BTreeMap::new();
    for finding in findings {
        let mut carried = Vec::new();
        for module in &finding.modules {
            if installed.contains(module) {
                carried.push(module.clone());
            } else {
                not_installed.insert(module.clone());
            }
        }
        if carried.is_empty() {
            continue;
        }
        let key = GroupKey {
            vuln_id: finding.vuln_id.clone(),
            package: finding.package.clone(),
            version: finding.version.clone(),
        };
        let group = grouped.entry(key).or_default();
        group.absorb(finding, carried);
    }

    let mut not_in_graph = BTreeSet::new();
    let mut ranked = Vec::new();
    for (key, group) in grouped {
        let mut in_graph = Vec::new();
        let mut absent = Vec::new();
        for module in &group.modules {
            let file = easyconfig_filename(module).unwrap_or_default();
            if file.is_empty()
                || missing.contains(file.as_str())
                || !rows_by_file.contains_key(&file)
            {
                absent.push(module.clone());
                not_in_graph.insert(module.clone());
            } else {
                in_graph.push(module.clone());
            }
        }
        absent.sort();
        if in_graph.is_empty() {
            ranked.push(empty_route(&key, &group, absent, stack, reach));
            continue;
        }
        let mut cost = ClosureCost::default();
        for module in &in_graph {
            let file = easyconfig_filename(module).unwrap_or_default();
            if let Some(rows) = rows_by_file.get(&file) {
                cost.add_rows(rows, &hints);
            }
            cost.add_components(module, &key.package, &key.version, &bundles);
        }
        ranked.push(route_from(
            &key, &group, &in_graph, absent, &cost, stack, reach,
        ));
    }
    ranked.sort_by(route_order);

    let rollup = rollup_of(stack, &ranked);
    FixCostDocument {
        schema_version: FIX_COST_SCHEMA_VERSION,
        generated_at: document.generated_at.clone(),
        installed_source: document.installed_source.clone(),
        stack_modules: stack,
        rows: findings.len(),
        findings: ranked.len(),
        ranked,
        rollup,
        not_installed: not_installed.into_iter().collect(),
        not_in_graph: not_in_graph.into_iter().collect(),
        notes: evidence.notes.clone(),
    }
}

/// The ranked table and the roll-up as Markdown.
pub fn render_markdown(document: &FixCostDocument) -> String {
    let mut out = String::new();
    out.push_str("# Fix cost\n\n");
    out.push_str(&format!(
        "Computed {} against the {} modules installed for `{}`. {} export rows, {} findings.\n\n",
        document.generated_at,
        document.stack_modules,
        document.installed_source,
        document.rows,
        document.findings
    ));
    out.push_str(
        "A version bump rebuilds the patched module and every installed reverse \
         dependent. The module path changes, so this breaks reproducibility. \
         An in-place patch keeps the name and version: add \
         `<Name>-<version>_<ID>.patch` to `patches`. That rebuilds the patched \
         module, a bundle that includes it as an extension or a component, and a \
         dependent known to link it statically. A runtime dependency is a shared \
         library and is not rebuilt. A builddependency-only dependent whose \
         recipe does not say, and whose install was not checked, is listed as \
         unknown and is not counted.\n\n",
    );
    out.push_str(
        "The reach class is the one `backport.md` uses. Only `cross-user` and \
         `network-facing` are recommended for a backport, and that backport is \
         the in-place patch. `local-only`, `not-reachable`, and `unclassified` \
         are deferred. A Python interpreter whose fix leaves the minor version \
         is a bump when a backport is justified, because the ABI changes. A \
         fix that stays on the same minor keeps the module path: renaming the \
         easyconfig to that patch release would rebuild the whole closure.\n\n",
    );
    let roll = &document.rollup;
    out.push_str("| Strategy | Modules | Fraction of the stack |\n");
    out.push_str("|---|---|---|\n");
    out.push_str(&format!(
        "| Version bump, every finding | {} | {} of {} ({}) |\n",
        roll.bump_modules,
        roll.bump_modules,
        roll.stack_modules,
        format_fraction(roll.bump_fraction)
    ));
    out.push_str(&format!(
        "| In-place patch, every finding | {} | {} of {} ({}) |\n",
        roll.in_place_modules,
        roll.in_place_modules,
        roll.stack_modules,
        format_fraction(roll.in_place_fraction)
    ));
    out.push_str(&format!(
        "| Recommended route | {} | {} of {} ({}) |\n",
        roll.recommended_modules,
        roll.recommended_modules,
        roll.stack_modules,
        format_fraction(roll.recommended_fraction)
    ));
    out.push_str(&format!(
        "\n{} builddependency-only dependents are not in the in-place total: \
         static linking cannot be known for them.\n\n",
        roll.static_unknown_modules
    ));
    out.push_str(
        "| Rank | Id | Package | Severity | Reach | Route | In place | Bump | Unknown static |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for (index, route) in document.ranked.iter().enumerate() {
        out.push_str(&format!(
            "| {} | `{}` | `{}` | {} | {} | {} | {} | {} | {} |\n",
            index + 1,
            route.vuln_id,
            route.package,
            route.severity,
            route.reach,
            route.recommended,
            route.in_place.modules,
            route.bump.modules,
            route.static_unknown.len()
        ));
    }
    for (index, route) in document.ranked.iter().enumerate() {
        out.push_str(&format!(
            "\n## {}. `{}` {} {}\n\n",
            index + 1,
            route.vuln_id,
            route.package,
            route.version
        ));
        out.push_str(&format!("{}\n\n", route.reason));
        out.push_str(&format!(
            "Patch files: {}.\n\n",
            display_list(&route.patch_files)
        ));
        out.push_str(&format!(
            "In place: {}.\n\n",
            display_list(&route.in_place.names)
        ));
        out.push_str(&format!(
            "Bump (breaks reproducibility: {}): {}.\n\n",
            yes_no(route.bump.breaks_reproducibility),
            display_list(&route.bump.names)
        ));
        out.push_str(&format!("Bundles: {}.\n\n", display_list(&route.bundles)));
        out.push_str(&format!(
            "Static linkers: {}.\n\n",
            display_list(&route.static_linkers)
        ));
        out.push_str(&format!(
            "Not static: {}.\n\n",
            display_list(&route.not_static)
        ));
        out.push_str(&format!(
            "Static link unknown: {}.\n",
            display_list(&route.static_unknown)
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
    if !document.notes.is_empty() {
        out.push('\n');
        for note in &document.notes {
            out.push_str(&format!("- note: {note}\n"));
        }
    }
    out
}

/// One line per finding, tab-separated, with a header.
pub fn render_tsv(document: &FixCostDocument) -> String {
    let mut out = String::from(
        "rank\tvuln_id\tpackage\tversion\tseverity\tfixed_in\tfix_state\treach\trecommended\tbump_modules\tbump_fraction\tbump_breaks_reproducibility\tin_place_modules\tin_place_fraction\tbump_names\tin_place_names\tbundles\tstatic_linkers\tstatic_unknown\tnot_static\tpatch_files\tmodules\treason\n",
    );
    for (index, route) in document.ranked.iter().enumerate() {
        let fields = [
            (index + 1).to_string(),
            route.vuln_id.clone(),
            route.package.clone(),
            route.version.clone(),
            route.severity.clone(),
            route.fixed_in.clone(),
            route.fix_state.clone(),
            route.reach.clone(),
            route.recommended.clone(),
            route.bump.modules.to_string(),
            format_fraction(route.bump.fraction),
            yes_no(route.bump.breaks_reproducibility).to_string(),
            route.in_place.modules.to_string(),
            format_fraction(route.in_place.fraction),
            route.bump.names.join(";"),
            route.in_place.names.join(";"),
            route.bundles.join(";"),
            route.static_linkers.join(";"),
            route.static_unknown.join(";"),
            route.not_static.join(";"),
            route.patch_files.join(";"),
            route.modules.join(";"),
            route.reason.clone(),
        ];
        let line: Vec<String> = fields.iter().map(|field| tsv_field(field)).collect();
        out.push_str(&line.join("\t"));
        out.push('\n');
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    vuln_id: String,
    package: String,
    version: String,
}

#[derive(Debug, Default)]
struct Group {
    severity: String,
    rank: usize,
    fixed_in: String,
    fix_state: String,
    modules: BTreeSet<String>,
}

impl Group {
    fn absorb(&mut self, finding: &Finding, modules: Vec<String>) {
        let rank = severity_rank(&finding.severity).unwrap_or(SEVERITIES.len());
        let replace = self.severity.is_empty() || rank < self.rank;
        if replace {
            self.rank = rank;
            self.severity = finding.severity.clone();
            if !finding.fixed_in.is_empty() {
                self.fixed_in = finding.fixed_in.clone();
            }
            if !finding.fix_state.is_empty() {
                self.fix_state = finding.fix_state.clone();
            }
        } else {
            if self.fixed_in.is_empty() {
                self.fixed_in = finding.fixed_in.clone();
            }
            if self.fix_state.is_empty() {
                self.fix_state = finding.fix_state.clone();
            }
        }
        self.modules.extend(modules);
    }
}

#[derive(Debug, Default)]
struct ClosureCost {
    bump: BTreeSet<String>,
    in_place: BTreeSet<String>,
    bundles: BTreeSet<String>,
    static_linkers: BTreeSet<String>,
    not_static: BTreeSet<String>,
    unknown: BTreeSet<String>,
}

impl ClosureCost {
    fn add_rows(&mut self, rows: &[&RebuildRow], hints: &BTreeMap<String, LinkHint>) {
        for row in rows {
            self.bump.insert(row.dependent.clone());
            match place(
                row,
                hints
                    .get(&row.dependent)
                    .copied()
                    .unwrap_or(LinkHint::Unknown),
            ) {
                Place::Patched => {
                    self.in_place.insert(row.dependent.clone());
                }
                Place::Bundle => {
                    self.in_place.insert(row.dependent.clone());
                    self.bundles.insert(row.dependent.clone());
                }
                Place::Static => {
                    self.in_place.insert(row.dependent.clone());
                    self.static_linkers.insert(row.dependent.clone());
                }
                Place::NotStatic => {
                    self.not_static.insert(row.dependent.clone());
                }
                Place::Unknown => {
                    self.unknown.insert(row.dependent.clone());
                }
            }
        }
    }

    fn add_components(
        &mut self,
        module: &str,
        package: &str,
        version: &str,
        bundles: &BTreeMap<(String, String), BTreeSet<String>>,
    ) {
        let names = [normalize(package), normalize(module_name(module))];
        let versions = version_keys(version, module);
        for name in names {
            if name.is_empty() {
                continue;
            }
            for version in &versions {
                let Some(found) = bundles.get(&(name.clone(), version.clone())) else {
                    continue;
                };
                for bundle in found {
                    if bundle == module {
                        continue;
                    }
                    self.in_place.insert(bundle.clone());
                    self.bundles.insert(bundle.clone());
                }
            }
        }
    }
}

enum Place {
    Patched,
    Bundle,
    Static,
    NotStatic,
    Unknown,
}

fn place(row: &RebuildRow, hint: LinkHint) -> Place {
    if row.edge_kind == "changed" {
        return Place::Patched;
    }
    if row.edge_kind == "extension" || row.reason.contains("extension in this bundle") {
        return Place::Bundle;
    }
    if row.reason.contains("defined symbols of") {
        return Place::Static;
    }
    if row.reason.contains("no defined dynamic symbol") || row.reason.contains("no static archive")
    {
        return Place::NotStatic;
    }
    if row.edge_kind == "runtime" || row.reason.contains("shared library") {
        return Place::NotStatic;
    }
    if row.class == CLASS_NO_REBUILD {
        return Place::NotStatic;
    }
    match hint {
        LinkHint::Static => Place::Static,
        LinkHint::Shared => Place::NotStatic,
        LinkHint::Unknown => Place::Unknown,
    }
}

fn route_from(
    key: &GroupKey,
    group: &Group,
    modules: &[String],
    not_in_graph: Vec<String>,
    cost: &ClosureCost,
    stack: usize,
    reach_table: &ReachTable,
) -> FindingRoute {
    let reach = reach_of(reach_table, &key.package, modules);
    let python = python_release(&key.package, modules, &key.version, &group.fixed_in);
    let (recommended, reason) = recommend(&key.package, &reach, python, cost, stack);
    let patch_files = patch_files(&key.vuln_id, &key.version, modules);
    FindingRoute {
        vuln_id: key.vuln_id.clone(),
        package: key.package.clone(),
        version: key.version.clone(),
        severity: severity_label(&group.severity),
        fixed_in: group.fixed_in.clone(),
        fix_state: group.fix_state.clone(),
        reach,
        modules: modules.to_vec(),
        not_in_graph,
        patch_files,
        bump: BumpCost {
            modules: cost.bump.len(),
            fraction: fraction(cost.bump.len(), stack),
            names: cost.bump.iter().cloned().collect(),
            breaks_reproducibility: true,
        },
        in_place: route_cost(&cost.in_place, stack),
        bundles: cost.bundles.iter().cloned().collect(),
        static_linkers: cost.static_linkers.iter().cloned().collect(),
        not_static: cost.not_static.iter().cloned().collect(),
        static_unknown: cost.unknown.iter().cloned().collect(),
        recommended,
        reason,
    }
}

fn empty_route(
    key: &GroupKey,
    group: &Group,
    not_in_graph: Vec<String>,
    stack: usize,
    reach_table: &ReachTable,
) -> FindingRoute {
    let modules: Vec<String> = group.modules.iter().cloned().collect();
    let reach = reach_of(reach_table, &key.package, &modules);
    FindingRoute {
        vuln_id: key.vuln_id.clone(),
        package: key.package.clone(),
        version: key.version.clone(),
        severity: severity_label(&group.severity),
        fixed_in: group.fixed_in.clone(),
        fix_state: group.fix_state.clone(),
        reach,
        patch_files: patch_files(&key.vuln_id, &key.version, &modules),
        modules,
        not_in_graph,
        bump: BumpCost {
            modules: 0,
            fraction: fraction(0, stack),
            names: Vec::new(),
            breaks_reproducibility: false,
        },
        in_place: route_cost(&BTreeSet::new(), stack),
        bundles: Vec::new(),
        static_linkers: Vec::new(),
        not_static: Vec::new(),
        static_unknown: Vec::new(),
        recommended: ROUTE_UNKNOWN.to_string(),
        reason: "the easyconfig is not in the deployed graph, so neither cost is known".to_string(),
    }
}

fn recommend(
    package: &str,
    reach: &str,
    python: PythonRelease,
    cost: &ClosureCost,
    stack: usize,
) -> (String, String) {
    let costs = format!(
        "An in-place patch rebuilds {} of {} modules. A version bump rebuilds {} of {} and breaks reproducibility, because the module path changes.",
        cost.in_place.len(),
        stack,
        cost.bump.len(),
        stack
    );
    let unknown = if cost.unknown.is_empty() {
        String::new()
    } else {
        format!(
            " {} builddependency-only dependents are not in the in-place count: the graph and the recipe cannot say whether they linked {} statically ({}).",
            cost.unknown.len(),
            package,
            cost.unknown.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    };
    let python_note = match python {
        PythonRelease::Patch => {
            " Python's ABI is stable within a minor version, so the fix is an in-place patch. Renaming the module to the patch release rebuilds the whole Python closure."
        }
        PythonRelease::MinorOrMajor => {
            " The fixed release changes Python's minor or major version, so the ABI changes and the module has to be renamed."
        }
        PythonRelease::NotPython | PythonRelease::Unknown => "",
    };
    if justifies_backport(reach) && python == PythonRelease::MinorOrMajor {
        return (
            ROUTE_BUMP.to_string(),
            format!(
                "{package} is {reach}, and the fix leaves the minor version. {costs}{unknown}{python_note}"
            ),
        );
    }
    if justifies_backport(reach) {
        return (
            ROUTE_IN_PLACE.to_string(),
            format!("{package} is {reach}. {costs}{unknown}{python_note}"),
        );
    }
    let why = match reach {
        REACH_NOT_REACHABLE => {
            format!("{package} is not reachable on Linux x86_64.")
        }
        REACH_LOCAL => {
            format!("{package} is local-only: a hostile file in the user's own job.")
        }
        _ => format!(
            "{package} is unclassified: no built-in name and no reachability file names it."
        ),
    };
    (
        ROUTE_DEFER.to_string(),
        format!(
            "{why} Only a cross-user or network-facing finding justifies a backport. {costs}{unknown}{python_note}"
        ),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PythonRelease {
    NotPython,
    /// Same `X.Y`, and the patch component differs or appears.
    Patch,
    /// `X` or `Y` differs.
    MinorOrMajor,
    /// Not enough version fields to say.
    Unknown,
}

fn python_release(
    package: &str,
    modules: &[String],
    version: &str,
    fixed_in: &str,
) -> PythonRelease {
    let interpreter = normalize(package) == "python"
        || modules
            .iter()
            .any(|module| normalize(module_name(module)) == "python");
    if !interpreter {
        return PythonRelease::NotPython;
    }
    let old = if version.is_empty() {
        modules
            .iter()
            .find_map(|module| software_version("", module))
            .unwrap_or_default()
    } else {
        version.to_string()
    };
    compare_python(&old, fixed_in)
}

fn compare_python(old: &str, new: &str) -> PythonRelease {
    let old_minor = version_minor(old);
    let new_minor = version_minor(new);
    let (Some(old_minor), Some(new_minor)) = (old_minor, new_minor) else {
        return PythonRelease::Unknown;
    };
    if old_minor != new_minor {
        return PythonRelease::MinorOrMajor;
    }
    let old_nums = numeric_prefix(old);
    let new_nums = numeric_prefix(new);
    if old_nums == new_nums {
        PythonRelease::Unknown
    } else {
        PythonRelease::Patch
    }
}

fn version_minor(version: &str) -> Option<(u64, u64)> {
    let nums = numeric_prefix(version);
    (nums.len() >= 2).then_some((nums[0], nums[1]))
}

fn numeric_prefix(version: &str) -> Vec<u64> {
    let mut nums = Vec::new();
    for part in crate::version::parse_version_parts(version) {
        if let crate::version::Part::Num(value) = part {
            nums.push(value);
        }
    }
    nums
}

fn reach_of(table: &ReachTable, package: &str, modules: &[String]) -> String {
    let mut best = REACH_UNCLASSIFIED.to_string();
    let mut saw = false;
    for module in modules {
        saw = true;
        let class = table.class_of(module, &[package]);
        if reach_rank(&class) < reach_rank(&best) {
            best = class;
        }
    }
    if !saw {
        return table.class_of(package, &[package]);
    }
    best
}

/// Lower is a stronger class, the same order as the backport table.
fn reach_rank(class: &str) -> u8 {
    match class {
        REACH_CROSS_USER => 0,
        REACH_NETWORK => 1,
        REACH_LOCAL => 2,
        REACH_NOT_REACHABLE => 3,
        _ => 4,
    }
}

fn patch_files(vuln_id: &str, version: &str, modules: &[String]) -> Vec<String> {
    let mut files = BTreeSet::new();
    let id = sanitize_token(vuln_id);
    for module in modules {
        let name = sanitize_token(module_name(module));
        let version = software_version(version, module).unwrap_or_else(|| sanitize_token(version));
        if name.is_empty() || version.is_empty() {
            continue;
        }
        files.insert(format!("{name}-{version}_{id}.patch"));
    }
    files.into_iter().collect()
}

fn software_version(finding_version: &str, module: &str) -> Option<String> {
    if !finding_version.trim().is_empty() {
        return Some(sanitize_token(finding_version.trim()));
    }
    let full = module
        .split_once('/')
        .map(|(_, version)| version)
        .unwrap_or(module);
    let version = match full.split_once('-') {
        Some((head, tail)) if tail.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => head,
        _ => full,
    };
    let version = sanitize_token(version);
    (!version.is_empty()).then_some(version)
}

fn version_keys(finding_version: &str, module: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if !finding_version.is_empty() {
        keys.insert(finding_version.to_string());
    }
    if let Some(version) = software_version("", module) {
        keys.insert(version);
    }
    let full = module
        .split_once('/')
        .map(|(_, version)| version)
        .unwrap_or("");
    if !full.is_empty() {
        keys.insert(full.to_string());
    }
    keys
}

fn sanitize_token(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    out
}

fn rollup_of(stack: usize, ranked: &[FindingRoute]) -> FixCostRollup {
    let mut bump = BTreeSet::new();
    let mut in_place = BTreeSet::new();
    let mut recommended = BTreeSet::new();
    let mut unknown = BTreeSet::new();
    for route in ranked {
        if route.recommended == ROUTE_UNKNOWN {
            unknown.extend(route.static_unknown.iter().cloned());
            continue;
        }
        bump.extend(route.bump.names.iter().cloned());
        in_place.extend(route.in_place.names.iter().cloned());
        unknown.extend(route.static_unknown.iter().cloned());
        let chosen: &[String] = match route.recommended.as_str() {
            ROUTE_IN_PLACE => &route.in_place.names,
            ROUTE_BUMP => &route.bump.names,
            _ => &[],
        };
        recommended.extend(chosen.iter().cloned());
    }
    FixCostRollup {
        stack_modules: stack,
        bump_modules: bump.len(),
        bump_fraction: fraction(bump.len(), stack),
        in_place_modules: in_place.len(),
        in_place_fraction: fraction(in_place.len(), stack),
        recommended_modules: recommended.len(),
        recommended_fraction: fraction(recommended.len(), stack),
        static_unknown_modules: unknown.len(),
    }
}

fn route_cost(names: &BTreeSet<String>, stack: usize) -> RouteCost {
    RouteCost {
        modules: names.len(),
        fraction: fraction(names.len(), stack),
        names: names.iter().cloned().collect(),
    }
}

fn fraction(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 10_000.0).round() / 10_000.0
    }
}

fn format_fraction(value: f64) -> String {
    format!("{value:.4}")
}

fn rows_by_changed(rows: &[RebuildRow]) -> BTreeMap<String, Vec<&RebuildRow>> {
    let mut out: BTreeMap<String, Vec<&RebuildRow>> = BTreeMap::new();
    for row in rows {
        for changed in row.changed.split(", ") {
            if !changed.is_empty() {
                out.entry(changed.to_string()).or_default().push(row);
            }
        }
    }
    out
}

fn link_hints(evidence: &EvidenceScan) -> BTreeMap<String, LinkHint> {
    evidence
        .recipes
        .iter()
        .map(|recipe| (recipe.module.clone(), recipe.link))
        .collect()
}

fn component_bundles(evidence: &EvidenceScan) -> BTreeMap<(String, String), BTreeSet<String>> {
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for recipe in &evidence.recipes {
        for (name, version) in &recipe.components {
            out.entry((normalize(name), version.clone()))
                .or_default()
                .insert(recipe.module.clone());
        }
    }
    out
}

fn recipe_from_text(file: &str, module: &str, text: &str) -> (RecipeEvidence, usize) {
    let cleaned = strip_hash_comments(text);
    let (components, skipped) = component_entries(&cleaned);
    let link = link_hint(&cleaned);
    (
        RecipeEvidence {
            easyconfig: file.to_string(),
            module: module.to_string(),
            components,
            link,
        },
        skipped,
    )
}

fn link_hint(text: &str) -> LinkHint {
    let text = strip_hash_comments(text);
    let mut body = String::new();
    for field in ["configopts", "buildopts"] {
        if let Some(value) = assignment_body(&text, field) {
            body.push_str(&value);
            body.push('\n');
        }
    }
    let lower = body.to_ascii_lowercase();
    let static_vote = lower.contains("--disable-shared")
        || lower.contains("--enable-static")
        || shared_libs(&lower, false);
    let shared_vote = lower.contains("--disable-static")
        || lower.contains("--enable-shared")
        || shared_libs(&lower, true);
    match (static_vote, shared_vote) {
        (true, false) => LinkHint::Static,
        (false, true) => LinkHint::Shared,
        _ => LinkHint::Unknown,
    }
}

fn shared_libs(text: &str, on: bool) -> bool {
    let needle = "build_shared_libs";
    let mut rest = text;
    while let Some(at) = rest.find(needle) {
        let after = &rest[at + needle.len()..];
        let after = after.trim_start_matches(|c: char| c == ':' || c.is_ascii_alphabetic());
        let after = after.trim_start();
        if let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let token = value
                .split(|c: char| !c.is_ascii_alphanumeric())
                .find(|part| !part.is_empty())
                .unwrap_or("");
            let enabled = matches!(token, "on" | "true" | "1" | "yes");
            let disabled = matches!(token, "off" | "false" | "0" | "no");
            if (on && enabled) || (!on && disabled) {
                return true;
            }
        }
        rest = &rest[at + needle.len()..];
    }
    false
}

fn component_entries(text: &str) -> (Vec<(String, String)>, usize) {
    let Some(body) = bracket_body(text, "components") else {
        return (Vec::new(), 0);
    };
    let mut found = Vec::new();
    let mut skipped = 0usize;
    for tuple in top_tuples(&body) {
        if let Some(pair) = quoted_pair(&tuple) {
            if pair.0.contains('%') || pair.1.contains('%') {
                skipped += 1;
            } else {
                found.push(pair);
            }
        } else if tuple.contains('(') {
            skipped += 1;
        }
    }
    (found, skipped)
}

fn quoted_pair(tuple: &str) -> Option<(String, String)> {
    let bytes = tuple.as_bytes();
    let (name, next) = next_quoted(bytes, 0)?;
    let (version, _) = next_quoted(bytes, next)?;
    Some((name, version))
}

fn next_quoted(bytes: &[u8], mut i: usize) -> Option<(String, usize)> {
    while i < bytes.len() && bytes[i] != b'\'' && bytes[i] != b'"' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let quote = bytes[i];
    let start = i + 1;
    let mut k = start;
    while k < bytes.len() && bytes[k] != quote {
        k += 1;
    }
    if k >= bytes.len() {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes[start..k]).into_owned();
    Some((text, k + 1))
}

fn bracket_body(text: &str, field: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let needle = field.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if bytes[i..].starts_with(needle) && at_line_start(bytes, i) {
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'=') {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if bytes.get(j) == Some(&b'[') {
                    let body = take_balanced(&text[j..]);
                    return Some(body);
                }
            }
        }
        i += 1;
    }
    None
}

fn assignment_body(text: &str, field: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let needle = field.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if bytes[i..].starts_with(needle) && at_line_start(bytes, i) {
            let mut j = i + needle.len();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'=') {
                let start = j + 1;
                let mut k = start;
                while k < bytes.len() {
                    if bytes[k] == b'\n'
                        && at_line_start(bytes, k + 1)
                        && looks_like_assign(bytes, k + 1)
                    {
                        break;
                    }
                    k += 1;
                }
                return Some(text[start..k].to_string());
            }
        }
        i += 1;
    }
    None
}

fn looks_like_assign(bytes: &[u8], mut i: usize) -> bool {
    if i >= bytes.len() || !bytes[i].is_ascii_alphabetic() {
        return false;
    }
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    bytes.get(i) == Some(&b'=')
}

fn at_line_start(bytes: &[u8], index: usize) -> bool {
    index == 0 || bytes[index - 1] == b'\n'
}

fn take_balanced(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'[') {
        return String::new();
    }
    let mut depth = 0i32;
    let mut quote = None;
    for (index, byte) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if *byte == q {
                quote = None;
            }
            continue;
        }
        match *byte {
            b'\'' | b'"' => quote = Some(*byte),
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return text[..=index].to_string();
                }
            }
            _ => {}
        }
    }
    text.to_string()
}

fn top_tuples(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let mut quote = None;
    let mut out = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
            i += 1;
            continue;
        }
        if c == b'(' && depth == 1 {
            let chunk = take_paren(&body[i..]);
            let len = chunk.len().max(1);
            out.push(chunk);
            i += len;
            continue;
        }
        if matches!(c, b'[' | b'(' | b'{') {
            depth += 1;
        }
        if matches!(c, b']' | b')' | b'}') {
            depth -= 1;
        }
        i += 1;
    }
    out
}

fn take_paren(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut quote = None;
    for (index, byte) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if *byte == q {
                quote = None;
            }
            continue;
        }
        match *byte {
            b'\'' | b'"' => quote = Some(*byte),
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return text[..=index].to_string();
                }
            }
            _ => {}
        }
    }
    text.to_string()
}

fn strip_hash_comments(text: &str) -> String {
    let mut out = String::new();
    let mut quote = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                out.push(c);
            }
            '#' => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn route_order(left: &FindingRoute, right: &FindingRoute) -> std::cmp::Ordering {
    route_rank(&left.recommended)
        .cmp(&route_rank(&right.recommended))
        .then_with(|| severity_index(&left.severity).cmp(&severity_index(&right.severity)))
        .then_with(|| right.bump.modules.cmp(&left.bump.modules))
        .then_with(|| left.vuln_id.cmp(&right.vuln_id))
        .then_with(|| left.package.cmp(&right.package))
        .then_with(|| left.version.cmp(&right.version))
}

fn route_rank(route: &str) -> u8 {
    match route {
        ROUTE_IN_PLACE => 0,
        ROUTE_BUMP => 1,
        ROUTE_DEFER => 2,
        _ => 3,
    }
}

fn severity_index(severity: &str) -> usize {
    severity_rank(severity).unwrap_or(SEVERITIES.len())
}

fn severity_label(severity: &str) -> String {
    SEVERITIES
        .get(severity_index(severity))
        .copied()
        .unwrap_or("unknown")
        .to_string()
}

fn module_for_filename(file: &str, installed: &BTreeSet<String>) -> Option<String> {
    installed
        .iter()
        .find(|module| easyconfig_filename(module).as_deref() == Some(file))
        .cloned()
}

fn module_name(module: &str) -> &str {
    module
        .split_once('/')
        .map(|(name, _)| name)
        .unwrap_or(module)
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn display_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("`{item}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn tsv_field(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::parse_findings_csv;
    use crate::rebuild_set::{
        RebuildCounts, RebuildRow, CLASS_MUST_REBUILD, CLASS_REVIEW, REBUILD_SET_SCHEMA_VERSION,
    };

    fn row(changed: &str, dependent: &str, edge: &str, class: &str, reason: &str) -> RebuildRow {
        RebuildRow {
            changed: changed.to_string(),
            dependent: dependent.to_string(),
            dependent_easyconfig: String::new(),
            edge_kind: edge.to_string(),
            class: class.to_string(),
            reason: reason.to_string(),
            eb_version: None,
        }
    }

    fn document(rows: Vec<RebuildRow>, missing: Vec<String>) -> RebuildDocument {
        RebuildDocument {
            schema_version: REBUILD_SET_SCHEMA_VERSION,
            generated_at: "2026-11-01T00:00:00Z".to_string(),
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
            reach: std::collections::BTreeMap::new(),
            rows,
        }
    }

    fn finding(
        severity: &str,
        id: &str,
        package: &str,
        version: &str,
        fixed: &str,
        module: &str,
    ) -> Finding {
        Finding {
            severity: severity.to_string(),
            vuln_id: id.to_string(),
            package: package.to_string(),
            version: version.to_string(),
            fixed_in: fixed.to_string(),
            fix_state: "fixed".to_string(),
            modules: vec![module.to_string()],
            fix_commits: Vec::new(),
        }
    }

    fn builtin() -> ReachTable {
        ReachTable::builtin()
    }

    fn zlib_rows() -> Vec<RebuildRow> {
        let file = "zlib-1.2-GCCcore-15.2.0.eb";
        vec![
            row(file, "zlib/1.2-GCCcore-15.2.0", "changed", CLASS_MUST_REBUILD, "patched easyconfig"),
            row(file, "libpng/1.6-GCCcore-15.2.0", "runtime", CLASS_NO_REBUILD, "runtime dependency; links the patched package as a shared library"),
            row(file, "AppStatic/1.0-GCCcore-15.2.0", "builddependency", CLASS_REVIEW, "builddependency only; it may have linked the patched package statically or embedded it"),
            row(file, "AppShared/1.0-GCCcore-15.2.0", "builddependency", CLASS_REVIEW, "builddependency only; it may have linked the patched package statically or embedded it"),
            row(file, "AppUnknown/1.0-GCCcore-15.2.0", "builddependency", CLASS_REVIEW, "builddependency only; it may have linked the patched package statically or embedded it"),
        ]
    }

    fn evidence() -> EvidenceScan {
        EvidenceScan {
            recipes: vec![
                RecipeEvidence {
                    easyconfig: "Kit-1.0-GCCcore-15.2.0.eb".into(),
                    module: "Kit/1.0-GCCcore-15.2.0".into(),
                    components: vec![("zlib".into(), "1.2".into())],
                    link: LinkHint::Unknown,
                },
                RecipeEvidence {
                    easyconfig: "AppStatic-1.0-GCCcore-15.2.0.eb".into(),
                    module: "AppStatic/1.0-GCCcore-15.2.0".into(),
                    components: Vec::new(),
                    link: LinkHint::Static,
                },
                RecipeEvidence {
                    easyconfig: "AppShared-1.0-GCCcore-15.2.0.eb".into(),
                    module: "AppShared/1.0-GCCcore-15.2.0".into(),
                    components: Vec::new(),
                    link: LinkHint::Shared,
                },
            ],
            notes: Vec::new(),
        }
    }

    fn installed_small() -> BTreeSet<String> {
        [
            "zlib/1.2-GCCcore-15.2.0",
            "libpng/1.6-GCCcore-15.2.0",
            "AppStatic/1.0-GCCcore-15.2.0",
            "AppShared/1.0-GCCcore-15.2.0",
            "AppUnknown/1.0-GCCcore-15.2.0",
            "Kit/1.0-GCCcore-15.2.0",
            "cURL/8.7.1-GCCcore-15.2.0",
            "Python/3.11.4-GCCcore-15.2.0",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn a_static_recipe_is_in_place_and_a_component_bundle_is_not_a_bump() {
        let findings = vec![finding(
            "Critical",
            "ADV-1",
            "zlib",
            "1.2",
            "1.2.1",
            "zlib/1.2-GCCcore-15.2.0",
        )];
        let installed = installed_small();
        let summary = report(
            &findings,
            &installed,
            &document(zlib_rows(), Vec::new()),
            &evidence(),
            &builtin(),
        );
        let route = &summary.ranked[0];
        assert_eq!(route.recommended, ROUTE_DEFER);
        assert_eq!(route.reach, REACH_UNCLASSIFIED);
        assert_eq!(route.patch_files, ["zlib-1.2_ADV-1.patch"]);
        assert!(route.reason.contains("breaks reproducibility"));
        assert!(route.reason.contains("cannot say"));
        assert_eq!(
            route.bump.names,
            [
                "AppShared/1.0-GCCcore-15.2.0",
                "AppStatic/1.0-GCCcore-15.2.0",
                "AppUnknown/1.0-GCCcore-15.2.0",
                "libpng/1.6-GCCcore-15.2.0",
                "zlib/1.2-GCCcore-15.2.0",
            ]
        );
        assert!(route.bump.breaks_reproducibility);
        assert_eq!(
            route.in_place.names,
            [
                "AppStatic/1.0-GCCcore-15.2.0",
                "Kit/1.0-GCCcore-15.2.0",
                "zlib/1.2-GCCcore-15.2.0",
            ]
        );
        assert_eq!(route.bundles, ["Kit/1.0-GCCcore-15.2.0"]);
        assert_eq!(route.static_linkers, ["AppStatic/1.0-GCCcore-15.2.0"]);
        assert_eq!(
            route.not_static,
            ["AppShared/1.0-GCCcore-15.2.0", "libpng/1.6-GCCcore-15.2.0"]
        );
        assert_eq!(route.static_unknown, ["AppUnknown/1.0-GCCcore-15.2.0"]);
        assert_eq!(route.in_place.fraction, fraction(3, installed.len()));
        assert_eq!(summary.rollup.bump_modules, 5);
        assert_eq!(summary.rollup.in_place_modules, 3);
        assert_eq!(summary.rollup.recommended_modules, 0);
        assert_eq!(summary.rollup.static_unknown_modules, 1);
    }

    #[test]
    fn network_and_python_routes_follow_reach_and_the_minor_version() {
        let curl = "cURL/8.7.1-GCCcore-15.2.0";
        let python = "Python/3.11.4-GCCcore-15.2.0";
        let file_c = "cURL-8.7.1-GCCcore-15.2.0.eb";
        let file_p = "Python-3.11.4-GCCcore-15.2.0.eb";
        let rows = vec![
            row(
                file_c,
                curl,
                "changed",
                CLASS_MUST_REBUILD,
                "patched easyconfig",
            ),
            row(
                file_p,
                python,
                "changed",
                CLASS_MUST_REBUILD,
                "patched easyconfig",
            ),
            row(
                file_p,
                "notebook/7.0-GCCcore-15.2.0",
                "runtime",
                CLASS_NO_REBUILD,
                "runtime dependency; links the patched package as a shared library",
            ),
        ];
        let patch = finding("High", "ADV-2", "cURL", "8.7.1", "8.7.2", curl);
        let same_minor = finding("High", "ADV-5", "Python", "3.11.4", "3.11.5", python);
        let next_minor = finding("Low", "ADV-6", "Python", "3.11.4", "3.12.0", python);
        let installed: BTreeSet<String> = [curl, python, "notebook/7.0-GCCcore-15.2.0"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let reach = ReachTable::builtin()
            .overlay("Python network-facing\n", "reach.txt")
            .expect("overlay");
        let summary = report(
            &[patch, same_minor, next_minor],
            &installed,
            &document(rows, Vec::new()),
            &EvidenceScan::default(),
            &reach,
        );
        let by_id = |id: &str| {
            summary
                .ranked
                .iter()
                .find(|route| route.vuln_id == id)
                .unwrap()
        };
        assert_eq!(by_id("ADV-2").reach, REACH_NETWORK);
        assert_eq!(by_id("ADV-2").recommended, ROUTE_IN_PLACE);
        assert_eq!(by_id("ADV-2").patch_files, ["cURL-8.7.1_ADV-2.patch"]);
        assert_eq!(by_id("ADV-5").recommended, ROUTE_IN_PLACE);
        assert!(by_id("ADV-5").reason.contains("ABI is stable"));
        assert_eq!(by_id("ADV-6").recommended, ROUTE_BUMP);
        assert!(by_id("ADV-6")
            .bump
            .names
            .contains(&"notebook/7.0-GCCcore-15.2.0".to_string()));
        assert_eq!(summary.ranked[0].vuln_id, "ADV-5");
        assert_eq!(summary.ranked[1].vuln_id, "ADV-2");
        assert_eq!(summary.ranked[2].vuln_id, "ADV-6");
        let text = render_markdown(&summary);
        assert!(text.contains("Recommended route"));
        let tsv = render_tsv(&summary);
        assert!(tsv.lines().next().unwrap().starts_with("rank\tvuln_id\t"));
        assert!(tsv.contains("ADV-2"));
    }

    #[test]
    fn an_explicit_unreachable_reach_defers_and_a_missing_graph_is_unknown() {
        let parsed = parse_findings_csv(
            "severity,vuln_id,package,version,used_in\n\
             Low,ADV-7,WinOnly,1.0,WinOnly/1.0-GCCcore-15.2.0\n\
             High,ADV-8,Gone,1.0,Gone/1.0\n",
        )
        .expect("parse");
        let installed: BTreeSet<String> = ["WinOnly/1.0-GCCcore-15.2.0".to_string()].into();
        let rows = vec![row(
            "WinOnly-1.0-GCCcore-15.2.0.eb",
            "WinOnly/1.0-GCCcore-15.2.0",
            "changed",
            CLASS_MUST_REBUILD,
            "patched easyconfig",
        )];
        let reach = ReachTable::builtin()
            .overlay("WinOnly not-reachable\n", "reach.txt")
            .expect("overlay");
        let summary = report(
            &parsed,
            &installed,
            &document(rows, vec!["Gone-1.0.eb".into()]),
            &EvidenceScan::default(),
            &reach,
        );
        assert_eq!(summary.not_installed, ["Gone/1.0"]);
        let win = summary
            .ranked
            .iter()
            .find(|route| route.vuln_id == "ADV-7")
            .unwrap();
        assert_eq!(win.recommended, ROUTE_DEFER);
        assert_eq!(win.reach, REACH_NOT_REACHABLE);
        assert!(summary.ranked.iter().all(|route| route.vuln_id != "ADV-8"));
    }

    #[test]
    fn symbol_and_archive_evidence_beats_the_recipe() {
        let file = "zlib-1.2-GCCcore-15.2.0.eb";
        let rows = vec![
            row(
                file,
                "zlib/1.2-GCCcore-15.2.0",
                "changed",
                CLASS_MUST_REBUILD,
                "patched easyconfig",
            ),
            row(
                file,
                "Hit/1.0",
                "builddependency",
                CLASS_MUST_REBUILD,
                "defined symbols of zlib-1.2-GCCcore-15.2.0.eb: deflate (1 symbols)",
            ),
            row(
                file,
                "Miss/1.0",
                "builddependency",
                CLASS_NO_REBUILD,
                "no defined dynamic symbol of the patched library; static archives: libz.a",
            ),
            row(
                file,
                "NoArchive/1.0",
                "builddependency",
                CLASS_NO_REBUILD,
                "no static archive installed",
            ),
        ];
        let evidence = EvidenceScan {
            recipes: vec![RecipeEvidence {
                easyconfig: "NoArchive.eb".into(),
                module: "NoArchive/1.0".into(),
                components: Vec::new(),
                link: LinkHint::Static,
            }],
            notes: Vec::new(),
        };
        let findings = vec![finding(
            "High",
            "ADV/9",
            "cURL",
            "1",
            "2",
            "zlib/1.2-GCCcore-15.2.0",
        )];
        let installed: BTreeSet<String> = [
            "zlib/1.2-GCCcore-15.2.0",
            "Hit/1.0",
            "Miss/1.0",
            "NoArchive/1.0",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let summary = report(
            &findings,
            &installed,
            &document(rows, Vec::new()),
            &evidence,
            &builtin(),
        );
        let route = &summary.ranked[0];
        assert_eq!(route.static_linkers, ["Hit/1.0"]);
        assert_eq!(route.not_static, ["Miss/1.0", "NoArchive/1.0"]);
        assert!(route.static_unknown.is_empty());
        assert_eq!(route.patch_files, ["zlib-1_ADV_9.patch"]);
        assert_eq!(route.recommended, ROUTE_IN_PLACE);
    }

    #[test]
    fn two_modules_of_one_finding_share_one_closure() {
        let rows = vec![
            row(
                "A-1.0.eb",
                "A/1.0",
                "changed",
                CLASS_MUST_REBUILD,
                "patched easyconfig",
            ),
            row(
                "A-1.0.eb",
                "UsesA/1.0",
                "runtime",
                CLASS_NO_REBUILD,
                "runtime dependency; links the patched package as a shared library",
            ),
            row(
                "B-1.0.eb",
                "B/1.0",
                "changed",
                CLASS_MUST_REBUILD,
                "patched easyconfig",
            ),
            row(
                "B-1.0.eb",
                "UsesB/1.0",
                "runtime",
                CLASS_NO_REBUILD,
                "runtime dependency; links the patched package as a shared library",
            ),
        ];
        let mut finding = finding("Critical", "ADV-1", "notebook", "1.0", "1.1", "A/1.0");
        finding.modules.push("B/1.0".into());
        let installed: BTreeSet<String> = ["A/1.0", "B/1.0", "UsesA/1.0", "UsesB/1.0", "Other/1.0"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let summary = report(
            &[finding],
            &installed,
            &document(rows, Vec::new()),
            &EvidenceScan::default(),
            &builtin(),
        );
        let route = &summary.ranked[0];
        assert_eq!(route.recommended, ROUTE_IN_PLACE);
        assert_eq!(route.reach, REACH_CROSS_USER);
        assert_eq!(route.bump.modules, 4);
        assert_eq!(route.in_place.names, ["A/1.0", "B/1.0"]);
        assert_eq!(
            route.patch_files,
            ["A-1.0_ADV-1.patch", "B-1.0_ADV-1.patch"]
        );
        assert_eq!(summary.rollup.stack_modules, 5);
        assert_eq!(summary.rollup.recommended_fraction, fraction(2, 5));
    }

    #[test]
    fn configure_flags_and_component_literals_are_read_from_the_recipe() {
        let static_text = "configopts = '--disable-shared --enable-static'\n";
        let both = "configopts = '--enable-static --enable-shared'\n";
        let shared = "buildopts = '-DBUILD_SHARED_LIBS=ON'\n";
        let off = "configopts = '-DBUILD_SHARED_LIBS:BOOL=OFF'\n";
        assert_eq!(link_hint(static_text), LinkHint::Static);
        assert_eq!(link_hint(both), LinkHint::Unknown);
        assert_eq!(link_hint(shared), LinkHint::Shared);
        assert_eq!(link_hint(off), LinkHint::Static);
        let commented = "configopts = '--enable-shared'\n# --disable-shared\n";
        assert_eq!(link_hint(commented), LinkHint::Shared);
        let bundle = "\
components = [
    ('zlib', '1.2', {'easyblock': 'ConfigureMake'}),
    ('bare', version),
]
";
        let (entries, skipped) = component_entries(bundle);
        assert_eq!(entries, [("zlib".to_string(), "1.2".to_string())]);
        assert_eq!(skipped, 1);
    }
}
