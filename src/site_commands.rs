//! The overlap and reach reports the CLI prints.
//!
//! The MCP tools call these same functions. A second copy of the orchestration
//! would let the two surfaces drift.

use crate::component_reach::parse_compat_layer;
use crate::dep_filter::{self, FilterDeps};
use crate::easyblock_reach::{self, diff_sources, reach, read_sources, write_report, DiffRequest};
use crate::merge_order;
use crate::pr_overlap::{self, easyconfig_record_seeing, overlap_index, pr_record, OverlapConfig};
use crate::rebuild_set::{self, StackEasystack};
use crate::{parse_easyconfig_tree, parse_easyconfig_trees, write_json_pretty};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

fn usage(message: impl std::fmt::Display) -> anyhow::Error {
    crate::cli_parse::usage(message)
}

/// Why `--merged` or `--impact` rejected `number`.
///
/// A number absent from the `--pr` input was never submitted. A number that
/// is in the input but produced no record changed no easyconfig and no
/// easystack, which is a different mistake.
fn rejected_pull_request(flag: &str, number: u64, input: &[u64]) -> String {
    if input.contains(&number) {
        format!(
            "{flag} {number}: pull request {number} has no record (no easyconfigs or easystack change)"
        )
    } else {
        format!("{flag} {number}: no pull request with that number")
    }
}

/// `--generated-at` is stored as given. An absent flag is the current UTC time.
fn generated_at(value: Option<String>, seconds: bool) -> Result<String> {
    let Some(text) = value else {
        return Ok(if seconds {
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        } else {
            chrono::Utc::now().to_rfc3339()
        });
    };
    chrono::DateTime::parse_from_rfc3339(&text)
        .map_err(|error| usage(format!("--generated-at {text}: {error}")))?;
    Ok(text)
}

/// `--version` on `component-reach`: `*`, an exact version, or a requirement.
fn require_version_req(version: &str) -> Result<()> {
    if version == "*" {
        return Ok(());
    }
    if version.trim().is_empty() {
        return Err(usage("--version is empty"));
    }
    crate::version::parse_requirement(version)
        .map(|_| ())
        .map_err(|error| usage(format!("--version {version}: {error}")))
}

fn listed_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which pull-request records `pr-overlap` closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlapKind {
    /// Easystack additions on a software-layer pull request.
    SoftwareLayer,
    /// Easyconfig files on an easybuild-easyconfigs pull request.
    Easyconfigs,
}

/// Why `pr-overlap` stopped before writing a report.
#[derive(Debug, thiserror::Error)]
pub enum PrOverlapError {
    /// `--kind software-layer` needs the installed module list.
    #[error("software-layer pr-overlap requires an installed module list")]
    InstalledRequired,
    /// The report could not be built or written.
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Inputs of `eb-stack pr-overlap`, without the clap layer.
pub struct PrOverlapRequest {
    /// Pull-request JSON files.
    pub prs: Vec<PathBuf>,
    /// Easystack additions, or easyconfig file changes.
    pub kind: OverlapKind,
    /// Easyconfig trees, later ones taking precedence.
    pub easyconfigs: Vec<PathBuf>,
    /// Commit=directory trees for from-commit pins.
    pub commit_trees: Vec<String>,
    /// Installed modules, one `Name/VersionFull` per line.
    pub installed: Option<PathBuf>,
    /// Stack version whose easystacks count. Empty for easyconfigs.
    pub eessi_version: String,
    /// Label for where the installed list came from.
    pub installed_label: String,
    /// Skip easystack paths containing one of these substrings.
    pub exclude_paths: Vec<String>,
    /// Keep only easystack paths containing one of these substrings.
    pub include_paths: Vec<String>,
    /// Dependency specs to drop, EasyBuild `--filter-deps` tokens.
    pub filter_deps: Vec<String>,
    /// Lmod `.modulerc.lua` files, or trees to scan.
    pub installed_modulerc: Vec<PathBuf>,
    /// Installed easyblocks package roots.
    pub easyblocks: Vec<PathBuf>,
    /// Commit=directory for one include-easyblocks-from-commit.
    pub easyblocks_commits: Vec<String>,
    /// RFC 3339 timestamp. `None` means now, in UTC.
    pub generated_at: Option<String>,
    /// Also write `merge-order.toml`.
    pub merge_order: bool,
    /// Pull requests already merged. Implies a merge order.
    pub merged: Vec<u64>,
    /// Pull requests judged on their own. Writes `merge-impact.toml`.
    pub impact: Vec<u64>,
    /// Repository URL for Markdown links.
    pub repo_url: Option<String>,
    /// Directory for `prs/`, `overlap.toml`, and `overlap.md`.
    pub out_dir: PathBuf,
    /// `owner/name` whose open easyconfigs pull requests are fetched.
    ///
    /// `None` reads only [`Self::prs`]. Set only with [`OverlapKind::Easyconfigs`].
    pub open_repo: Option<String>,
    /// Git ref for the base text of [`Self::open_repo`]. `None` is `develop`.
    pub base_ref: Option<String>,
    /// Close every head entry of each changed easystack, not only added lines.
    ///
    /// Implied by [`Self::arch_installed`].
    pub bot_build_set: bool,
    /// Installed modules for one architecture, `ARCH=FILE`. Repeatable.
    pub arch_installed: Vec<String>,
}

/// Counts `pr-overlap` prints on its one summary line.
pub struct PrOverlapRun {
    /// Pull requests read from the JSON inputs.
    pub prs: usize,
    /// Records written.
    pub records: usize,
    /// Modules more than one pull request would build.
    pub shared_modules: usize,
    /// Easyconfigs the parser skipped.
    pub skipped_easyconfigs: usize,
    /// Directory the report was written to.
    pub out_dir: PathBuf,
}

impl PrOverlapRun {
    /// The line `eb-stack pr-overlap` prints.
    pub fn summary(&self) -> String {
        format!(
            "prs={} records={} shared_modules={} skipped_easyconfigs={} out={}",
            self.prs,
            self.records,
            self.shared_modules,
            self.skipped_easyconfigs,
            self.out_dir.display()
        )
    }
}

/// Inputs of `eb-stack rebuild-set`.
pub struct RebuildSetRequest {
    /// Patched easyconfig filenames, or a list file.
    pub changed: Vec<String>,
    /// Deployed easystacks. Empty means every installed module is a root.
    pub easystacks: Vec<PathBuf>,
    /// Scanner findings export. Each installed module it names is patched.
    pub findings: Option<PathBuf>,
    /// Site reachability file. Requires [`Self::findings`].
    pub reachability: Option<PathBuf>,
    /// Easyconfig trees, later ones taking precedence.
    pub easyconfigs: Vec<PathBuf>,
    /// Commit=directory trees for from-commit pins.
    pub commit_trees: Vec<String>,
    /// Installed modules, one `Name/VersionFull` per line.
    pub installed: PathBuf,
    /// Label for where the installed list came from.
    pub installed_label: String,
    /// Dependency specs to drop.
    pub filter_deps: Vec<String>,
    /// Lmod `.modulerc.lua` files, or trees to scan.
    pub installed_modulerc: Vec<PathBuf>,
    /// Installed easyblocks package roots.
    pub easyblocks: Vec<PathBuf>,
    /// Commit=directory for one include-easyblocks-from-commit.
    pub easyblocks_commits: Vec<String>,
    /// RFC 3339 timestamp. `None` means now, in UTC.
    pub generated_at: Option<String>,
    /// Installed prefix whose static archives are read.
    pub prefix: Option<PathBuf>,
    /// Compare defined dynamic symbols when `--prefix` is set.
    pub check_static: bool,
    /// Directory for `rebuild-set.json`, `.md`, and `rebuilds/`.
    pub out_dir: PathBuf,
}

/// Counts `rebuild-set` prints.
pub struct RebuildSetRun {
    /// Patched easyconfigs.
    pub changed: usize,
    /// Installed reverse dependents.
    pub rows: usize,
    /// Modules that must be rebuilt.
    pub must_rebuild: usize,
    /// Build-dependency dependents left for review.
    pub review: usize,
    /// Dependents that do not need a rebuild.
    pub no_rebuild: usize,
    /// Easyconfigs the closure could not find.
    pub missing: usize,
    /// Easystack files written under `rebuilds/`.
    pub easystacks: usize,
    /// Easyconfigs the parser skipped.
    pub skipped_easyconfigs: usize,
    /// Installed modules with a finding, when `--findings` was given.
    pub findings_modules: Option<usize>,
    /// Modules on the backport list, when `--findings` was given.
    pub backport_modules: Option<usize>,
    /// Findings in the fix-cost table, when `--findings` was given.
    pub fix_routes: Option<usize>,
    /// Bump, in-place, and recommended module counts, then the stack size.
    pub fix_cost: Option<(usize, usize, usize, usize)>,
    /// Directory the report was written to.
    pub out_dir: PathBuf,
}

impl RebuildSetRun {
    /// The line `eb-stack rebuild-set` prints.
    pub fn summary(&self) -> String {
        let findings = self
            .findings_modules
            .map(|count| {
                let backport = self.backport_modules.unwrap_or(0);
                let routes = self
                    .fix_cost
                    .zip(self.fix_routes)
                    .map(|((bump, in_place, recommended, stack), routes)| {
                        format!(
                            " fix_routes={routes} bump={bump}/{stack} in_place={in_place}/{stack} recommended={recommended}/{stack}"
                        )
                    })
                    .unwrap_or_default();
                format!(" findings_modules={count} backport={backport}{routes}")
            })
            .unwrap_or_default();
        format!(
            "changed={} rows={} must-rebuild={} review={} no-rebuild={} missing={} easystacks={} skipped_easyconfigs={}{findings} out={}",
            self.changed,
            self.rows,
            self.must_rebuild,
            self.review,
            self.no_rebuild,
            self.missing,
            self.easystacks,
            self.skipped_easyconfigs,
            self.out_dir.display()
        )
    }
}

/// Inputs of `eb-stack component-reach`.
pub struct ComponentReachRequest {
    /// Component name. ASCII case is ignored. One of this, `purl`, or `cve`.
    pub component: Option<String>,
    /// Exact version, `*`, or a requirement such as `>=8,<9`.
    pub version: Option<String>,
    /// Package URL. Replaces `component`.
    pub purl: Option<String>,
    /// CVE id, or another OSV id. Versions come from OSV.
    pub cve: Option<String>,
    /// Toolchain generation, for example `2026.1`.
    pub generation: String,
    /// Easyconfig trees.
    pub easyconfigs: Vec<PathBuf>,
    /// Installed modules, one per line.
    pub installed: Option<PathBuf>,
    /// Deployed easystacks. With none, the generation is the stack.
    pub easystacks: Vec<PathBuf>,
    /// Deployed easystacks fetched over HTTPS.
    pub easystack_urls: Vec<String>,
    /// Compat-layer components, `name version` per line.
    pub compat_layer: Option<PathBuf>,
    /// Dependency specs to drop before the closure.
    pub filter_deps: Vec<String>,
    /// RFC 3339 timestamp. `None` means now, in UTC.
    pub generated_at: Option<String>,
    /// Directory for `component-reach.toml`, `.json`, and `.md`.
    pub out_dir: PathBuf,
    /// Recorded OSV responses. Absent, the command asks api.osv.dev.
    pub osv_cache: Option<PathBuf>,
}

/// Counts `component-reach` prints.
pub struct ComponentReachRun {
    /// Component that was asked for.
    pub component: String,
    /// Version requirement that was asked for.
    pub version: String,
    /// Generation that was asked for.
    pub generation: String,
    /// Easyconfigs and compat entries that carry it.
    pub carriers: usize,
    /// Carrier recipes fed into the rebuild closure.
    pub changed: usize,
    /// Modules that must be rebuilt.
    pub must_rebuild: usize,
    /// Build-dependency dependents left for review.
    pub review: usize,
    /// Dependents that do not need a rebuild.
    pub no_rebuild: usize,
    /// Directory the report was written to.
    pub out_dir: PathBuf,
    /// The document written under `out_dir`. A CVE run is the CVE document.
    pub document: serde_json::Value,
}

impl ComponentReachRun {
    /// The line `eb-stack component-reach` prints.
    pub fn summary(&self) -> String {
        if let Some(cve) = self.document.get("cve").and_then(|value| value.as_str()) {
            let packages = self
                .document
                .get("results")
                .and_then(|value| value.as_array())
                .map(Vec::len)
                .unwrap_or(0);
            return format!(
                "cve={} packages={} generation={} carriers={} changed={} must-rebuild={} review={} no-rebuild={} out={}",
                cve,
                packages,
                self.generation,
                self.carriers,
                self.changed,
                self.must_rebuild,
                self.review,
                self.no_rebuild,
                self.out_dir.display()
            );
        }
        format!(
            "component={} version={} generation={} reach={} carriers={} changed={} must-rebuild={} review={} no-rebuild={} out={}",
            self.component,
            self.version,
            self.generation,
            self.document
                .get("reach")
                .and_then(|value| value.as_str())
                .unwrap_or("unclassified"),
            self.carriers,
            self.changed,
            self.must_rebuild,
            self.review,
            self.no_rebuild,
            self.out_dir.display()
        )
    }
}

/// Inputs of `eb-stack easyblock-reach`.
pub struct EasyblockReachRequest {
    /// Easyblock trees. Earlier roots win a class name.
    pub easyblocks: Vec<PathBuf>,
    /// Framework trees, indexed the same way.
    pub framework: Vec<PathBuf>,
    /// Easyconfig files or trees.
    pub easyconfigs: Vec<PathBuf>,
    /// Changed Python files.
    pub file: Vec<PathBuf>,
    /// `git diff --name-only` range, for example `base..HEAD`.
    pub range: Option<String>,
    /// Pull request number, fetched as `refs/pull/N/head`.
    pub pr: Option<u64>,
    /// Remote to fetch the pull request from.
    pub remote: String,
    /// Base branch for the pull request merge-base.
    pub base: String,
    /// Clone `git` reads. Defaults to the first easyblocks path when `None`.
    pub repo: Option<PathBuf>,
    /// Installed module names, one per line.
    pub installed: Option<PathBuf>,
    /// Also list modules that depend on a reached module.
    pub reverse: bool,
    /// RFC 3339 timestamp. `None` means now.
    pub generated_at: Option<String>,
    /// Directory for `reach.toml`, `reach.json`, and `reach.md`.
    pub out_dir: PathBuf,
}

/// Counts `easyblock-reach` prints.
pub struct EasyblockReachRun {
    /// Easyconfigs the change reaches.
    pub easyconfigs: usize,
    /// Installed modules the change reaches.
    pub installed: usize,
    /// Classes the change reaches.
    pub classes: usize,
    /// Reverse-dependency modules.
    pub reverse: usize,
    /// Directory the report was written to.
    pub out_dir: PathBuf,
}

impl EasyblockReachRun {
    /// The line `eb-stack easyblock-reach` prints.
    pub fn summary(&self) -> String {
        format!(
            "easyconfigs={} installed={} classes={} reverse={} out={}",
            self.easyconfigs,
            self.installed,
            self.classes,
            self.reverse,
            self.out_dir.display()
        )
    }
}

/// Write the pull-request overlap report.
///
/// The `pr-overlap` command and the `eb_pr_overlap` MCP tool both call this,
/// so a flag means the same thing in both places. `--open` uses [`pr_overlap::UreqGet`].
pub fn run_pr_overlap(args: &PrOverlapRequest) -> Result<PrOverlapRun, PrOverlapError> {
    run_pr_overlap_with(args, &pr_overlap::UreqGet)
}

/// [`run_pr_overlap`] with the client that reads `--open`.
pub fn run_pr_overlap_with(
    args: &PrOverlapRequest,
    http: &dyn pr_overlap::HttpGet,
) -> Result<PrOverlapRun, PrOverlapError> {
    if args.open_repo.is_some() && args.kind != OverlapKind::Easyconfigs {
        return Err(PrOverlapError::Failed(usage(
            "--open needs --kind easyconfigs",
        )));
    }
    if (args.bot_build_set || !args.arch_installed.is_empty())
        && args.kind != OverlapKind::SoftwareLayer
    {
        return Err(PrOverlapError::Failed(usage(
            "--bot-build-set and --arch-installed are only for --kind software-layer",
        )));
    }
    if !args.arch_installed.is_empty() && args.installed.is_some() {
        return Err(PrOverlapError::Failed(usage(
            "--arch-installed replaces --installed; pass every architecture as ARCH=FILE",
        )));
    }
    if !args.arch_installed.is_empty() && !args.installed_label.is_empty() {
        return Err(PrOverlapError::Failed(usage(
            "--installed-label applies to --installed; each --arch-installed name is the label",
        )));
    }
    if args.kind == OverlapKind::SoftwareLayer
        && args.installed.is_none()
        && args.arch_installed.is_empty()
    {
        return Err(PrOverlapError::InstalledRequired);
    }
    run_pr_overlap_inner(args, http).map_err(PrOverlapError::Failed)
}

fn run_pr_overlap_inner(
    args: &PrOverlapRequest,
    http: &dyn pr_overlap::HttpGet,
) -> Result<PrOverlapRun> {
    use pr_overlap::{record_to_toml, render_markdown, single_trailing_newline, to_toml, PrInput};
    if let Some(repo) = &args.open_repo {
        pr_overlap::validate_open_easyconfig_query(repo, args.base_ref.as_deref())
            .map_err(usage)?;
    }
    let mut prs: Vec<PrInput> = Vec::new();
    for path in &args.prs {
        prs.extend(pr_overlap::read_pr_inputs(path, "--pr")?);
    }
    if let Some(repo) = &args.open_repo {
        let mut query = pr_overlap::OpenEasyconfigQuery::new(repo);
        if let Some(base_ref) = args.base_ref.as_deref().filter(|base| !base.is_empty()) {
            query.base_ref = base_ref.to_string();
        }
        prs.extend(pr_overlap::fetch_open_easyconfig_prs(http, &query)?);
    }
    prs.sort_by_key(|pr| pr.number);
    prs.dedup_by_key(|pr| pr.number);

    let (robot, skipped_easyconfigs) = load_robot_trees(&args.easyconfigs, &args.commit_trees)?;
    let installed = match &args.installed {
        Some(path) => Some(load_installed_modules(path, &args.installed_modulerc)?),
        None => None,
    };
    let filter_specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter_deps = FilterDeps::parse(&filter_specs)
        .map_err(|error| usage(format!("--filter-deps: {error}")))?;
    let easyblocks = load_easyblock_catalog(&args.easyblocks, &args.easyblocks_commits)?;
    let easyconfigs = args.kind == OverlapKind::Easyconfigs;
    let mut config = OverlapConfig {
        eessi_version: if easyconfigs {
            String::new()
        } else {
            args.eessi_version.clone()
        },
        installed_source: args.installed_label.clone(),
        exclude_paths: args.exclude_paths.clone(),
        include_paths: args.include_paths.clone(),
        generated_at: generated_at(args.generated_at.clone(), true)?,
        filter_deps,
        easyblocks,
        build_scope: pr_overlap::BuildScope::Added,
    };
    let records: Vec<_> = if easyconfigs {
        let installed = installed.as_ref();
        prs.iter()
            .filter_map(|pr| {
                let others: Vec<_> = prs
                    .iter()
                    .filter(|other| other.number != pr.number)
                    .flat_map(|other| other.easyconfigs.iter().cloned())
                    .collect();
                easyconfig_record_seeing(pr, &robot.base, &others, installed, &config)
            })
            .collect()
    } else {
        let lists = installed_architectures(args, installed)?;
        let scope = if args.bot_build_set || !args.arch_installed.is_empty() {
            pr_overlap::BuildScope::ChangedEasystack
        } else {
            pr_overlap::BuildScope::Added
        };
        let mut groups = Vec::new();
        for (arch, modules) in &lists {
            let mut one = config.clone();
            one.installed_source = arch.clone();
            one.build_scope = scope;
            let arch_records: Vec<_> = prs
                .iter()
                .filter_map(|pr| pr_record(pr, &robot, modules, &one))
                .collect();
            groups.push((arch.clone(), arch_records));
        }
        let records = pr_overlap::combine_arch_records(groups);
        if records.iter().any(|record| !record.build_sets.is_empty()) {
            if let Some(source) = records.first() {
                config.installed_source = source.installed_source.clone();
            }
        }
        records
    };
    let index = overlap_index(&records, &config);
    let repo_url = args.repo_url.clone().unwrap_or_else(|| {
        if easyconfigs {
            "https://github.com/easybuilders/easybuild-easyconfigs".to_string()
        } else {
            "https://github.com/EESSI/software-layer".to_string()
        }
    });

    // Reject a --merged number that produced no record before any file is
    // created. --impact already requires a record; --merged matches it.
    // A number in the input with no record is a different error from a
    // number the input never contained.
    let input_numbers: Vec<u64> = prs.iter().map(|pr| pr.number).collect();
    let write_order = args.merge_order || !args.merged.is_empty();
    if write_order {
        let recorded: Vec<u64> = records.iter().map(|record| record.pr).collect();
        if let Some(number) = merge_order::unknown_merged(&recorded, &args.merged) {
            return Err(usage(rejected_pull_request(
                "--merged",
                number,
                &input_numbers,
            )));
        }
    }
    if !args.impact.is_empty() {
        for number in &args.impact {
            if args.merged.contains(number) {
                return Err(usage(format!(
                    "--impact {number}: already listed in --merged"
                )));
            }
        }
    }
    let multi_arch = records.iter().any(|record| !record.build_sets.is_empty());
    let arch_names: Vec<String> = records
        .iter()
        .find(|record| !record.build_sets.is_empty())
        .map(|record| {
            record
                .build_sets
                .iter()
                .map(|set| set.arch.clone())
                .collect()
        })
        .unwrap_or_default();
    let order = if write_order {
        if multi_arch {
            let mut arches = Vec::new();
            for arch in &arch_names {
                let view = pr_overlap::records_for_arch(&records, arch);
                let view_index = overlap_index(&view, &config);
                let one = merge_order::plan(&view, &view_index, &args.merged);
                arches.push(merge_order::ArchMerge {
                    arch: arch.clone(),
                    steps: one.steps,
                    cycles: one.cycles,
                });
            }
            let mut order = merge_order::plan(&[], &index, &args.merged);
            order.conflicts = merge_order::conflicts_from_records(&records);
            order.arches = arches;
            Some(order)
        } else {
            let mut order = merge_order::plan(&records, &index, &args.merged);
            order.conflicts = merge_order::conflicts_from_records(&records);
            Some(order)
        }
    } else {
        None
    };
    let impact = if args.impact.is_empty() {
        None
    } else if multi_arch {
        let mut arches = Vec::new();
        for arch in &arch_names {
            let view = pr_overlap::records_for_arch(&records, arch);
            let view_index = overlap_index(&view, &config);
            let one = merge_order::impact_report(
                &view,
                &view_index,
                &args.impact,
                &config.generated_at,
                &args.merged,
            )
            .map_err(|number| usage(rejected_pull_request("--impact", number, &input_numbers)))?;
            arches.push(merge_order::ArchImpact {
                arch: arch.clone(),
                impacts: one.impacts,
            });
        }
        Some(merge_order::MergeImpactReport {
            schema_version: merge_order::MERGE_IMPACT_SCHEMA_VERSION,
            generated_at: config.generated_at.clone(),
            impacts: Vec::new(),
            arches,
        })
    } else {
        Some(
            merge_order::impact_report(
                &records,
                &index,
                &args.impact,
                &config.generated_at,
                &args.merged,
            )
            .map_err(|number| usage(rejected_pull_request("--impact", number, &input_numbers)))?,
        )
    };

    let pr_dir = args.out_dir.join("prs");
    std::fs::create_dir_all(&pr_dir)?;
    for entry in std::fs::read_dir(&pr_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
            std::fs::remove_file(&path)?;
        }
    }
    for record in &records {
        std::fs::write(
            pr_dir.join(format!("{}.toml", record.pr)),
            record_to_toml(record)?,
        )?;
    }
    std::fs::write(args.out_dir.join("overlap.toml"), to_toml(&index)?)?;
    let mut markdown = render_markdown(&records, &index, &repo_url);
    if let Some(order) = &order {
        markdown.push_str(&merge_order::render_section(order, &repo_url));
    }
    let markdown = single_trailing_newline(markdown);
    std::fs::write(args.out_dir.join("overlap.md"), markdown)?;
    if let Some(order) = &order {
        std::fs::write(args.out_dir.join("merge-order.toml"), to_toml(order)?)?;
    }
    if let Some(report) = &impact {
        std::fs::write(args.out_dir.join("merge-impact.toml"), to_toml(report)?)?;
    }
    Ok(PrOverlapRun {
        prs: prs.len(),
        records: records.len(),
        shared_modules: if index.arches.is_empty() {
            index.shared.len()
        } else {
            index.arches.iter().map(|arch| arch.shared.len()).sum()
        },
        skipped_easyconfigs,
        out_dir: args.out_dir.clone(),
    })
}

/// Write the rebuild-set report.
pub fn run_rebuild_set(args: &RebuildSetRequest) -> Result<RebuildSetRun> {
    let installed = load_installed_modules(&args.installed, &args.installed_modulerc)?;
    if args.reachability.is_some() && args.findings.is_none() {
        return Err(usage("--reachability requires --findings"));
    }
    let findings = match &args.findings {
        Some(path) => Some(
            crate::findings::parse_findings(
                &std::fs::read_to_string(path)
                    .with_context(|| format!("--findings {}: not readable", path.display()))?,
                &installed,
            )
            .map_err(|error| usage(format!("--findings {}: {error}", path.display())))?,
        ),
        None => None,
    };
    let reach = match &args.reachability {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("--reachability {}: not readable", path.display()))?;
            crate::findings::ReachTable::builtin()
                .overlay(&text, path.display().to_string())
                .map_err(|error| usage(format!("--reachability {}: {error}", path.display())))?
        }
        None => crate::findings::ReachTable::builtin(),
    };
    let mut changed = if args.changed.is_empty() {
        Vec::new()
    } else {
        read_changed_names(&args.changed)?
    };
    if let Some(findings) = &findings {
        changed.extend(crate::findings::changed_easyconfigs(findings, &installed));
    }
    if changed.is_empty() {
        return Err(usage(if findings.is_some() {
            "--findings names no installed module"
        } else {
            "--changed named no easyconfig"
        }));
    }
    let (robot, skipped_easyconfigs) = load_robot_trees(&args.easyconfigs, &args.commit_trees)?;
    let filter_specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter_deps = FilterDeps::parse(&filter_specs)
        .map_err(|error| usage(format!("--filter-deps: {error}")))?;
    let filter_for_report = filter_deps.clone();
    let easyblocks = load_easyblock_catalog(&args.easyblocks, &args.easyblocks_commits)?;
    let mut easystacks = Vec::new();
    for path in &args.easystacks {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--easystack {}: not readable", path.display()))?;
        easystacks.push(StackEasystack {
            path: path.display().to_string(),
            text,
        });
    }
    if easystacks.is_empty() {
        easystacks.push(crate::findings::installed_easystack(&installed));
    }
    let evidence = if findings.is_some() {
        let mut ordered: Vec<&crate::domain::Candidate> = robot.base.iter().collect();
        for tree in robot.commits.values() {
            ordered.extend(tree.iter());
        }
        crate::fix_cost::scan_candidates(ordered, &installed)
    } else {
        crate::fix_cost::EvidenceScan::default()
    };
    let mut document = rebuild_set::rebuild_set(&rebuild_set::RebuildQuery {
        base: robot.base,
        commits: robot.commits,
        easystacks,
        changed,
        installed: installed.clone(),
        filter_deps,
        easyblocks,
        installed_source: args.installed_label.clone(),
        generated_at: generated_at(args.generated_at.clone(), true)?,
        prefix: args.prefix.clone(),
        check_static: args.check_static,
    })
    .map_err(|error| match error {
        rebuild_set::RebuildError::CheckStaticNeedsPrefix => usage("--check-static needs --prefix"),
        other => anyhow!(other),
    })?;
    document.reach = crate::findings::reach_for_easyconfigs(&reach, &document.changed);

    std::fs::create_dir_all(&args.out_dir)?;
    write_json_pretty(&args.out_dir.join("rebuild-set.json"), &document)?;
    std::fs::write(
        args.out_dir.join("rebuild-set.md"),
        rebuild_set::render_markdown(&document),
    )?;
    let rebuild_dir = args.out_dir.join("rebuilds");
    std::fs::create_dir_all(&rebuild_dir)?;
    for entry in std::fs::read_dir(&rebuild_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yml") {
            std::fs::remove_file(&path)?;
        }
    }
    let stacks = rebuild_set::rebuild_easystacks(&document);
    for stack in &stacks {
        std::fs::write(rebuild_dir.join(&stack.filename), &stack.body)?;
    }
    let findings_modules = match &findings {
        Some(findings) => {
            let summary = crate::findings::summarize(findings, &installed, &document, &reach);
            write_json_pretty(&args.out_dir.join("findings.json"), &summary)?;
            std::fs::write(
                args.out_dir.join("findings.md"),
                crate::findings::render_markdown(&summary),
            )?;
            std::fs::write(
                args.out_dir.join("findings.csv"),
                crate::findings::render_csv(&summary),
            )?;
            std::fs::write(
                args.out_dir.join("backport.md"),
                crate::findings::render_backport(&summary, &reach, findings, &filter_for_report),
            )?;
            let fix = crate::fix_cost::report(findings, &installed, &document, &evidence, &reach);
            write_json_pretty(&args.out_dir.join("fix-cost.json"), &fix)?;
            std::fs::write(
                args.out_dir.join("fix-cost.md"),
                crate::fix_cost::render_markdown(&fix),
            )?;
            std::fs::write(
                args.out_dir.join("fix-cost.tsv"),
                crate::fix_cost::render_tsv(&fix),
            )?;
            Some((
                summary.modules,
                summary.backport_modules,
                fix.findings,
                (
                    fix.rollup.bump_modules,
                    fix.rollup.in_place_modules,
                    fix.rollup.recommended_modules,
                    fix.rollup.stack_modules,
                ),
            ))
        }
        None => None,
    };
    Ok(RebuildSetRun {
        changed: document.changed.len(),
        rows: document.rows.len(),
        must_rebuild: document.counts.must_rebuild,
        review: document.counts.review,
        no_rebuild: document.counts.no_rebuild,
        missing: document.missing.len(),
        easystacks: stacks.len(),
        skipped_easyconfigs,
        findings_modules: findings_modules.as_ref().map(|(modules, _, _, _)| *modules),
        backport_modules: findings_modules
            .as_ref()
            .map(|(_, backport, _, _)| *backport),
        fix_routes: findings_modules.as_ref().map(|(_, _, routes, _)| *routes),
        fix_cost: findings_modules.map(|(_, _, _, cost)| cost),
        out_dir: args.out_dir.clone(),
    })
}

/// Closure totals of one CVE reach document.
fn cve_closure_totals(
    document: &crate::component_reach::CveReachDocument,
) -> (usize, usize, usize, usize) {
    let changed = document.results.iter().map(|item| item.changed.len()).sum();
    let must_rebuild = document
        .results
        .iter()
        .map(|item| item.closure_counts.must_rebuild)
        .sum();
    let mut review: usize = document
        .results
        .iter()
        .map(|item| item.closure_counts.review)
        .sum();
    let no_rebuild = document
        .results
        .iter()
        .map(|item| item.closure_counts.no_rebuild)
        .sum();
    if review == 0 && must_rebuild == 0 && no_rebuild == 0 && !document.unresolved.is_empty() {
        review = 1;
    }
    (changed, must_rebuild, review, no_rebuild)
}

/// Write the component-reach report.
pub fn run_component_reach(args: &ComponentReachRequest) -> Result<ComponentReachRun> {
    let git = crate::osv::CachingGitRanges::from_env();
    if let Some(dir) = &args.osv_cache {
        let client = crate::osv::load_recorded_osv(dir)
            .map_err(|error| anyhow!("--osv-cache {}: {error}", dir.display()))?;
        return run_component_reach_with(args, &client, &git);
    }
    run_component_reach_with(args, &crate::registry::UreqClient, &git)
}

fn run_component_reach_with(
    args: &ComponentReachRequest,
    client: &dyn crate::registry::RegistryClient,
    git: &dyn crate::osv::GitRangeSource,
) -> Result<ComponentReachRun> {
    use crate::component_reach::{
        component_reach, cve_reach, cve_to_toml, fetch_easystack_urls, render_cve_markdown,
        render_markdown, to_toml, ReachQuery, ReachScope,
    };
    use crate::osv::{parse_purl, resolve_vulnerability_with};

    // A purl the carrier model does not know is a usage error. Reject it
    // before any output directory is created.
    if let Some(purl) = &args.purl {
        let parsed = parse_purl(purl).map_err(|error| anyhow!(error))?;
        if !crate::component_reach::purl_carrier_known(&parsed.kind) {
            return Err(usage(format!(
                "purl type {} has no carrier model",
                parsed.kind
            )));
        }
    }
    if let Some(version) = &args.version {
        require_version_req(version)?;
    }
    let generated_at = generated_at(args.generated_at.clone(), true)?;
    let trees: Vec<&Path> = args.easyconfigs.iter().map(PathBuf::as_path).collect();
    let parsed = parse_easyconfig_trees(&trees).map_err(|error| {
        anyhow::Error::msg(error)
            .context(format!("--easyconfigs {}", listed_paths(&args.easyconfigs)))
    })?;
    let installed = args
        .installed
        .as_ref()
        .map(|path| {
            std::fs::read_to_string(path)
                .map(|text| pr_overlap::parse_installed(&text))
                .with_context(|| format!("--installed {}: not readable", path.display()))
        })
        .transpose()?;
    let mut easystacks = Vec::new();
    for path in &args.easystacks {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--easystack {}: not readable", path.display()))?;
        easystacks.push(StackEasystack {
            path: path.display().to_string(),
            text,
        });
    }
    easystacks.extend(
        fetch_easystack_urls(&args.easystack_urls, client).map_err(|error| anyhow!(error))?,
    );
    let compat_text = args
        .compat_layer
        .as_ref()
        .map(|path| {
            std::fs::read_to_string(path)
                .with_context(|| format!("--compat-layer {}: not readable", path.display()))
        })
        .transpose()?
        .unwrap_or_default();
    let compat = parse_compat_layer(&compat_text);
    let specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter =
        FilterDeps::parse(&specs).map_err(|error| usage(format!("--filter-deps: {error}")))?;
    let skipped = if parsed.skipped.is_empty() {
        None
    } else {
        Some(format!("{} easyconfigs skipped", parsed.skipped.len()))
    };
    std::fs::create_dir_all(&args.out_dir)?;
    if let Some(cve) = &args.cve {
        let resolution =
            resolve_vulnerability_with(cve, client, git).map_err(|error| anyhow!(error))?;
        let mut notes = resolution.notes;
        if let Some(skipped) = skipped {
            notes.push(skipped);
        }
        let document = cve_reach(
            &resolution.id,
            &resolution.components,
            &notes,
            &resolution.unresolved,
            &ReachScope {
                candidates: &parsed.candidates,
                generation: &args.generation,
                installed: installed.as_ref(),
                easystacks: &easystacks,
                compat: &compat,
                filter_deps: &filter,
                generated_at: &generated_at,
            },
        );
        std::fs::write(
            args.out_dir.join("component-reach.toml"),
            cve_to_toml(&document)?,
        )?;
        write_json_pretty(&args.out_dir.join("component-reach.json"), &document)?;
        std::fs::write(
            args.out_dir.join("component-reach.md"),
            render_cve_markdown(&document),
        )?;
        let carriers: usize = document
            .results
            .iter()
            .map(|item| item.carriers.len())
            .sum();
        return Ok(ComponentReachRun {
            component: String::new(),
            version: String::new(),
            generation: document.generation.clone(),
            carriers,
            changed: cve_closure_totals(&document).0,
            must_rebuild: cve_closure_totals(&document).1,
            review: cve_closure_totals(&document).2,
            no_rebuild: cve_closure_totals(&document).3,
            out_dir: args.out_dir.clone(),
            document: serde_json::to_value(&document)?,
        });
    }
    let (component, version_req, purl, ecosystem) = if let Some(purl) = &args.purl {
        let parsed = parse_purl(purl).map_err(|error| anyhow!(error))?;
        let version = args
            .version
            .clone()
            .or(parsed.version)
            .ok_or_else(|| usage("purl has no version; pass --version"))?;
        require_version_req(&version)?;
        (
            parsed.name,
            version,
            Some(purl.clone()),
            Some(parsed.ecosystem),
        )
    } else {
        let component = args
            .component
            .clone()
            .ok_or_else(|| usage("pass --component, --purl, or --cve"))?;
        let version = args
            .version
            .clone()
            .ok_or_else(|| usage("--version is required with --component"))?;
        require_version_req(&version)?;
        (component, version, None, None)
    };
    let mut document = component_reach(&ReachQuery {
        candidates: &parsed.candidates,
        component: &component,
        version_req: &version_req,
        also_reqs: &[],
        exact_versions: &[],
        purl: purl.as_deref(),
        ecosystem: ecosystem.as_deref(),
        generation: &args.generation,
        installed: installed.as_ref(),
        easystacks: &easystacks,
        compat: &compat,
        filter_deps: &filter,
        generated_at: &generated_at,
    });
    if let Some(skipped) = skipped {
        document.notes.push(skipped);
    }
    std::fs::write(
        args.out_dir.join("component-reach.toml"),
        to_toml(&document)?,
    )?;
    write_json_pretty(&args.out_dir.join("component-reach.json"), &document)?;
    std::fs::write(
        args.out_dir.join("component-reach.md"),
        render_markdown(&document),
    )?;
    Ok(ComponentReachRun {
        component: document.component.clone(),
        version: document.version_req.clone(),
        generation: document.generation.clone(),
        carriers: document.carriers.len(),
        changed: document.changed.len(),
        must_rebuild: document.closure_counts.must_rebuild,
        review: document.closure_counts.review,
        no_rebuild: document.closure_counts.no_rebuild,
        out_dir: args.out_dir.clone(),
        document: serde_json::to_value(&document)?,
    })
}

/// Write the easyblock-reach report.
pub fn run_easyblock_reach(args: &EasyblockReachRequest) -> Result<EasyblockReachRun> {
    if args.file.is_empty() && args.range.is_none() && args.pr.is_none() {
        return Err(usage("pass --file, --range, or --pr"));
    }
    if args.easyblocks.is_empty() && args.framework.is_empty() {
        return Err(usage("pass --easyblocks or --framework"));
    }
    if args.easyconfigs.is_empty() {
        return Err(usage("pass --easyconfigs"));
    }
    let mut changed = read_sources(&args.file).context("read --file")?;
    if args.range.is_some() || args.pr.is_some() {
        let repo = args
            .repo
            .clone()
            .or_else(|| args.easyblocks.first().cloned())
            .context("--repo or --easyblocks is the clone to diff")?;
        changed.extend(
            diff_sources(&DiffRequest {
                repo,
                range: args.range.clone(),
                pr: args.pr,
                remote: args.remote.clone(),
                base: args.base.clone(),
            })
            .map_err(|error| anyhow!(error.to_string()))
            .context("diff")?,
        );
    }
    let installed = match &args.installed {
        Some(path) => easyblock_reach::parse_installed(
            &std::fs::read_to_string(path)
                .with_context(|| format!("--installed {}: not readable", path.display()))?,
        ),
        None => Default::default(),
    };
    let mut roots = args.easyblocks.clone();
    roots.extend(args.framework.iter().cloned());
    let generated_at = generated_at(args.generated_at.clone(), false)?;
    let report = reach(&easyblock_reach::ReachQuery {
        roots,
        easyconfigs: args.easyconfigs.clone(),
        changed,
        installed,
        reverse: args.reverse,
        generated_at,
    })
    .map_err(|error| anyhow!(error.to_string()))?;
    write_report(&report, &args.out_dir).map_err(|error| anyhow!(error.to_string()))?;
    Ok(EasyblockReachRun {
        easyconfigs: report.easyconfig_count,
        installed: report.installed_count,
        classes: report.reached_classes.len(),
        reverse: report.reverse.len(),
        out_dir: args.out_dir.clone(),
    })
}

/// Release easyconfigs plus one tree per commit pin.
/// Arguments for `stack gap`.
pub struct StackGapRequest {
    /// Site module trees or listings. Repeatable.
    pub site: Vec<PathBuf>,
    /// Older EESSI module trees or listings users load beside the site. Repeatable.
    pub older: Vec<PathBuf>,
    /// Target module trees or listings. Repeat for a CPU tree and an accelerator tree.
    pub installed: Vec<PathBuf>,
    /// Easyconfig trees to find recipes in.
    pub easyconfigs: Vec<PathBuf>,
    /// Toolchain generation, `2026.1`.
    pub generation: String,
    /// `name<TAB>reason` lines for names the stack does not carry.
    pub exclude_file: Option<PathBuf>,
    /// Names filtered to the compatibility layer, as EESSI-extend sets them.
    pub filter_deps: Vec<String>,
    /// Open software-layer pull requests, `pr-overlap --pr` JSON. Repeatable.
    pub open_layer: Vec<PathBuf>,
    /// Open easybuild-easyconfigs pull requests, `pr-overlap --pr` JSON. Repeatable.
    pub open_easyconfigs: Vec<PathBuf>,
    /// Keep open software-layer easystacks of this EESSI version only, `2026.06`.
    pub eessi_version: Option<String>,
    /// Output directory.
    pub out_dir: PathBuf,
}

/// What `stack gap` found.
pub struct StackGapRun {
    /// One row per site package.
    pub rows: Vec<crate::gap::GapRow>,
    /// Where the files were written.
    pub out_dir: PathBuf,
}

impl StackGapRun {
    /// One line for the CLI.
    pub fn summary(&self) -> String {
        let count = |route: &str| self.rows.iter().filter(|r| r.route == route).count();
        format!(
            "site={} in-stack={} filtered={} open-layer={} recipe={} open-easyconfigs={} no-recipe={} excluded={} out={}",
            self.rows.len(),
            count("in-stack"),
            count("filtered"),
            count("open-layer"),
            count("recipe"),
            count("open-easyconfigs"),
            count("no-recipe"),
            count("excluded"),
            self.out_dir.display()
        )
    }
}

/// Compare a site's packages with a stack and name the recipe that closes
/// each gap. Writes `gap.tsv`, one TSV per route, `roots.txt` (the recipes
/// on the generation, the input of `stack batch --roots-file`), and
/// `bump-sources.txt` (the newest recipe on any toolchain for each name
/// without one), and `version-change.tsv` (old module versions moved to the
/// target EESSI version, gone, or kept site-only).
pub fn run_stack_gap(args: &StackGapRequest) -> Result<StackGapRun> {
    use std::collections::{BTreeMap, BTreeSet};
    if args.site.is_empty() && args.older.is_empty() {
        return Err(usage("pass at least one --site or --older module tree"));
    }
    if args.installed.is_empty() {
        return Err(usage("pass at least one --installed target tree"));
    }
    let mut site_inventory = crate::gap::ModuleInventory::new();
    for path in &args.site {
        merge_module_inventory(
            &mut site_inventory,
            crate::gap::inventory_from_module_source(path)
                .with_context(|| format!("--site {}", path.display()))?,
        );
    }
    for path in &args.older {
        merge_module_inventory(
            &mut site_inventory,
            crate::gap::inventory_from_module_source(path)
                .with_context(|| format!("--older {}", path.display()))?,
        );
    }
    let site: BTreeSet<String> = site_inventory.keys().cloned().collect();
    let mut stack_inventory = crate::gap::ModuleInventory::new();
    for path in &args.installed {
        merge_module_inventory(
            &mut stack_inventory,
            crate::gap::inventory_from_module_source(path)
                .with_context(|| format!("--installed {}", path.display()))?,
        );
    }
    let stack: BTreeSet<String> = stack_inventory.keys().cloned().collect();
    let filter_specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter = FilterDeps::parse(&filter_specs)
        .map_err(|error| usage(format!("--filter-deps: {error}")))?;
    let filtered: BTreeSet<String> = filter.names().map(|name| name.to_lowercase()).collect();
    let mut excluded = BTreeMap::new();
    if let Some(path) = &args.exclude_file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--exclude-file {}: not readable", path.display()))?;
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (name, reason) = line
                .split_once(['\t', ' '])
                .map(|(n, r)| (n.trim(), r.trim()))
                .unwrap_or((line, "excluded"));
            excluded.insert(name.to_string(), reason.to_string());
        }
    }
    let trees: Vec<&Path> = args.easyconfigs.iter().map(PathBuf::as_path).collect();
    let parsed = parse_easyconfig_trees(&trees).map_err(|error| {
        anyhow::Error::msg(error)
            .context(format!("--easyconfigs {}", listed_paths(&args.easyconfigs)))
    })?;
    let parent = crate::domain::Toolchain {
        name: "foss".into(),
        version: args.generation.clone(),
    };
    let mut members = crate::hierarchy::known_hierarchy(&parent)
        .map(|h| h.member_labels())
        .ok_or_else(|| {
            usage(format!(
                "--generation {} has no known toolchain hierarchy",
                args.generation
            ))
        })?;
    if let Ok(table) = crate::generation_lint::GenerationTable::bundled() {
        for extra in table.members(&args.generation) {
            if !members.iter().any(|label| label == &extra) {
                members.push(extra);
            }
        }
    }
    let mut layer_prs = Vec::new();
    for path in &args.open_layer {
        layer_prs.extend(crate::pr_overlap::read_pr_inputs(path, "--open-layer")?);
    }
    let mut easy_prs = Vec::new();
    for path in &args.open_easyconfigs {
        easy_prs.extend(crate::pr_overlap::read_pr_inputs(
            path,
            "--open-easyconfigs",
        )?);
    }
    let eessi_version = args
        .eessi_version
        .as_deref()
        .filter(|version| !version.is_empty());
    let open_layer = crate::gap::carries_from_layer_version(&layer_prs, eessi_version);
    let open_easyconfigs =
        crate::gap::carries_from_easyconfigs(&easy_prs, &args.generation, &members);
    let rows = crate::gap::gap(&crate::gap::GapInput {
        site: &site,
        stack: &stack,
        candidates: &parsed.candidates,
        generation: &args.generation,
        members: &members,
        excluded: &excluded,
        filtered: &filtered,
        open_layer: &open_layer,
        open_easyconfigs: &open_easyconfigs,
    });
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;
    crate::write_text(
        &args.out_dir.join("gap.tsv"),
        &crate::gap::render_tsv(&rows),
    )?;
    let version_changes = crate::gap::version_changes(&rows, &site_inventory, &stack_inventory);
    crate::write_text(
        &args.out_dir.join("version-change.tsv"),
        &crate::gap::render_version_changes(&version_changes),
    )?;
    for (route, filename) in crate::gap::ROUTE_FILES {
        let subset: Vec<_> = rows
            .iter()
            .filter(|row| row.route == *route)
            .cloned()
            .collect();
        crate::write_text(
            &args.out_dir.join(filename),
            &crate::gap::render_tsv(&subset),
        )?;
    }
    let roots: Vec<&str> = rows
        .iter()
        .filter(|row| row.route == "recipe")
        .filter_map(|row| row.recipe.as_deref())
        .collect();
    crate::write_text(
        &args.out_dir.join("roots.txt"),
        &format!("{}\n", roots.join("\n")),
    )?;
    let bumps: Vec<&str> = rows
        .iter()
        .filter_map(|r| r.newest_elsewhere.as_deref())
        .collect();
    crate::write_text(
        &args.out_dir.join("bump-sources.txt"),
        &format!("{}\n", bumps.join("\n")),
    )?;
    Ok(StackGapRun {
        rows,
        out_dir: args.out_dir.clone(),
    })
}

fn merge_module_inventory(
    into: &mut crate::gap::ModuleInventory,
    from: crate::gap::ModuleInventory,
) {
    for (name, versions) in from {
        into.entry(name).or_default().extend(versions);
    }
}

/// Arguments for `stack batch`.
pub struct StackBatchRequest {
    /// Root easyconfig file names.
    pub roots: Vec<String>,
    /// Robot-path easyconfig trees (the easyconfigs the roots come from).
    pub easyconfigs: Vec<PathBuf>,
    /// Commit=directory trees for from-commit pins the open pull requests use.
    pub commit_trees: Vec<String>,
    /// Installed modules, one `Name/VersionFull` per line.
    pub installed: PathBuf,
    /// Modulerc files with `module_version` aliases.
    pub installed_modulerc: Vec<PathBuf>,
    /// Open software-layer pull requests as `pr-overlap --pr` JSON.
    pub open_prs: Vec<PathBuf>,
    /// Read the open pull requests of this `OWNER/NAME` over HTTPS too.
    pub open_repo: Option<String>,
    /// EESSI version, `2026.06`.
    pub eessi_version: String,
    /// EasyBuild release the entries are built with, `5.4.1`.
    pub easybuild_version: String,
    /// Toolchain generation, `2026.1`.
    pub generation: String,
    /// `--filter-deps` as EESSI-extend sets it.
    pub filter_deps: Vec<String>,
    /// `NAME` or `NAME@GENERATION` allowed a second version by the
    /// `stack generation` second-version check `stack batch` runs.
    pub allow_versions: Vec<String>,
    /// Most roots per pull request.
    pub size: usize,
    /// easybuild-easyconfigs checkout: a root not in the release tag gets a
    /// `from-commit` pin to the last commit that touched its file.
    pub easyconfigs_repo: Option<PathBuf>,
    /// RFC 3339 time written into the plan; absent is now.
    pub generated_at: Option<String>,
    /// Architecture families to judge the plan against. Empty checks every
    /// family in the unsupported list.
    pub arches: Vec<String>,
    /// Replace the bundled unsupported list.
    pub unsupported: Option<PathBuf>,
    /// Replace the bundled known-failure table.
    pub known_failures: Option<PathBuf>,
    /// Replace the bundled build-memory profile table used by `stack oom`.
    pub oom_profile: Option<PathBuf>,
    /// Replace the bundled bot-resource table used by `stack oom`.
    pub oom_resources: Option<PathBuf>,
    /// The command line, written into every reproduce script.
    pub command_line: String,
    /// Output directory.
    pub out_dir: PathBuf,
}

/// What `stack batch` wrote.
pub struct StackBatchRun {
    /// Where the batches were written.
    pub out_dir: PathBuf,
    /// The plan.
    pub plan: crate::batch::BatchPlan,
    /// Sha256 over the inputs that decide the plan.
    pub digest: String,
}

impl StackBatchRun {
    /// One line for the CLI.
    pub fn summary(&self) -> String {
        format!(
            "batches={} roots={} waits={} unresolved={} deployed={} digest={} out={}",
            self.plan.batches.len(),
            self.plan
                .batches
                .iter()
                .map(|b| b.roots.len())
                .sum::<usize>(),
            self.plan.waits.len(),
            self.plan.unresolved.len(),
            self.plan.deployed.len(),
            self.digest,
            self.out_dir.display()
        )
    }
}

/// Plan dependency-disjoint software-layer pull requests and write each one's
/// easystacks, lock, SBOM, reproduce script, and pull-request body.
pub fn run_stack_batch(args: &StackBatchRequest) -> Result<StackBatchRun> {
    use crate::batch::{batch_slug, plan, OpenSet, RootSet};
    use crate::build_order::{module_name, ModuleKey};
    use crate::domain::{Candidate, LockPackage, SolverMeta, StackLock, Toolchain};
    use crate::easystack::{eessi_easystacks, EasystackOptions, EessiLayout};
    use pr_overlap::{FileChange, PrInput};
    use sha2::{Digest, Sha256};
    use std::collections::{BTreeMap, BTreeSet};

    if args.roots.is_empty() {
        return Err(usage("pass at least one --root or --roots-file entry"));
    }
    // Parse the second-version allow list before any output is written, so a
    // bad spec is a usage error with exit status 2.
    let allow_versions = args
        .allow_versions
        .iter()
        .map(|spec| crate::generation_lint::VersionAllow::parse(spec))
        .collect::<Result<Vec<_>>>()?;
    let (robot, _) = load_robot_trees(&args.easyconfigs, &args.commit_trees)?;
    let installed = load_installed_modules(&args.installed, &args.installed_modulerc)?;
    let filter_specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter_deps = FilterDeps::parse(&filter_specs)
        .map_err(|error| usage(format!("--filter-deps: {error}")))?;
    let stamp = generated_at(args.generated_at.clone(), true)?;
    let config = OverlapConfig {
        eessi_version: args.eessi_version.clone(),
        installed_source: "stack batch".into(),
        exclude_paths: Vec::new(),
        include_paths: Vec::new(),
        generated_at: stamp.clone(),
        filter_deps,
        easyblocks: None,
        build_scope: pr_overlap::BuildScope::Added,
    };
    let parent = Toolchain {
        name: "foss".into(),
        version: args.generation.clone(),
    };
    let members = crate::hierarchy::known_hierarchy(&parent)
        .map(|h| h.member_labels())
        .ok_or_else(|| {
            usage(format!(
                "--generation {} has no known toolchain hierarchy",
                args.generation
            ))
        })?;
    let layout = EessiLayout {
        eessi_version: args.eessi_version.clone(),
        easybuild_version: args.easybuild_version.clone(),
        generation: args.generation.clone(),
        members,
    };

    let mut root_names: Vec<String> = args.roots.clone();
    root_names.sort();
    root_names.dedup();
    let unsupported = match &args.unsupported {
        Some(path) => crate::arch_support::UnsupportedList::load(path)?,
        None => crate::arch_support::UnsupportedList::bundled()?,
    };
    let hits = crate::arch_support::unsupported_in_plan(
        &unsupported,
        &args.arches,
        &robot.base,
        &root_names,
    );
    if !hits.is_empty() {
        return Err(anyhow!(
            "{}",
            hits.iter()
                .map(|hit| hit.message())
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    let known_failures = match &args.known_failures {
        Some(path) => crate::arch_known_failures::KnownFailureList::load(path)?,
        None => crate::arch_known_failures::KnownFailureList::bundled()?,
    };
    let known_context = crate::arch_known_failures::PlanContext {
        eessi_version: Some(args.eessi_version.clone()),
        easybuild_version: Some(args.easybuild_version.clone()),
    };
    let mut root_sets = Vec::new();
    for (i, root) in root_names.iter().enumerate() {
        let path = layout.path_for(root).map_err(usage)?;
        let pr = PrInput {
            number: 1_000_000 + i as u64,
            head_sha: String::new(),
            title: Some(root.clone()),
            easystacks: vec![FileChange {
                path,
                base: None,
                head: Some(format!("easyconfigs:\n  - {root}\n")),
                base_blob: None,
                head_blob: None,
            }],
            files: Vec::new(),
            easyconfigs: Vec::new(),
            file_list_truncated: false,
        };
        let record = pr_record(&pr, &robot, &installed, &config);
        let (to_build, unresolved) = match record {
            Some(r) => (r.to_build.into_iter().collect(), r.unresolved),
            None => (Default::default(), vec!["no record".into()]),
        };
        root_sets.push(RootSet {
            root: root.clone(),
            to_build,
            unresolved,
        });
    }

    let mut open_inputs: Vec<PrInput> = Vec::new();
    for path in &args.open_prs {
        open_inputs.extend(pr_overlap::read_pr_inputs(path, "--open-prs")?);
    }
    if let Some(repo) = &args.open_repo {
        let mut query = pr_overlap::OpenEasyconfigQuery::new(repo);
        query.base_ref = "main".into();
        open_inputs.extend(pr_overlap::fetch_open_software_layer_prs(
            &pr_overlap::UreqGet,
            &query,
        )?);
    }
    open_inputs.sort_by_key(|pr| pr.number);
    open_inputs.dedup_by_key(|pr| pr.number);
    let open: Vec<OpenSet> = open_inputs
        .iter()
        .filter_map(|pr| pr_record(pr, &robot, &installed, &config))
        .map(|r| OpenSet {
            number: r.pr,
            to_build: r.to_build.into_iter().collect(),
        })
        .collect();

    let planned = plan(&root_sets, &open, args.size);

    // The digest covers what decides the plan, so two runs that print the same
    // digest planned from the same inputs.
    let mut hasher = Sha256::new();
    for line in [
        format!("eessi={}", args.eessi_version),
        format!("easybuild={}", args.easybuild_version),
        format!("generation={}", args.generation),
        format!("size={}", args.size),
    ] {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    for root in &root_names {
        hasher.update(format!("root={root}\n").as_bytes());
    }
    for module in &installed {
        hasher.update(format!("installed={module}\n").as_bytes());
    }
    for pr in &open_inputs {
        hasher.update(format!("open={} {}\n", pr.number, pr.head_sha).as_bytes());
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let candidates: BTreeMap<String, &Candidate> = robot
        .base
        .iter()
        .map(|c| (module_name(&ModuleKey::of(c)), c))
        .collect();
    let by_file: BTreeMap<String, &Candidate> = robot
        .base
        .iter()
        .filter_map(|c| {
            let file = c.easyconfig_path.rsplit('/').next()?.to_string();
            Some((file, c))
        })
        .collect();

    // The second-version check `stack generation` runs: a name with two
    // versions in one toolchain generation is a warning unless the allow list
    // accepts it. A version the generation table does not carry is skipped for
    // the whole run. `existing` starts as the installed modules turned into
    // easyconfig file names and grows with each batch's roots, so a pair split
    // across two batches is reported once, in the later batch.
    let generations = crate::generation_lint::GenerationTable::bundled()?;
    let check_versions = generations.generations(&args.eessi_version).is_some();
    let mut existing: BTreeSet<String> = installed
        .iter()
        .map(|module| format!("{}.eb", module.replacen('/', "-", 1)))
        .collect();

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;
    let mut plan_md = format!(
        "# Software-layer batches for EESSI {v}\n\nEasyBuild {x}, generation {g}, at most {s} roots per pull request.\nInputs digest `{digest}`.\n\n```\n{cmd}\n```\n\n",
        v = args.eessi_version,
        x = args.easybuild_version,
        g = args.generation,
        s = args.size,
        cmd = args.command_line
    );
    for batch in &planned.batches {
        let slug = batch_slug(batch);
        let dir = args.out_dir.join(&slug);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

        let mut options = EasystackOptions::new();
        let mut pins = BTreeMap::new();
        if let Some(repo) = &args.easyconfigs_repo {
            for root in &batch.roots {
                if let Some(commit) = release_pin(repo, &args.easybuild_version, root)? {
                    options
                        .entry(root.clone())
                        .or_default()
                        .insert("from-commit".into(), commit.clone());
                    pins.insert(root.clone(), commit);
                }
            }
        }
        let root_paths: Vec<&str> = batch.roots.iter().map(String::as_str).collect();
        let stacks = eessi_easystacks(&root_paths, &options, &layout).map_err(usage)?;
        let plan_entries: Vec<crate::easystack::EasystackEntry> = batch
            .roots
            .iter()
            .map(|root| crate::easystack::EasystackEntry {
                file: root.clone(),
                from_commit: pins.get(root).cloned(),
                include_easyblocks_from_commit: None,
                include_easyblocks_from_pr: None,
            })
            .collect();
        let known: Vec<crate::arch_known_failures::KnownHit> =
            crate::arch_known_failures::known_failures_in_plan(
                &known_failures,
                &args.arches,
                &robot.base,
                &plan_entries,
                &known_context,
                &|key| installed.contains(&module_name(key)),
            )
            .into_iter()
            .filter(|hit| hit.remedied.is_none())
            .collect();
        for (rel, yaml) in &stacks {
            crate::write_text(&dir.join("software-layer").join(rel), yaml)?;
        }
        let easystack_paths: Vec<PathBuf> = stacks
            .keys()
            .map(|rel| dir.join("software-layer").join(rel))
            .collect();
        let oom_report = crate::oom_profile::run_oom_check(&crate::oom_profile::OomCheckRequest {
            arches: args.arches.clone(),
            easystacks: easystack_paths,
            easyconfigs: args.easyconfigs.clone(),
            installed: Some(args.installed.clone()),
            profile: args.oom_profile.clone(),
            resources: args.oom_resources.clone(),
            nodes: Vec::new(),
            job_memory: None,
            hooks: None,
            parallel: None,
            max_parallel: None,
        })?;
        let oom = oom_report.warnings;
        let factor_warnings = oom_report.factor_warnings;

        // The second-version check this batch gets. `detected` ignores the
        // allow list, so a pair the reviewer accepted still records a `versions`
        // object in batch.json with a zero count; `warnings` are the pairs the
        // allow list did not accept, which also head the pr-body and plan.md
        // blocks. A batch with no second-version pair writes none of the three,
        // so its outputs stay byte-for-byte what they were.
        let (detected, warnings): (bool, Vec<String>) = if check_versions {
            let lint = |allow: &[crate::generation_lint::VersionAllow]| -> Result<Vec<String>> {
                let plan = crate::generation_lint::Plan {
                    label: None,
                    files: vec![crate::generation_lint::PlanFile {
                        path: stacks
                            .keys()
                            .next()
                            .cloned()
                            .unwrap_or_else(|| args.eessi_version.clone()),
                        added: batch.roots.clone(),
                        kept: Vec::new(),
                    }],
                };
                Ok(crate::generation_lint::lint_plan(
                    &generations,
                    &plan,
                    Some(&args.eessi_version),
                    &existing,
                    allow,
                )?
                .into_iter()
                .filter(|finding| finding.kind == crate::generation_lint::FindingKind::Versions)
                .map(|finding| finding.message)
                .collect())
            };
            (!lint(&[])?.is_empty(), lint(&allow_versions)?)
        } else {
            (false, Vec::new())
        };
        existing.extend(batch.roots.iter().cloned());

        let mut packages: Vec<LockPackage> = batch
            .modules
            .iter()
            .filter_map(|m| candidates.get(m))
            .map(|c| LockPackage {
                name: c.name.clone(),
                version: c.version.clone(),
                toolchain: c.toolchain.clone(),
                versionsuffix: c.versionsuffix.clone(),
                easyconfig_path: c.easyconfig_path.clone(),
            })
            .collect();
        packages.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
        let lock = StackLock {
            schema_version: 1,
            toolchain: parent.clone(),
            generation_label: Some(args.generation.clone()),
            packages,
            solver: SolverMeta {
                engine: "eb-stack stack batch".into(),
                engine_version: env!("CARGO_PKG_VERSION").into(),
                timestamp: stamp.clone(),
                criteria: Vec::new(),
            },
        };
        let lock_path = dir.join("lock.json");
        crate::write_json_pretty(&lock_path, &lock)?;
        let (sbom, _) = crate::sbom::lock_to_cyclonedx_from_recipes(&lock, &lock_path);
        crate::write_json_pretty(&dir.join("sbom.cdx.json"), &sbom)?;

        let toolchains: BTreeSet<String> = batch
            .roots
            .iter()
            .map(|r| match by_file.get(r) {
                Some(c) if crate::hierarchy::is_system_toolchain(&c.toolchain) => "system".into(),
                Some(c) => format!("{}/{}", c.toolchain.name, c.toolchain.version),
                None => "unknown".into(),
            })
            .collect();
        let names: Vec<String> = batch
            .roots
            .iter()
            .map(|r| match by_file.get(r) {
                Some(c) => format!("{} {}", c.name, c.version),
                None => r.trim_end_matches(".eb").to_string(),
            })
            .collect();
        let title = format!(
            "{{{}}}[{}] {}",
            args.eessi_version,
            toolchains.into_iter().collect::<Vec<_>>().join(","),
            names.join(", ")
        );
        let dry_runs: Vec<String> = stacks
            .keys()
            .map(|rel| format!("eb --easystack {rel} --robot --dry-run"))
            .collect();
        let reproduce = format!(
            "#!/bin/sh\n# Regenerate this batch: same inputs give digest {digest}.\nset -eu\n{cmd}\n\n# Inside EESSI {v} with EESSI-extend loaded, from the software-layer root:\n{dry}\n\n# The SBOM from the lock this batch wrote:\neb-stack stack sbom --lock {slug}/lock.json --out {slug}/sbom.cdx.json\n",
            cmd = args.command_line,
            v = args.eessi_version,
            dry = dry_runs.join("\n"),
        );
        crate::write_text(&dir.join("reproduce.sh"), &reproduce)?;
        let mut body = format!(
            "{title}\n\n{n} root(s), {m} module(s) to build on top of EESSI {v}; no other open pull request builds any of them.\n\nGenerated with:\n\n```\n{cmd}\n```\n\nDry run, from the software-layer root with EESSI-extend loaded:\n\n```\n{dry}\n```\n\nModules this pull request builds:\n\n{mods}\n\nSBOM: `sbom.cdx.json` (CycloneDX), from `eb-stack stack sbom --lock lock.json`.\n",
            n = batch.roots.len(),
            m = batch.modules.len(),
            v = args.eessi_version,
            cmd = args.command_line,
            dry = dry_runs.join("\n"),
            mods = batch
                .modules
                .iter()
                .map(|m| format!("- `{m}`"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        if !known.is_empty() {
            body.push_str(
                "\nKnown per-architecture failures, from `eb-stack stack known-failures`:\n\n```\n",
            );
            for hit in &known {
                body.push_str(&hit.message());
                body.push('\n');
            }
            body.push_str("```\n");
        }
        if !oom.is_empty() {
            body.push_str("\nPotential out-of-memory builds, from `eb-stack stack oom`:\n\n```\n");
            for warning in &oom {
                for line in warning.lines() {
                    body.push_str(&line);
                    body.push('\n');
                }
            }
            body.push_str("```\n");
        }
        if !factor_warnings.is_empty() {
            body.push_str(
                "\nNon-integer PARALLELISM_LIMITS factors, from `eb-stack stack oom`:\n\n```\n",
            );
            for line in &factor_warnings {
                body.push_str(line);
                body.push('\n');
            }
            body.push_str("```\n");
        }
        if !warnings.is_empty() {
            body.push_str(
                "\nSecond versions in one generation, from `eb-stack stack generation`:\n\n```\n",
            );
            for warning in &warnings {
                body.push_str(warning);
                body.push('\n');
            }
            body.push_str("```\n");
        }
        crate::write_text(&dir.join("pr-body.md"), &body)?;
        let mut record = serde_json::json!({
            "index": batch.index,
            "title": title,
            "roots": batch.roots,
            "modules": batch.modules,
            "pins": pins,
            "known_failures": known,
            "oom": {
                "warnings": oom.iter().map(crate::oom_profile::OomWarning::lines).collect::<Vec<_>>(),
                "count": oom.len(),
                "factor_warnings": factor_warnings,
            },
            "easystacks": stacks.keys().collect::<Vec<_>>(),
            "digest": digest,
            "command": args.command_line,
        });
        if detected {
            record["versions"] = serde_json::json!({
                "warnings": warnings,
                "count": warnings.len(),
            });
        }
        crate::write_json_pretty(&dir.join("batch.json"), &record)?;
        plan_md.push_str(&format!(
            "## {slug}\n\n{title}\n\n{} module(s) to build.\n\n",
            batch.modules.len()
        ));
        if !known.is_empty() {
            plan_md.push_str(&format!(
                "{} known per-architecture failure(s); `pr-body.md` has the remedies.\n\n",
                known.len()
            ));
        }
        if !oom.is_empty() {
            plan_md.push_str(&format!(
                "{} out-of-memory warning(s); `pr-body.md` has the remedies.\n\n",
                oom.len()
            ));
        }
        if !warnings.is_empty() {
            plan_md.push_str(&format!(
                "{} second-version warning(s); `pr-body.md` has the details.\n\n",
                warnings.len()
            ));
        }
    }
    if !planned.waits.is_empty() {
        plan_md.push_str("## Waiting on open pull requests\n\n");
        for w in &planned.waits {
            plan_md.push_str(&format!(
                "- `{}` waits on {}: shares {}\n",
                w.root,
                w.prs
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                w.shared.join(", ")
            ));
        }
        plan_md.push('\n');
    }
    if !planned.unresolved.is_empty() {
        plan_md.push_str("## Unresolved\n\n");
        for (root, missing) in &planned.unresolved {
            plan_md.push_str(&format!("- `{root}`: {}\n", missing.join(", ")));
        }
        plan_md.push('\n');
    }
    if !planned.deployed.is_empty() {
        plan_md.push_str("## Already installed\n\n");
        for root in &planned.deployed {
            plan_md.push_str(&format!("- `{root}`\n"));
        }
    }
    crate::write_text(&args.out_dir.join("plan.md"), &plan_md)?;
    crate::write_json_pretty(
        &args.out_dir.join("plan.json"),
        &serde_json::json!({
            "digest": digest,
            "command": args.command_line,
            "generated_at": stamp,
            "plan": planned,
        }),
    )?;
    Ok(StackBatchRun {
        out_dir: args.out_dir.clone(),
        plan: planned,
        digest,
    })
}

/// The `from-commit` pin a root needs: none when the EasyBuild release tag
/// already carries its file, else the last commit on `HEAD` that touched it.
fn release_pin(repo: &Path, easybuild_version: &str, file: &str) -> Result<Option<String>> {
    let git = |args: &[&str]| -> Result<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .with_context(|| format!("git in {}", repo.display()))?;
        if !out.status.success() {
            anyhow::bail!(
                "git {} in {}: {}",
                args.join(" "),
                repo.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let first = file.chars().next().unwrap_or('_').to_ascii_lowercase();
    let listed = git(&[
        "ls-tree",
        "-r",
        "--name-only",
        "HEAD",
        &format!("easybuild/easyconfigs/{first}"),
    ])?;
    let Some(path) = listed.lines().find(|p| p.ends_with(&format!("/{file}"))) else {
        return Ok(None);
    };
    let tag = format!("easybuild-easyconfigs-v{easybuild_version}");
    git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("{tag}^{{commit}}"),
    ])
    .with_context(|| format!("--easyconfigs-repo has no tag {tag}; fetch the tags first"))?;
    let in_release = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e", &format!("{tag}:{path}")])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if in_release {
        return Ok(None);
    }
    let commit = git(&["log", "-1", "--format=%H", "HEAD", "--", path])?;
    Ok((!commit.is_empty()).then_some(commit))
}

fn load_robot_trees(
    easyconfigs: &[PathBuf],
    commit_trees: &[String],
) -> Result<(pr_overlap::RobotTrees, usize)> {
    let trees: Vec<&Path> = easyconfigs.iter().map(PathBuf::as_path).collect();
    let base = parse_easyconfig_trees(&trees).map_err(|error| {
        anyhow::Error::msg(error).context(format!("--easyconfigs {}", listed_paths(easyconfigs)))
    })?;
    let skipped = base.skipped.len();
    let mut robot = pr_overlap::RobotTrees {
        base: base.candidates,
        commits: Default::default(),
    };
    for spec in commit_trees {
        let (commit, dir) = spec
            .split_once('=')
            .ok_or_else(|| usage(format!("--commit-tree wants <commit>=<dir>, got {spec}")))?;
        if commit.is_empty() || dir.is_empty() {
            return Err(usage(format!(
                "--commit-tree wants <commit>=<dir>, got {spec}"
            )));
        }
        let parsed = parse_easyconfig_tree(Path::new(dir))
            .map_err(|error| anyhow!(error).context(format!("--commit-tree {spec}")))?;
        robot.commits.insert(commit.to_string(), parsed.candidates);
    }
    Ok((robot, skipped))
}

/// Installed lists for a software-layer run, in the order they were given.
///
/// `--arch-installed` is one list per architecture and implies the bot build
/// set. Otherwise the single `--installed` list is used, labeled by
/// `--installed-label`.
fn installed_architectures(
    args: &PrOverlapRequest,
    installed: Option<std::collections::BTreeSet<String>>,
) -> Result<Vec<(String, std::collections::BTreeSet<String>)>> {
    use std::collections::BTreeSet;
    if !args.arch_installed.is_empty() {
        let mut seen = BTreeSet::new();
        let mut lists = Vec::new();
        for spec in &args.arch_installed {
            let (arch, path) = spec
                .split_once('=')
                .ok_or_else(|| usage(format!("--arch-installed wants ARCH=FILE, got {spec}")))?;
            let label_ok = !arch.is_empty()
                && arch
                    .chars()
                    .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\');
            if !label_ok || path.is_empty() {
                return Err(usage(format!(
                    "--arch-installed wants ARCH=FILE, got {spec}"
                )));
            }
            if !seen.insert(arch.to_string()) {
                return Err(usage(format!("--arch-installed {arch} was given twice")));
            }
            let modules = load_installed_modules(Path::new(path), &args.installed_modulerc)
                .with_context(|| format!("--arch-installed {spec}"))?;
            lists.push((arch.to_string(), modules));
        }
        return Ok(lists);
    }
    let modules = installed.expect("software-layer checks --installed");
    Ok(vec![(args.installed_label.clone(), modules)])
}

/// Installed modules, with `module_version` aliases from the modulerc files.
fn load_installed_modules(
    installed: &Path,
    modulerc: &[PathBuf],
) -> Result<std::collections::BTreeSet<String>> {
    let mut modules = pr_overlap::parse_installed(
        &std::fs::read_to_string(installed)
            .with_context(|| format!("--installed {}: not readable", installed.display()))?,
    );
    let mut aliases = Vec::new();
    for path in modulerc {
        aliases.extend(
            crate::modulerc::load_modulerc(path)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("--installed-modulerc {}", path.display()))?,
        );
    }
    modules = crate::modulerc::apply_modulerc(&modules, &aliases);
    Ok(modules)
}

/// `--changed` values. A path to a non-`.eb` file is a list of filenames.
fn read_changed_names(values: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for value in values {
        let path = Path::new(value);
        let ext_eb = path.extension().and_then(|ext| ext.to_str()) == Some("eb");
        let as_list = path.is_file() && !ext_eb;
        if !ext_eb && !path.is_file() && (value.contains('/') || value.contains('\\')) {
            return Err(anyhow!(
                "--changed {value}: not a file; a path that does not end in .eb is a list of easyconfig filenames"
            ));
        }
        if as_list {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                out.push(line.rsplit(['/', '\\']).next().unwrap_or(line).to_string());
            }
        } else {
            out.push(
                value
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(value)
                    .to_string(),
            );
        }
    }
    if out.is_empty() {
        return Err(usage("--changed named no easyconfig"));
    }
    Ok(out)
}

/// Installed easyblocks plus the files each commit pin touches.
fn load_easyblock_catalog(
    roots: &[PathBuf],
    commit_specs: &[String],
) -> Result<Option<pr_overlap::EasyblockCatalog>> {
    use std::collections::BTreeMap;
    if roots.is_empty() && commit_specs.is_empty() {
        return Ok(None);
    }
    let mut catalog = pr_overlap::EasyblockCatalog {
        check: !roots.is_empty(),
        ..pr_overlap::EasyblockCatalog::default()
    };
    if !roots.is_empty() {
        let paths: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
        let index = crate::eb_easyblock::index_easyblocks(&paths).map_err(|error| {
            anyhow::Error::msg(error).context(format!("--easyblocks {}", listed_paths(roots)))
        })?;
        for (class, (path, _)) in index {
            catalog.base.insert(class, path.display().to_string());
        }
    }
    for spec in commit_specs {
        let (commit, dir) = spec.split_once('=').ok_or_else(|| {
            usage(format!(
                "--easyblocks-commit wants <commit>=<dir>, got {spec}"
            ))
        })?;
        if commit.is_empty() || dir.is_empty() {
            return Err(usage(format!(
                "--easyblocks-commit wants <commit>=<dir>, got {spec}"
            )));
        }
        let index = crate::eb_easyblock::index_easyblocks(&[Path::new(dir)]).map_err(|error| {
            anyhow::Error::msg(error).context(format!("--easyblocks-commit {spec}"))
        })?;
        let mut classes = BTreeMap::new();
        for (class, (path, _)) in index {
            classes.insert(class, path.display().to_string());
        }
        catalog.commits.insert(commit.to_string(), classes);
    }
    Ok(Some(catalog))
}

/// Arguments for `stack shard`.
pub struct StackShardRequest {
    /// Jenkins build list of one node type.
    pub build_list: PathBuf,
    /// Robot-path easyconfig trees; later trees take precedence.
    pub easyconfigs: Vec<PathBuf>,
    /// `COMMIT=DIR`: the easyconfigs of one `--from-commit` pin.
    pub commit_trees: Vec<String>,
    /// `N=DIR`: the easyconfigs one `--from-pr` pull request changes.
    pub pr_trees: Vec<String>,
    /// Directory the trees of fetched lines are found in or downloaded to.
    pub commit_cache: Option<PathBuf>,
    /// Installed-module lists, one `Name/VersionFull` per line.
    pub installed: Vec<PathBuf>,
    /// Lmod `modules/all` trees whose modules count as installed.
    pub installed_modules_dirs: Vec<PathBuf>,
    /// Upper bound on the number of shards.
    pub shards: usize,
    /// `module<TAB>seconds` per line.
    pub weights: Option<PathBuf>,
    /// Weight of a module `weights` does not list.
    pub default_weight: u64,
    /// `--filter-deps` as EasyBuild takes it.
    pub filter_deps: Vec<String>,
    /// RFC 3339 time written into the plan; absent is now.
    pub generated_at: Option<String>,
    /// Output directory.
    pub out_dir: PathBuf,
    /// Opt-in: lift CPU-only prerequisites of accelerator roots into the
    /// prelude even when only one root needs them. Default is false (shared
    /// dependencies only), matching historical `stack shard` behaviour.
    pub lift_cpu_prerequisites: bool,
}

/// What `stack shard` wrote.
#[derive(Debug)]
pub struct StackShardRun {
    /// Shards written.
    pub shards: usize,
    /// Prelude modules.
    pub prelude: usize,
    /// CPU-prerequisite modules (opt-in lift). Zero when the flag is off.
    pub cpu_prerequisites: usize,
    /// Whether `--lift-cpu-prerequisites` was set.
    pub lift_cpu_prerequisites: bool,
    /// Build-list lines.
    pub lines: usize,
    /// Lines planned without a closure.
    pub opaque: usize,
    /// One line per opaque line and per other planning caveat, for stderr.
    pub warnings: Vec<String>,
}

impl StackShardRun {
    /// One line for the CLI. Legacy format when the CPU-lift flag is off.
    pub fn summary(&self) -> String {
        let mut text = if self.lift_cpu_prerequisites {
            format!(
                "shards={} prelude={} cpu_prerequisites={} lines={}",
                self.shards, self.prelude, self.cpu_prerequisites, self.lines
            )
        } else {
            format!(
                "shards={} prelude={} lines={}",
                self.shards, self.prelude, self.lines
            )
        };
        if self.opaque > 0 {
            text.push_str(&format!(" opaque={}", self.opaque));
        }
        text
    }
}

/// Modules under an Lmod `modules/all` tree, as `Name/Version` (the version
/// may hold further `/` levels). Hidden files, `default`, and `.modulerc`
/// files are skipped; a symlink is followed.
fn modules_under(root: &Path) -> Result<std::collections::BTreeSet<String>> {
    fn walk(
        dir: &Path,
        prefix: &str,
        out: &mut std::collections::BTreeSet<String>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "default" {
                continue;
            }
            let path = entry.path();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                walk(&path, &rel, out)?;
            } else if !prefix.is_empty() {
                out.insert(rel.strip_suffix(".lua").unwrap_or(&rel).to_string());
            }
        }
        Ok(())
    }
    let mut out = std::collections::BTreeSet::new();
    walk(root, "", &mut out).map_err(|error| {
        usage(format!(
            "--installed-modules-dir {}: {error}",
            root.display()
        ))
    })?;
    Ok(out)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Find an explicit filename using the robot trees' later-path precedence.
fn file_on_robot(roots: &[PathBuf], name: &str) -> Option<PathBuf> {
    for root in roots.iter().rev() {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if entry.file_name() != ".git" {
                        stack.push(path);
                    }
                } else if entry.file_name() == name {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Whether a file named `name` exists anywhere under one of `roots`.
fn file_under(roots: &[PathBuf], name: &str) -> bool {
    file_on_robot(roots, name).is_some()
}

/// Where a fetched source's easyconfigs were found.
enum FetchedTree {
    /// Directories: a `--commit-tree`, a `--pr-tree`, or a cache entry.
    Dirs(Vec<PathBuf>),
    /// A commit in `--easyconfigs-repo`.
    Git(crate::easyconfig_tree::Tree),
}

/// The trees of one fetched source, kept for lookups after planning.
struct SourceView {
    trees: Vec<crate::easyconfig_tree::Tree>,
    provider: &'static str,
}

impl SourceView {
    /// The path inside the tree of a candidate parsed from it.
    fn rel(&self, path: &str) -> Option<String> {
        self.trees.iter().find_map(|tree| {
            let prefix = tree.label("");
            let prefix = if prefix.ends_with('/') || prefix.ends_with(':') {
                prefix
            } else {
                format!("{prefix}/")
            };
            path.strip_prefix(&prefix).map(str::to_string)
        })
    }

    /// The names in `robot` (file name to the robot path's bytes) that a
    /// tree has an easyconfig of with other content. Each tree is read once.
    fn differing(
        &self,
        robot: &std::collections::BTreeMap<String, Option<Vec<u8>>>,
    ) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let mut left: std::collections::BTreeSet<&String> = robot.keys().collect();
        for tree in &self.trees {
            let found: Vec<(&String, String)> = left
                .iter()
                .filter_map(|name| tree.find_easyconfig(name).map(|rel| (*name, rel)))
                .collect();
            let rels: Vec<String> = found.iter().map(|(_, rel)| rel.clone()).collect();
            let Ok(read) = tree.read_all(&rels) else {
                continue;
            };
            for (name, rel) in found {
                left.remove(name);
                if robot[name].as_deref() != Some(read[&rel].as_slice()) {
                    out.insert(name.clone());
                }
            }
        }
        out
    }
}

/// `--build-robot-path` values, when `--materialize` is set.
fn build_robot_paths(extra: &StackShardSources) -> Result<Option<Vec<String>>> {
    match (extra.materialize, extra.build_robot_paths.is_empty()) {
        (false, true) => Ok(None),
        (false, false) => Err(usage("--build-robot-path is read only with --materialize")),
        (true, true) => Err(usage(
            "--materialize needs --build-robot-path: the robot-paths that `eb --show-config` reports on the build host",
        )),
        (true, false) => {
            for path in &extra.build_robot_paths {
                if !Path::new(path).is_absolute()
                    || path.contains([':', ','])
                    || path.chars().any(char::is_whitespace)
                {
                    return Err(usage(format!(
                        "--build-robot-path {path}: give one absolute path without ':', ',' or spaces"
                    )));
                }
            }
            Ok(Some(extra.build_robot_paths.clone()))
        }
    }
}

/// Split a build list into shards whose builds do not overlap, plus a prelude
/// of the dependencies two or more shards need.
///
/// A line with `--from-commit` or `--from-pr` is resolved the way EasyBuild
/// resolves it: against that commit's or pull request's easyconfigs first,
/// then the robot path. Its trees come from `--commit-tree`, `--pr-tree`, or
/// `--commit-cache`, which downloads a missing one. A prelude line for a
/// module only such a tree provides carries the line's source options. A line
/// eb-stack still cannot resolve is planned as an opaque line, and reported,
/// when EasyBuild fetches its easyconfig or the file is on the robot path but
/// does not parse; any other unresolved line writes nothing.
pub fn run_stack_shard(args: &StackShardRequest) -> Result<StackShardRun> {
    run_stack_shard_with(args, &pr_overlap::UreqGet)
}

/// [`run_stack_shard`] with downloads through `http`.
pub fn run_stack_shard_with(
    args: &StackShardRequest,
    http: &dyn pr_overlap::HttpGet,
) -> Result<StackShardRun> {
    run_stack_shard_with_sources(args, &StackShardSources::default(), http)
}

/// Where `stack shard` reads `--from-commit` trees from besides
/// [`StackShardRequest`], and whether it writes lists that run without
/// fetching them again.
#[derive(Debug, Clone, Default)]
pub struct StackShardSources {
    /// easybuild-easyconfigs git repository (a checkout, a bare repository,
    /// or a blobless clone) whose objects hold the commits of `--from-commit`
    /// lines. A commit found there is read in place; `--commit-cache` serves
    /// only a commit it lacks. `--commit-tree` still comes first.
    pub easyconfigs_repo: Option<PathBuf>,
    /// Write `sources/v1/<commit>/` with the files the plan reads from each
    /// commit, its manifest `sources/v1/<commit>.json`, and
    /// `prelude.local.list` and `shard-NN.local.list`, whose `--from-commit`
    /// lines read that directory instead of fetching the commit.
    pub materialize: bool,
    /// The build host's EasyBuild robot paths, in EasyBuild's order, as
    /// `eb --show-config` reports `robot-paths`. An executable line passes
    /// them, then its sources directory, as `--robot-paths`.
    pub build_robot_paths: Vec<String>,
}

/// [`run_stack_shard`] with the sources in `sources`.
pub fn run_stack_shard_sources(
    args: &StackShardRequest,
    sources: &StackShardSources,
) -> Result<StackShardRun> {
    run_stack_shard_with_sources(args, sources, &pr_overlap::UreqGet)
}

/// [`run_stack_shard_sources`] with downloads through `http`.
pub fn run_stack_shard_with_sources(
    args: &StackShardRequest,
    extra: &StackShardSources,
    http: &dyn pr_overlap::HttpGet,
) -> Result<StackShardRun> {
    use crate::accel_install::is_accelerator_candidate_filtered;
    use crate::build_order::{closure_against_installed, module_name, Choice, ModuleKey};
    use crate::domain::Candidate;
    use crate::eb_parse::merge_candidates_with_precedence;
    use crate::shard::{
        is_commit_sha, merge_orders, parse_lines, parse_weights, plan, plan_with, LineClosure,
        LineSource,
    };
    use crate::shard_trees::{overlay_candidates, TreeSources, EASYCONFIGS_REPO};
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    const FETCHED: &str = "fetched by EasyBuild (--from-pr or --from-commit)";
    const UNPARSED: &str = "on the robot path, not parsed by eb-stack";

    if args.shards == 0 {
        return Err(usage("--shards must be at least 1"));
    }
    if let Some(repo) = &extra.easyconfigs_repo {
        if crate::easyconfig_tree::git_output(repo, &["rev-parse", "--git-dir"])?.is_none() {
            return Err(usage(format!(
                "--easyconfigs-repo {} is not a git repository",
                repo.display()
            )));
        }
    }
    let build_robot_paths = build_robot_paths(extra)?;
    let mut commit_trees = BTreeMap::new();
    for spec in &args.commit_trees {
        let (commit, dir) = spec
            .split_once('=')
            .filter(|(c, d)| is_commit_sha(c) && !d.is_empty())
            .ok_or_else(|| usage(format!("--commit-tree wants COMMIT=DIR, got {spec}")))?;
        commit_trees.insert(commit.to_string(), PathBuf::from(dir));
    }
    let mut pr_trees = BTreeMap::new();
    for spec in &args.pr_trees {
        let (number, dir) = spec
            .split_once('=')
            .and_then(|(n, d)| Some((n.parse::<u64>().ok()?, d)))
            .filter(|(n, d)| *n > 0 && !d.is_empty())
            .ok_or_else(|| usage(format!("--pr-tree wants N=DIR, got {spec}")))?;
        pr_trees.insert(number, PathBuf::from(dir));
    }
    let trees = TreeSources {
        commit_trees,
        pr_trees,
        cache: args.commit_cache.clone(),
        http,
        repo: EASYCONFIGS_REPO.into(),
    };
    let list_bytes = std::fs::read(&args.build_list)
        .with_context(|| format!("--build-list {}: not readable", args.build_list.display()))?;
    let list_text = String::from_utf8_lossy(&list_bytes).into_owned();
    let lines = parse_lines(&list_text);
    if lines.is_empty() {
        return Err(usage(format!(
            "--build-list {}: no build-list lines",
            args.build_list.display()
        )));
    }
    let overrides: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            line.filter_deps_option().map(|option| {
                format!(
                    "line {}: {}: {option} overrides dependency filters for one line; \
                     pass --filter-deps or EASYBUILD_FILTER_DEPS for the whole list",
                    line.number, line.file
                )
            })
        })
        .collect();
    if !overrides.is_empty() {
        return Err(anyhow!(
            "{} dependency-filter override(s); nothing written\n{}",
            overrides.len(),
            overrides.join("\n")
        ));
    }
    let (weights, weights_digest) = match &args.weights {
        Some(path) => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("--weights {}: not readable", path.display()))?;
            let parsed = parse_weights(&String::from_utf8_lossy(&bytes))
                .map_err(|error| usage(format!("--weights {}: {error}", path.display())))?;
            (parsed, Some(sha256_hex(&bytes)))
        }
        None => (BTreeMap::new(), None),
    };
    let stamp = generated_at(args.generated_at.clone(), true)?;
    let filter_specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter = FilterDeps::parse(&filter_specs)
        .map_err(|error| usage(format!("--filter-deps: {error}")))?;

    let mut installed: BTreeSet<String> = BTreeSet::new();
    for path in &args.installed {
        installed.extend(pr_overlap::parse_installed(
            &std::fs::read_to_string(path)
                .with_context(|| format!("--installed {}: not readable", path.display()))?,
        ));
    }
    for dir in &args.installed_modules_dirs {
        installed.extend(modules_under(dir)?);
    }

    let (robot, _) = load_robot_trees(&args.easyconfigs, &[])?;
    let mut base = robot.base;
    let file_name = |c: &Candidate| -> String {
        c.easyconfig_path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    // Identity merging can discard a named alias with a distinct dependency
    // set. Explicit build-list filenames select their own robot candidate.
    let mut selected = Vec::new();
    let mut selected_names: BTreeMap<ModuleKey, String> = BTreeMap::new();
    for line in lines.iter().filter(|line| !line.fetched) {
        let candidate = base.iter().find(|c| file_name(c) == line.file).cloned();
        let candidate = candidate.or_else(|| {
            file_on_robot(&args.easyconfigs, &line.file)
                .and_then(|path| crate::eb_parse::parse_easyconfig_file(&path).ok())
        });
        if let Some(candidate) = candidate {
            if let Some(other) = selected_names.insert(ModuleKey::of(&candidate), line.file.clone())
            {
                if other != line.file {
                    return Err(usage(format!(
                        "{} and {} select different easyconfigs for the same module; nothing written",
                        other, line.file
                    )));
                }
            }
            selected.push(candidate);
        }
    }
    base = merge_candidates_with_precedence(&[base, selected]);
    let filtered = |mut universe: Vec<Candidate>| -> Vec<Candidate> {
        if !filter.is_empty() {
            filter.apply(&mut universe);
        }
        universe
    };

    // Universe 0 is the robot path. Each fetched source a tree was found for
    // is one more: the robot path first, then that tree, the order EasyBuild
    // searches after it appends the fetched files to the robot path. The
    // easyconfig a line names is the tree's, as EasyBuild replaces a named
    // file with the fetched copy.
    let mut sources: Vec<LineSource> = Vec::new();
    let mut overlays: Vec<Vec<Candidate>> = Vec::new();
    let mut views: Vec<SourceView> = Vec::new();
    let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut tree_status: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut source_failure: BTreeMap<LineSource, String> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    let wanted: BTreeSet<&LineSource> = lines
        .iter()
        .filter(|l| l.fetched)
        .filter_map(|l| l.source.as_ref())
        .filter(|s| !matches!(s, LineSource::Invalid(_)))
        .collect();
    let mut named_by: BTreeMap<&LineSource, BTreeSet<String>> = BTreeMap::new();
    for line in lines.iter().filter(|l| l.fetched) {
        if let Some(source) = &line.source {
            named_by
                .entry(source)
                .or_default()
                .insert(line.file.clone());
        }
    }
    let base_paths: BTreeSet<String> = base.iter().map(|c| c.easyconfig_path.clone()).collect();
    if !wanted.is_empty() && (trees.any() || extra.easyconfigs_repo.is_some()) {
        // SHA-256 of the robot path's effective easyconfig of each name, so a
        // tree's unchanged files are not parsed again.
        let mut base_digests: BTreeMap<String, String> = BTreeMap::new();
        for c in &base {
            if let Ok(bytes) = std::fs::read(&c.easyconfig_path) {
                base_digests.insert(file_name(c), sha256_hex(&bytes));
            }
        }
        let robot_names: BTreeSet<String> = base.iter().map(file_name).collect();
        let mut parsed: BTreeMap<PathBuf, Result<Vec<Candidate>, String>> = BTreeMap::new();
        for source in wanted {
            let roots = named_by.get(source).cloned().unwrap_or_default();
            let found: Result<FetchedTree, String> = match source {
                LineSource::Commit(sha) => {
                    match (trees.explicit_commit_tree(sha), &extra.easyconfigs_repo) {
                        (Some(dir), _) => dir.map(|d| FetchedTree::Dirs(vec![d])),
                        (None, Some(repo)) => match crate::easyconfig_tree::Tree::git(repo, sha)? {
                            Some(tree) => Ok(FetchedTree::Git(tree)),
                            None if trees.cache.is_some() => {
                                trees.commit_tree(sha).map(|d| FetchedTree::Dirs(vec![d]))
                            }
                            None => Err(format!(
                                "commit {sha} is not in --easyconfigs-repo {}",
                                repo.display()
                            )),
                        },
                        (None, None) => trees.commit_tree(sha).map(|d| FetchedTree::Dirs(vec![d])),
                    }
                }
                LineSource::Prs(prs) => prs
                    .iter()
                    .map(|n| trees.pr_tree(*n))
                    .collect::<Result<Vec<_>, _>>()
                    .map(FetchedTree::Dirs),
                LineSource::Invalid(_) => unreachable!("invalid sources are not wanted"),
            };
            // A cache miss that never reaches an HTTP status cannot fall back:
            // the tree was requested and the download did not happen. An HTTP
            // status still becomes an opaque line below.
            if let Err(why) = &found {
                if why.starts_with(crate::shard_trees::CACHE_DOWNLOAD_FAILED) {
                    return Err(anyhow!("{why}; nothing written"));
                }
            }
            let layers = found.and_then(|found| match found {
                FetchedTree::Dirs(dirs) => {
                    let mut layer = Vec::new();
                    let mut view = Vec::new();
                    for dir in dirs {
                        let tree = parsed.entry(dir.clone()).or_insert_with(|| {
                            let (candidates, skipped) = overlay_candidates(&dir, &base_digests)?;
                            for file in skipped {
                                warnings
                                    .push(format!("{}: does not parse: {file}", source.option()));
                            }
                            Ok(candidates)
                        });
                        layer.extend(tree.clone()?);
                        view.push(crate::easyconfig_tree::Tree::dir(&dir));
                    }
                    // A file the robot path also has loses to it, unless a
                    // line names it.
                    layer.retain(|c| {
                        let name = file_name(c);
                        !robot_names.contains(&name) || roots.contains(&name)
                    });
                    Ok((
                        layer,
                        SourceView {
                            trees: view,
                            provider: "directory",
                        },
                    ))
                }
                FetchedTree::Git(tree) => {
                    let (layer, skipped) = crate::shard_sources::git_overlay(
                        &tree,
                        &base_digests,
                        &roots,
                        &mut contents,
                    )?;
                    for file in skipped {
                        warnings.push(format!("{}: does not parse: {file}", source.option()));
                    }
                    Ok((
                        layer,
                        SourceView {
                            trees: vec![tree],
                            provider: "git",
                        },
                    ))
                }
            });
            match layers {
                Ok((layer, view)) => {
                    tree_status.insert(source.option(), None);
                    sources.push(source.clone());
                    overlays.push(layer);
                    views.push(view);
                }
                Err(why) => {
                    tree_status.insert(source.option(), Some(why.clone()));
                    source_failure.insert(source.clone(), why);
                }
            }
        }
    }
    let content = |path: &str| -> Option<Vec<u8>> {
        contents
            .get(path)
            .cloned()
            .or_else(|| std::fs::read(path).ok())
    };
    let universe_of = |source: &LineSource| -> Option<usize> {
        sources.iter().position(|s| s == source).map(|i| i + 1)
    };
    // The own easyconfig of every fetched line, from its tree, with the
    // source options that fetch it. A line that needs another line's module
    // finds it installed by that line when it builds, whichever tree that
    // module's easyconfig is in, so these sit under every universe at the
    // lowest precedence and the two lines share a shard.
    let mut fetched_roots: Vec<Candidate> = Vec::new();
    let mut root_options: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in &lines {
        let Some(u) = line.source.as_ref().and_then(universe_of) else {
            continue;
        };
        if let Some(c) = overlays[u - 1].iter().find(|c| file_name(c) == line.file) {
            root_options
                .entry(c.easyconfig_path.clone())
                .or_insert_with(|| line.source_options.clone());
            fetched_roots.push(c.clone());
        }
    }
    // The easyconfigs the lines of each fetched source name, from its tree.
    let named_roots: Vec<Vec<Candidate>> = sources
        .iter()
        .zip(&overlays)
        .map(|(source, layer)| {
            let names = named_by.get(source).cloned().unwrap_or_default();
            layer
                .iter()
                .filter(|c| names.contains(&file_name(c)))
                .cloned()
                .collect()
        })
        .collect();
    let build_universe = |u: usize| -> Vec<Candidate> {
        let mut layers = vec![fetched_roots.clone()];
        if u > 0 {
            layers.push(overlays[u - 1].clone());
        }
        layers.push(base.clone());
        if u > 0 {
            layers.push(named_roots[u - 1].clone());
        }
        filtered(merge_candidates_with_precedence(&layers))
    };
    let overlay_paths: Vec<BTreeSet<String>> = std::iter::once(BTreeSet::new())
        .chain(overlays.iter().map(|layer| {
            layer
                .iter()
                .map(|c| c.easyconfig_path.clone())
                .collect::<BTreeSet<_>>()
        }))
        .collect();

    // Resolve every line before computing anything, so a bad list fails with
    // every bad line named.
    let mut problems = Vec::new();
    let mut universe_for: Vec<Option<usize>> = vec![None; lines.len()];
    // A line eb-stack cannot resolve but EasyBuild can still build is planned
    // as an opaque line: its own group, no closure, the default weight.
    let mut opaque: Vec<Option<&'static str>> = vec![None; lines.len()];
    let mut opaque_reason: Vec<Option<String>> = vec![None; lines.len()];
    // A fetched line whose tree is missing falls back to the robot path when
    // its easyconfig is there; this is why.
    let mut fallback: Vec<Option<String>> = vec![None; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        if !line.file.ends_with(".eb") {
            problems.push(format!(
                "line {}: {} is not an easyconfig filename",
                line.number, line.file
            ));
            continue;
        }
        if !line.fetched {
            universe_for[i] = Some(0);
            continue;
        }
        match &line.source {
            Some(LineSource::Invalid(why)) => fallback[i] = Some(why.clone()),
            Some(source) => match universe_of(source) {
                Some(u) => universe_for[i] = Some(u),
                None => {
                    fallback[i] = Some(source_failure.get(source).cloned().unwrap_or_else(|| {
                        format!(
                            "no tree for {}: pass --commit-tree, --pr-tree, or --commit-cache",
                            source.option()
                        )
                    }))
                }
            },
            None => fallback[i] = Some("no --from-pr or --from-commit value".into()),
        }
        if fallback[i].is_some() {
            universe_for[i] = Some(0);
        }
    }

    // First pass, one universe at a time: each line's own module.
    let used: BTreeSet<usize> = universe_for.iter().flatten().copied().collect();
    let mut starts: Vec<Option<ModuleKey>> = vec![None; lines.len()];
    for &u in &used {
        let universe = build_universe(u);
        let by_file: BTreeMap<String, &Candidate> =
            universe.iter().map(|c| (file_name(c), c)).collect();
        for (i, line) in lines.iter().enumerate() {
            if universe_for[i] != Some(u) {
                continue;
            }
            match by_file.get(&line.file) {
                Some(c) => starts[i] = Some(ModuleKey::of(c)),
                None if line.fetched => {
                    opaque[i] = Some(FETCHED);
                    opaque_reason[i] = Some(match (&fallback[i], &line.source) {
                        (Some(why), _) => why.clone(),
                        (None, Some(source)) => format!(
                            "{} is in neither the tree of {} nor the robot path",
                            line.file,
                            source.option()
                        ),
                        (None, None) => "no tree".into(),
                    });
                }
                None if file_under(&args.easyconfigs, &line.file) => opaque[i] = Some(UNPARSED),
                None => problems.push(format!(
                    "line {}: {}: no such easyconfig on the robot path",
                    line.number, line.file
                )),
            }
        }
    }
    let forced: BTreeSet<ModuleKey> = lines
        .iter()
        .zip(&starts)
        .filter(|(line, _)| line.forced)
        .filter_map(|(_, key)| key.clone())
        .collect();
    let is_installed =
        |key: &ModuleKey| !forced.contains(key) && installed.contains(&module_name(key));

    // Second pass: closures, a dependency order per universe, and where each
    // module's easyconfig comes from (the first line that reaches it wins).
    struct Origin {
        file: String,
        path: String,
        options: Vec<String>,
    }
    let mut origin: BTreeMap<String, Origin> = BTreeMap::new();
    let mut closures: Vec<Option<LineClosure>> = vec![None; lines.len()];
    let mut orders: Vec<Vec<String>> = Vec::new();
    // Per line: its own easyconfig, and every easyconfig its closure reads.
    let mut root_paths: Vec<Option<String>> = vec![None; lines.len()];
    let mut members: Vec<Vec<String>> = vec![Vec::new(); lines.len()];
    // Robot-path easyconfigs a closure of a fetched line builds, checked
    // against that line's tree after the pass: (universe, line, module, path).
    let mut robot_built: Vec<(usize, usize, String, String)> = Vec::new();
    for &u in &used {
        let universe = build_universe(u);
        let by_key: BTreeMap<ModuleKey, &Candidate> =
            universe.iter().map(|c| (ModuleKey::of(c), c)).collect();
        let mut universe_starts = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if universe_for[i] != Some(u) || opaque[i].is_some() {
                continue;
            }
            let Some(start) = &starts[i] else { continue };
            let closure = closure_against_installed(
                &universe,
                std::slice::from_ref(start),
                Choice::Newest,
                &is_installed,
            );
            if !closure.unresolved.is_empty() {
                let needs: Vec<String> = closure
                    .unresolved
                    .iter()
                    .map(|r| format!("{} needs {}", module_name(&r.from), r.requirement))
                    .collect();
                if line.fetched {
                    // EasyBuild may still find it; plan the line apart.
                    opaque[i] = Some(FETCHED);
                    let tree = match &line.source {
                        Some(source) if u > 0 => format!("the tree of {} or ", source.option()),
                        _ => String::new(),
                    };
                    opaque_reason[i] = Some(format!(
                        "{}, which {tree}the robot path does not provide",
                        needs.join("; ")
                    ));
                    continue;
                }
                for need in needs {
                    problems.push(format!("line {}: {}: {need}", line.number, line.file));
                }
                continue;
            }
            if let Some(why) = &fallback[i] {
                warnings.push(format!(
                    "line {}: {}: closure over the robot path only: {why}",
                    line.number, line.file
                ));
            }
            root_paths[i] = by_key.get(start).map(|c| c.easyconfig_path.clone());
            for key in closure.to_build.iter().chain(&closure.installed) {
                let Some(c) = by_key.get(key) else { continue };
                members[i].push(c.easyconfig_path.clone());
                if u > 0
                    && closure.to_build.contains(key)
                    && base_paths.contains(&c.easyconfig_path)
                {
                    robot_built.push((u, i, module_name(key), c.easyconfig_path.clone()));
                }
                let options = if overlay_paths[u].contains(&c.easyconfig_path) {
                    line.source_options.clone()
                } else {
                    root_options
                        .get(&c.easyconfig_path)
                        .cloned()
                        .unwrap_or_default()
                };
                let module = module_name(key);
                match origin.get(&module) {
                    Some(known) if known.path != c.easyconfig_path => {
                        let differs = content(&known.path) != content(&c.easyconfig_path);
                        if differs && closure.to_build.contains(key) {
                            warnings.push(format!(
                                "line {}: {}: {module} resolves to {} here and to {} {} for an earlier line; the earlier one is planned",
                                line.number,
                                line.file,
                                c.easyconfig_path,
                                known.path,
                                known.options.join(" ")
                            ));
                        }
                    }
                    Some(_) => {}
                    None => {
                        origin.insert(
                            module,
                            Origin {
                                file: file_name(c),
                                path: c.easyconfig_path.clone(),
                                options,
                            },
                        );
                    }
                }
            }
            universe_starts.push(start.clone());
            closures[i] = Some(LineClosure {
                root: module_name(start),
                to_build: closure.to_build.iter().map(module_name).collect(),
            });
        }
        // One closure over every line of the universe gives a dependency
        // order for the prelude.
        orders.push(
            closure_against_installed(&universe, &universe_starts, Choice::Newest, &is_installed)
                .to_build
                .iter()
                .map(module_name)
                .collect(),
        );
    }
    // A robot-path easyconfig a fetched line builds, where its tree has a
    // file of the same name with other content.
    let mut shadowed: Vec<serde_json::Value> = Vec::new();
    for (u, view) in views.iter().enumerate().map(|(i, v)| (i + 1, v)) {
        let mut robot: BTreeMap<String, Option<Vec<u8>>> = BTreeMap::new();
        for (_, _, _, path) in robot_built.iter().filter(|(w, ..)| *w == u) {
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            robot.entry(name).or_insert_with(|| content(path));
        }
        let differing = view.differing(&robot);
        for (_, i, module, path) in robot_built.iter().filter(|(w, ..)| *w == u) {
            let name = path.rsplit('/').next().unwrap_or(path);
            if !differing.contains(name) {
                continue;
            }
            let source = sources[u - 1].option();
            warnings.push(format!(
                "line {}: {}: {module} builds from the robot path's {name}; the tree of {source} has a different {name}, which EasyBuild reads only for a module the robot path lacks",
                lines[*i].number, lines[*i].file
            ));
            shadowed.push(json!({
                "line": lines[*i].number,
                "easyconfig": lines[*i].file,
                "module": module,
                "robot_easyconfig": name,
                "source": source,
            }));
        }
    }
    if !problems.is_empty() {
        return Err(anyhow!(
            "{} problem(s) in the build list; nothing written\n{}",
            problems.len(),
            problems.join("\n")
        ));
    }
    let closures: Vec<LineClosure> = closures
        .into_iter()
        .zip(&lines)
        .map(|(closure, line)| {
            closure.unwrap_or_else(|| LineClosure {
                root: line.file.clone(),
                to_build: vec![line.file.clone()],
            })
        })
        .collect();
    let mut opaque_json = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(why) = opaque[i] else { continue };
        let reason = opaque_reason[i].as_deref();
        warnings.push(format!(
            "line {}: {}: opaque, planned without a closure: {}",
            line.number,
            line.file,
            reason.unwrap_or(why)
        ));
        opaque_json.push(json!({
            "line": line.number,
            "easyconfig": line.file,
            "opaque": why,
            "reason": reason,
        }));
    }

    let order = merge_orders(&orders);
    let easyconfig_of = |module: &str| -> String {
        origin
            .get(module)
            .map(|o| o.file.clone())
            .unwrap_or_else(|| module.to_string())
    };
    let prelude_line = |module: &str| -> String {
        match origin.get(module) {
            Some(o) if !o.options.is_empty() => format!("{} {}", o.file, o.options.join(" ")),
            _ => easyconfig_of(module),
        }
    };
    let weight = |module: &str| -> u64 {
        weights
            .get(module)
            .or_else(|| weights.get(&easyconfig_of(module)))
            .copied()
            .unwrap_or(args.default_weight)
    };
    // Map each planned module to the candidate its origin selected, so CPU vs
    // accelerator classification follows the effective robot / from-commit
    // recipe (including source pins), not the module-name spelling.
    let mut by_path: BTreeMap<String, Candidate> = BTreeMap::new();
    for &u in &used {
        for c in build_universe(u) {
            by_path
                .entry(c.easyconfig_path.clone())
                .or_insert_with(|| c);
        }
    }
    // EasyBuild 5.4.2 dependency_names() defaults combine build+runtime then
    // filter_deps. The universe already dropped filtered edges; re-apply the
    // same filter when classifying so a filtered CUDA builddep does not count.
    let is_accel_module = |module: &str| -> bool {
        origin
            .get(module)
            .and_then(|o| by_path.get(&o.path))
            .map(|c| is_accelerator_candidate_filtered(c, &filter))
            .unwrap_or(false)
    };
    let mut cpu_lift: BTreeSet<String> = BTreeSet::new();
    let mut cpu_root_lines: BTreeSet<usize> = BTreeSet::new();
    if args.lift_cpu_prerequisites {
        for (i, closure) in closures.iter().enumerate() {
            if opaque[i].is_some() {
                continue;
            }
            let root_is_cpu = !is_accel_module(&closure.root);
            if root_is_cpu {
                // CPU-only build-list root: emit under CPU settings, not an
                // accelerator shard. Its missing CPU closure joins cpu_lift so
                // a shared CPU dependency of two CPU roots does not stay in
                // the ordinary (accelerator) prelude.
                cpu_root_lines.insert(i);
            }
            for module in &closure.to_build {
                if module == &closure.root {
                    continue;
                }
                if !is_accel_module(module) {
                    cpu_lift.insert(module.clone());
                }
            }
        }
        // Refuse planning a CPU package that still needs an uninstalled
        // accelerator dependency from the same resolved closure (identity =
        // name/version/toolchain/versionsuffix via build_order::satisfies),
        // not any same-named candidate from an unrelated closure.
        let mut cpu_modules = cpu_lift.clone();
        for &i in &cpu_root_lines {
            cpu_modules.insert(closures[i].root.clone());
        }
        let mut cpu_problems = Vec::new();
        for module in &cpu_modules {
            let Some(o) = origin.get(module) else {
                continue;
            };
            let Some(c) = by_path.get(&o.path) else {
                continue;
            };
            let home: Vec<_> = closures
                .iter()
                .enumerate()
                .filter(|(i, cl)| opaque[*i].is_none() && cl.to_build.iter().any(|m| m == module))
                .map(|(_, cl)| cl)
                .collect();
            for dep in c
                .dependencies
                .iter()
                .chain(c.builddependencies.iter())
                .filter(|d| !filter.dep_is_filtered(&d.name, d.version_req.as_str()))
            {
                for cl in &home {
                    for dep_module in &cl.to_build {
                        if installed.contains(dep_module) {
                            continue;
                        }
                        let Some(dep_origin) = origin.get(dep_module) else {
                            continue;
                        };
                        let Some(dep_c) = by_path.get(&dep_origin.path) else {
                            continue;
                        };
                        if !crate::build_order::satisfies(dep_c, dep) {
                            continue;
                        }
                        if !is_accelerator_candidate_filtered(dep_c, &filter) {
                            continue;
                        }
                        cpu_problems.push(format!(
                            "CPU prerequisite {module} needs accelerator module {dep_module} \
                             ({}); install that under the accelerator prefix first",
                            dep_origin.file
                        ));
                    }
                }
            }
        }
        if !cpu_problems.is_empty() {
            cpu_problems.sort();
            cpu_problems.dedup();
            return Err(anyhow!(
                "{} CPU/accelerator conflict(s); nothing written\n{}",
                cpu_problems.len(),
                cpu_problems.join("\n")
            ));
        }
    }
    let planned = if !args.lift_cpu_prerequisites {
        plan(&closures, &order, &weight, args.shards)
    } else {
        plan_with(&closures, &order, &weight, args.shards, &cpu_lift)
    };

    // The digest covers what decides the plan: the list, the weights, the
    // installed modules, and every easyconfig a closure reached, by file name
    // (and source, for one a fetched tree provides) and content.
    let mut easyconfig_digests: BTreeMap<String, String> = BTreeMap::new();
    for o in origin.values() {
        let bytes = content(&o.path).with_context(|| format!("read {}", o.path))?;
        let key = if o.options.is_empty() {
            o.file.clone()
        } else {
            format!("{} {}", o.file, o.options.join(" "))
        };
        easyconfig_digests.insert(key, sha256_hex(&bytes));
    }
    let mut digest_input = format!(
        "shards={}\ndefault_weight={}\nlift_cpu_prerequisites={}\nbuild_list={}\n",
        args.shards,
        args.default_weight,
        args.lift_cpu_prerequisites,
        sha256_hex(&list_bytes)
    );
    if let Some(digest) = &weights_digest {
        digest_input.push_str(&format!("weights={digest}\n"));
    }
    for spec in &filter_specs {
        digest_input.push_str(&format!("filter_deps={spec}\n"));
    }
    for module in &installed {
        digest_input.push_str(&format!("installed={module}\n"));
    }
    for (file, digest) in &easyconfig_digests {
        digest_input.push_str(&format!("easyconfig={file} {digest}\n"));
    }
    for (source, status) in &tree_status {
        digest_input.push_str(&format!(
            "tree={source} {}\n",
            status.as_deref().unwrap_or("found")
        ));
    }
    let inputs_digest = sha256_hex(digest_input.as_bytes());

    let mut files: Vec<(String, String)> = vec![(
        "prelude.list".into(),
        planned
            .prelude
            .iter()
            .map(|m| format!("{}\n", prelude_line(m)))
            .collect(),
    )];
    // CPU prerequisite deps (easyconfig lines) then CPU-only root lines.
    let mut cpu_prereq_text = String::new();
    for m in &planned.cpu_prerequisites {
        cpu_prereq_text.push_str(&format!("{}\n", prelude_line(m)));
    }
    let mut cpu_root_json = Vec::new();
    for &i in &cpu_root_lines {
        cpu_prereq_text.push_str(&format!("{}\n", lines[i].text));
        cpu_root_json.push(json!({
            "line": lines[i].number,
            "text": lines[i].text,
            "easyconfig": lines[i].file,
            "module": closures[i].root,
        }));
    }
    if args.lift_cpu_prerequisites
        && (!planned.cpu_prerequisites.is_empty() || !cpu_root_lines.is_empty())
    {
        files.push(("cpu-prerequisites.list".into(), cpu_prereq_text.clone()));
    }
    let mut shard_json = Vec::new();
    let mut written_shards: Vec<(Vec<usize>, Vec<String>)> = Vec::new();
    for shard in &planned.shards {
        let shard_lines: Vec<usize> = shard
            .lines
            .iter()
            .copied()
            .filter(|i| !cpu_root_lines.contains(i))
            .collect();
        if shard_lines.is_empty() {
            // Empty after moving CPU-only roots out; do not write an accel shard.
            continue;
        }
        let shard_modules: Vec<String> = shard
            .modules
            .iter()
            .filter(|m| {
                !planned.cpu_prerequisites.iter().any(|c| c == *m)
                    && !cpu_root_lines.iter().any(|&i| closures[i].root == **m)
            })
            .cloned()
            .collect();
        written_shards.push((shard_lines, shard_modules));
    }
    let width = written_shards.len().to_string().len().max(2);
    let shard_name = |s: usize| format!("shard-{:0width$}", s + 1);
    let mut line_shard_name: Vec<Option<String>> = vec![None; lines.len()];
    for (s, (shard_lines, shard_modules)) in written_shards.iter().enumerate() {
        let name = shard_name(s);
        for &i in shard_lines {
            line_shard_name[i] = Some(name.clone());
        }
        files.push((
            format!("{}.list", name),
            shard_lines
                .iter()
                .map(|&i| format!("{}\n", lines[i].text))
                .collect(),
        ));
        shard_json.push(json!({
            "name": name,
            "file": format!("{}.list", name),
            "weight": shard_modules.iter().map(|m| weight(m)).sum::<u64>(),
            "lines": shard_lines.iter().map(|&i| lines[i].number).collect::<Vec<_>>(),
            "modules": shard_modules,
        }));
    }
    for &i in &cpu_root_lines {
        line_shard_name[i] = Some("cpu-prerequisites".into());
    }

    // With --materialize: the files each commit gives the planned closures,
    // and lists whose --from-commit lines read them instead of the commit.
    let mut source_writes: Vec<(usize, String, Vec<crate::shard_sources::SourceFile>)> = Vec::new();
    let mut materialized_json: Option<serde_json::Value> = None;
    if let Some(robot_paths) = &build_robot_paths {
        use crate::shard_sources::{
            executable_line, recipe_needs, unsupported_option, SOURCES_DIR,
        };
        let out_abs = std::path::absolute(&args.out_dir)
            .with_context(|| format!("--out-dir {}", args.out_dir.display()))?;
        let robot_trees: Vec<crate::easyconfig_tree::Tree> = args
            .easyconfigs
            .iter()
            .map(|p| crate::easyconfig_tree::Tree::dir(p))
            .collect();
        let source_dir = |u: usize| -> PathBuf {
            let LineSource::Commit(sha) = &sources[u - 1] else {
                unreachable!("only commit sources are written out")
            };
            out_abs.join(SOURCES_DIR).join(sha.to_ascii_lowercase())
        };
        // Per commit source: path in the sources directory to path in the commit.
        let mut selected: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
        let mut local_text: Vec<Option<String>> = vec![None; lines.len()];
        let mut line_entries = Vec::new();
        let mut unchanged = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if !line.fetched {
                continue;
            }
            let attempt = (|| -> Result<(usize, BTreeMap<String, String>, bool), String> {
                if let Some(why) = opaque[i] {
                    return Err(format!(
                        "planned without a closure: {}",
                        opaque_reason[i].as_deref().unwrap_or(why)
                    ));
                }
                if let Some(why) = &fallback[i] {
                    return Err(format!("no tree: {why}"));
                }
                let Some(u) = universe_for[i].filter(|u| *u > 0) else {
                    return Err("no tree".into());
                };
                if !matches!(sources[u - 1], LineSource::Commit(_)) {
                    return Err(format!(
                        "{} is not a --from-commit source",
                        sources[u - 1].option()
                    ));
                }
                if let Some(why) = unsupported_option(&line.text) {
                    return Err(why);
                }
                let view = &views[u - 1];
                let commit = &view.trees[0];
                let mut files: BTreeMap<String, String> = BTreeMap::new();
                // EasyBuild copies the named easyconfig from the commit whenever
                // the commit has it, even when the robot path has the same bytes,
                // and looks up its patches beside that copy.
                let root_from_commit = commit.find_easyconfig(&line.file);
                if let Some(rel) = &root_from_commit {
                    files.insert(line.file.clone(), rel.clone());
                    files.insert(rel.clone(), rel.clone());
                }
                for path in &members[i] {
                    let is_root = Some(path) == root_paths[i].as_ref();
                    if let (true, Some(rel)) = (is_root, &root_from_commit) {
                        let text = contents
                            .get(&commit.label(rel))
                            .cloned()
                            .or_else(|| {
                                commit
                                    .read_all(std::slice::from_ref(rel))
                                    .ok()
                                    .and_then(|mut read| read.remove(rel))
                            })
                            .and_then(|bytes| String::from_utf8(bytes).ok())
                            .ok_or_else(|| format!("{rel} is not readable text"))?;
                        let needs = recipe_needs(&text, (commit, ""), commit, &robot_trees)
                            .map_err(|why| format!("{}: {why}", line.file))?;
                        for rel in needs {
                            files.insert(rel.clone(), rel);
                        }
                        continue;
                    }
                    let text = content(path)
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .ok_or_else(|| format!("{path} is not readable text"))?;
                    let needs = if overlay_paths[u].contains(path) {
                        let rel = view
                            .rel(path)
                            .ok_or_else(|| format!("{path} is not in the commit tree"))?;
                        files.insert(rel.clone(), rel.clone());
                        let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                        recipe_needs(&text, (commit, dir), commit, &robot_trees)
                    } else if base_paths.contains(path) {
                        let parent = Path::new(path).parent().unwrap_or(Path::new("."));
                        let beside = crate::easyconfig_tree::Tree::at(parent);
                        recipe_needs(&text, (&beside, ""), commit, &robot_trees)
                    } else {
                        // Another line's own easyconfig: that line installs it first.
                        Ok(BTreeSet::new())
                    }
                    .map_err(|why| format!("{}: {why}", path.rsplit('/').next().unwrap_or(path)))?;
                    for rel in needs {
                        files.insert(rel.clone(), rel);
                    }
                }
                Ok((u, files, root_from_commit.is_some()))
            })();
            match attempt {
                Ok((u, files, root_from_commit)) => {
                    let known = selected.entry(u).or_default();
                    if let Some((path, _)) = files
                        .iter()
                        .find(|(path, from)| known.get(*path).is_some_and(|k| k != *from))
                    {
                        unchanged.push(json!({
                            "line": line.number,
                            "easyconfig": line.file,
                            "reason": format!("two commit files would be written to {path}"),
                        }));
                        continue;
                    }
                    known.extend(files);
                    let dir = source_dir(u);
                    let robot = format!("{}:{}", robot_paths.join(":"), dir.display());
                    let root = root_from_commit.then(|| dir.join(&line.file).display().to_string());
                    local_text[i] = Some(executable_line(&line.text, root.as_deref(), &robot));
                    line_entries.push(json!({
                        "line": line.number,
                        "easyconfig": line.file,
                        "source": sources[u - 1].option(),
                        "root": if root_from_commit { "commit" } else { "robot path" },
                    }));
                }
                Err(reason) => unchanged.push(json!({
                    "line": line.number,
                    "easyconfig": line.file,
                    "reason": reason,
                })),
            }
        }
        // A prelude module from a commit runs from its sources directory when
        // a written line's closure already selected it and what it reads.
        let mut prelude_local = String::new();
        let mut prelude_rewritten = Vec::new();
        for m in &planned.prelude {
            let rewritten = origin.get(m.as_str()).and_then(|o| {
                let u = (1..=sources.len()).find(|u| {
                    matches!(sources[u - 1], LineSource::Commit(_))
                        && o.options.contains(&sources[u - 1].option())
                        && overlay_paths[*u].contains(&o.path)
                })?;
                let view = &views[u - 1];
                let rel = view.rel(&o.path)?;
                let known = selected.get(&u)?;
                if known.get(&rel) != Some(&rel) {
                    return None;
                }
                let text = prelude_line(m);
                if unsupported_option(&text).is_some() {
                    return None;
                }
                let bytes = content(&o.path)?;
                let recipe = String::from_utf8(bytes).ok()?;
                let needs =
                    recipe_needs(&recipe, (&view.trees[0], ""), &view.trees[0], &robot_trees)
                        .ok()?;
                if known.get(&o.file).is_some_and(|k| *k != rel)
                    || needs.iter().any(|n| known.get(n).is_some_and(|k| k != n))
                {
                    return None;
                }
                Some((u, rel, needs, text, o.file.clone()))
            });
            match rewritten {
                Some((u, rel, needs, text, file)) => {
                    let known = selected.entry(u).or_default();
                    known.insert(file.clone(), rel);
                    for n in needs {
                        known.insert(n.clone(), n);
                    }
                    let dir = source_dir(u);
                    let robot = format!("{}:{}", robot_paths.join(":"), dir.display());
                    let root = dir.join(&file).display().to_string();
                    prelude_local.push_str(&executable_line(&text, Some(&root), &robot));
                    prelude_local.push('\n');
                    prelude_rewritten.push(m.clone());
                }
                None => {
                    prelude_local.push_str(&prelude_line(m));
                    prelude_local.push('\n');
                }
            }
        }
        for dir in selected.keys().map(|u| source_dir(*u)) {
            let text = dir.display().to_string();
            if text.contains([':', ',']) || text.chars().any(char::is_whitespace) {
                return Err(usage(format!(
                    "--materialize: {text} holds a ':', ',' or space, which a robot path cannot"
                )));
            }
        }
        let mut lists = vec!["prelude.local.list".to_string()];
        files.push(("prelude.local.list".into(), prelude_local));
        // Same rewrite path for CPU prerequisites that came from a commit.
        let mut cpu_local = String::new();
        let mut cpu_rewritten = Vec::new();
        for m in &planned.cpu_prerequisites {
            let rewritten = origin.get(m.as_str()).and_then(|o| {
                let u = (1..=sources.len()).find(|u| {
                    matches!(sources[u - 1], LineSource::Commit(_))
                        && o.options.contains(&sources[u - 1].option())
                        && overlay_paths[*u].contains(&o.path)
                })?;
                let view = &views[u - 1];
                let rel = view.rel(&o.path)?;
                let known = selected.get(&u)?;
                if known.get(&rel) != Some(&rel) {
                    return None;
                }
                let text = prelude_line(m);
                if unsupported_option(&text).is_some() {
                    return None;
                }
                let bytes = content(&o.path)?;
                let recipe = String::from_utf8(bytes).ok()?;
                let needs =
                    recipe_needs(&recipe, (&view.trees[0], ""), &view.trees[0], &robot_trees)
                        .ok()?;
                if known.get(&o.file).is_some_and(|k| *k != rel)
                    || needs.iter().any(|n| known.get(n).is_some_and(|k| k != n))
                {
                    return None;
                }
                Some((u, rel, needs, text, o.file.clone()))
            });
            match rewritten {
                Some((u, rel, needs, text, file)) => {
                    let known = selected.entry(u).or_default();
                    known.insert(file.clone(), rel);
                    for n in needs {
                        known.insert(n.clone(), n);
                    }
                    let dir = source_dir(u);
                    let robot = format!("{}:{}", robot_paths.join(":"), dir.display());
                    let root = dir.join(&file).display().to_string();
                    cpu_local.push_str(&executable_line(&text, Some(&root), &robot));
                    cpu_local.push('\n');
                    cpu_rewritten.push(m.clone());
                }
                None => {
                    cpu_local.push_str(&prelude_line(m));
                    cpu_local.push('\n');
                }
            }
        }
        for &i in &cpu_root_lines {
            cpu_local.push_str(local_text[i].as_deref().unwrap_or(&lines[i].text));
            cpu_local.push('\n');
        }
        if args.lift_cpu_prerequisites
            && (!planned.cpu_prerequisites.is_empty() || !cpu_root_lines.is_empty())
        {
            files.push(("cpu-prerequisites.local.list".into(), cpu_local));
            lists.insert(0, "cpu-prerequisites.local.list".to_string());
        }
        for (s, (shard_lines, _)) in written_shards.iter().enumerate() {
            let name = format!("{}.local.list", shard_name(s));
            files.push((
                name.clone(),
                shard_lines
                    .iter()
                    .map(|&i| format!("{}\n", local_text[i].as_deref().unwrap_or(&lines[i].text)))
                    .collect(),
            ));
            lists.push(name);
        }
        for (u, chosen) in &selected {
            let LineSource::Commit(sha) = &sources[u - 1] else {
                continue;
            };
            let chosen: Vec<crate::shard_sources::SourceFile> = chosen
                .iter()
                .map(|(path, from)| crate::shard_sources::SourceFile {
                    path: path.clone(),
                    from: from.clone(),
                })
                .collect();
            source_writes.push((*u, sha.to_ascii_lowercase(), chosen));
        }
        materialized_json = Some(json!({
            "schema_version": crate::shard_sources::SOURCES_SCHEMA_VERSION,
            "robot_paths": robot_paths,
            "lists": lists,
            "lines": line_entries,
            "unchanged": unchanged,
            "prelude": prelude_rewritten,
            "cpu_prerequisites": cpu_rewritten,
        }));
    }

    let line_json: Vec<_> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            json!({
                "line": line.number,
                "text": line.text,
                "easyconfig": line.file,
                "module": closures[i].root,
                "source": line.source.as_ref().map(LineSource::option),
                "group": planned.group_of[i] + 1,
                "shard": line_shard_name[i],
                "closure": closures[i].to_build,
                "opaque": opaque[i],
                "opaque_reason": opaque_reason[i],
            })
        })
        .collect();
    let module_meta = |modules: &[String]| -> Vec<serde_json::Value> {
        modules
            .iter()
            .map(|m| {
                json!({
                    "module": m,
                    "easyconfig": easyconfig_of(m),
                    "options": origin.get(m.as_str()).map(|o| o.options.clone()).unwrap_or_default(),
                    "weight": weight(m),
                })
            })
            .collect()
    };
    let mut document = json!({
        "schema_version": 1,
        "generated_at": stamp,
        "inputs": {
            "sha256": inputs_digest,
            "build_list_sha256": sha256_hex(&list_bytes),
            "weights_sha256": weights_digest,
            "installed_modules": installed.len(),
            "filter_deps": filter_specs,
            "easyconfigs": easyconfig_digests,
            "trees": tree_status,
        },
        "shards_requested": args.shards,
        "default_weight": args.default_weight,
        "lift_cpu_prerequisites": args.lift_cpu_prerequisites,
        "prelude": {
            "file": "prelude.list",
            "weight": planned.prelude_weight,
            "modules": module_meta(&planned.prelude),
        },
        "shards": shard_json,
        "lines": line_json,
        "opaque": opaque_json,
        "warnings": warnings,
    });
    if args.lift_cpu_prerequisites {
        document["cpu_prerequisites"] = json!({
            "file": if planned.cpu_prerequisites.is_empty() && cpu_root_lines.is_empty() {
                serde_json::Value::Null
            } else {
                json!("cpu-prerequisites.list")
            },
            "weight": planned.cpu_prerequisites_weight,
            "modules": module_meta(&planned.cpu_prerequisites),
            "roots": cpu_root_json,
        });
    }

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;
    // A rerun with fewer shards must not leave an old shard list behind for
    // the pipeline to pick up.
    let written: BTreeSet<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    for entry in std::fs::read_dir(&args.out_dir)
        .with_context(|| format!("read {}", args.out_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = (name.starts_with("shard-") && name.ends_with(".list"))
            || name == "prelude.local.list"
            || name == "cpu-prerequisites.list"
            || name == "cpu-prerequisites.local.list";
        if ours && !written.contains(name.as_str()) {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("remove stale {}", entry.path().display()))?;
        }
    }
    let sources_root = args.out_dir.join(crate::shard_sources::SOURCES_DIR);
    if sources_root.exists() {
        std::fs::remove_dir_all(&sources_root)
            .with_context(|| format!("remove stale {}", sources_root.display()))?;
    }
    let mut sources_json = BTreeMap::new();
    for (u, sha, chosen) in &source_writes {
        let view = &views[u - 1];
        let manifest = sources_root.join(format!("{sha}.json"));
        let written = crate::shard_sources::write_sources(
            &view.trees[0],
            sha,
            view.provider,
            &sources_root.join(sha),
            &manifest,
            chosen,
        )?;
        sources_json.insert(
            sha.clone(),
            json!({
                "directory": format!("{}/{sha}", crate::shard_sources::SOURCES_DIR),
                "manifest": format!("{}/{sha}.json", crate::shard_sources::SOURCES_DIR),
                "provider": view.provider,
                "files": chosen.len(),
                "sha256": written["sha256"],
            }),
        );
    }
    if !shadowed.is_empty() {
        document["shadowed"] = json!(shadowed);
    }
    if let Some(mut materialized) = materialized_json {
        materialized["sources"] = json!(sources_json);
        document["materialized"] = materialized;
    }
    for (name, text) in &files {
        crate::write_text(&args.out_dir.join(name), text)?;
    }
    crate::write_json_pretty(&args.out_dir.join("plan.json"), &document)?;
    Ok(StackShardRun {
        shards: written_shards.len(),
        prelude: planned.prelude.len(),
        cpu_prerequisites: planned.cpu_prerequisites.len() + cpu_root_lines.len(),
        lift_cpu_prerequisites: args.lift_cpu_prerequisites,
        lines: lines.len(),
        opaque: opaque_json.len(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MapClient;
    use std::collections::BTreeMap;

    #[test]
    fn a_cve_summary_sums_the_closure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("cURL.eb"),
            "name = 'cURL'\nversion = '8.17.0'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("git.eb"),
            "name = 'git'\nversion = '2'\ntoolchain = {'name': 'GCCcore', 'version': '15.2.0'}\ndependencies = [('cURL', '8.17.0')]\n",
        )
        .unwrap();
        let mut pages = BTreeMap::new();
        pages.insert(
            "https://api.osv.dev/v1/vulns/CVE-2024-0001".into(),
            br#"{"id":"CVE-2024-0001","affected":[{"package":{"ecosystem":"generic","name":"curl","purl":"pkg:generic/curl"},"versions":["8.17.0"]}]}"#
                .to_vec(),
        );
        let out = root.join("out");
        let report = run_component_reach_with(
            &ComponentReachRequest {
                component: None,
                version: None,
                purl: None,
                cve: Some("CVE-2024-0001".into()),
                generation: "2026.1".into(),
                easyconfigs: vec![root.to_path_buf()],
                installed: None,
                easystacks: Vec::new(),
                easystack_urls: Vec::new(),
                compat_layer: None,
                filter_deps: Vec::new(),
                generated_at: Some("2026-10-24T00:00:00Z".into()),
                out_dir: out,
                osv_cache: None,
            },
            &MapClient { pages },
            &crate::osv::NoGit,
        )
        .unwrap();
        assert!(report.carriers >= 1, "carriers {}", report.carriers);
        assert!(
            report.must_rebuild >= 1,
            "must_rebuild {} summary {}",
            report.must_rebuild,
            report.summary()
        );
        assert!(report.changed >= 1, "changed {}", report.changed);
        let summary = report.summary();
        assert!(
            summary.contains("must-rebuild=1")
                || summary.contains(&format!("must-rebuild={}", report.must_rebuild)),
            "{summary}"
        );
        assert!(!summary.contains("must-rebuild=0"), "{summary}");
    }

    /// EESSI 2026.06 fixtures pin urllib3 2.6.3 and 2.7.0. Both are past the
    /// 2.6.0 fix, so a parsed GIT range has no carrier. Offline, the same
    /// empty carrier list is still review because the GIT range was not parsed.
    #[test]
    fn cve_2025_66471_in_the_eessi_2026_06_fixtures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let easyconfigs = root.join("tests/sbom_recorded/rebuild/easybuild/easyconfigs");
        let stack =
            root.join("tests/sbom_recorded/rebuild/easystacks/eessi-2026.06-eb-5.4.0-2026.1.yml");
        let recorded = root.join("tests/component_reach_eessi/osv");
        let client = crate::osv::load_recorded_osv(&recorded).unwrap();
        let offline = reach_cve(&easyconfigs, &stack, &client, &crate::osv::NoGit);
        assert_eq!(offline.carriers, 0, "{}", offline.summary());
        assert_eq!(offline.review, 1, "{}", offline.summary());
        assert!(
            offline.document["unresolved"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("GIT range not parsed")),
            "{}",
            offline.summary()
        );

        let parsed = reach_cve(&easyconfigs, &stack, &client, &Urllib3Tags);
        assert_eq!(parsed.carriers, 0, "{}", parsed.summary());
        assert_eq!(parsed.review, 0, "{}", parsed.summary());
        let unresolved = parsed.document["unresolved"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        assert!(
            unresolved
                .iter()
                .all(|item| !item.as_str().unwrap_or("").contains("GIT range not parsed")),
            "{unresolved:?}"
        );
        let urllib = parsed.document["resolved"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "urllib3" && item["ecosystem"] == "PyPI")
            .expect("PyPI urllib3");
        assert!(urllib["version_reqs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|req| req == ">=1.0,<2.6.0"));
        assert!(urllib["versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|version| version == "2.5.0"));
        assert!(urllib["versions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|version| version != "2.6.3" && version != "2.7.0"));
    }

    fn reach_cve(
        easyconfigs: &Path,
        stack: &Path,
        client: &crate::registry::MapClient,
        git: &dyn crate::osv::GitRangeSource,
    ) -> ComponentReachRun {
        let out = tempfile::tempdir().unwrap();
        run_component_reach_with(
            &ComponentReachRequest {
                component: None,
                version: None,
                purl: None,
                cve: Some("CVE-2025-66471".into()),
                generation: "2026.1".into(),
                easyconfigs: vec![easyconfigs.to_path_buf()],
                installed: None,
                easystacks: vec![stack.to_path_buf()],
                easystack_urls: Vec::new(),
                compat_layer: None,
                filter_deps: Vec::new(),
                generated_at: Some("2026-11-01T00:00:00Z".into()),
                out_dir: out.path().join("out"),
                osv_cache: None,
            },
            client,
            git,
        )
        .unwrap()
    }

    struct Urllib3Tags;

    impl crate::osv::GitRangeSource for Urllib3Tags {
        fn release_tags(
            &self,
            repo: &str,
        ) -> std::result::Result<Vec<crate::osv_git::ReleaseTag>, String> {
            assert!(repo == "https://github.com/urllib3/urllib3", "{repo}");
            Ok(vec![
                crate::osv_git::ReleaseTag {
                    version: "1.0".into(),
                    commit: "04c6c18b6630386334fb010079a3ae3cdc59eea2".into(),
                },
                crate::osv_git::ReleaseTag {
                    version: "2.6.0".into(),
                    commit: "720f484b605f18887a48eef448d0084e2b76902d".into(),
                },
            ])
        }

        fn is_ancestor(
            &self,
            _repo: &str,
            ancestor: &str,
            descendant: &str,
        ) -> std::result::Result<bool, String> {
            Ok(ancestor == "c19571de34c47de3a766541b041637ba5f716ed7"
                && descendant == "720f484b605f18887a48eef448d0084e2b76902d")
        }
    }

    mod shard_fetched_lines {
        use super::super::*;
        use crate::shard_trees::tests::{archive, MapHttp};
        use serde_json::Value;
        use std::collections::BTreeMap;
        use std::path::Path;

        const SHA: &str = "0d6b80c34f3553774e93162cf89088e2423ecf89";

        /// `<root>/<letter>/<name>/<name>-<version>-GCCcore-15.2.0.eb`, its
        /// dependencies at the same toolchain. Returns the path below `root`.
        fn ec(root: &Path, name: &str, version: &str, deps: &[(&str, &str)]) -> String {
            let letter = name[..1].to_ascii_lowercase();
            let relative = format!("{letter}/{name}/{name}-{version}-GCCcore-15.2.0.eb");
            let deps: Vec<String> = deps
                .iter()
                .map(|(n, v)| format!("('{n}', '{v}')"))
                .collect();
            let text = format!(
                "easyblock = 'ConfigureMake'\nname = '{name}'\nversion = '{version}'\nhomepage = 'https://example.org/'\ndescription = 'stack shard test'\ntoolchain = {{'name': 'GCCcore', 'version': '15.2.0'}}\nsource_urls = ['https://example.org/']\nsources = [SOURCE_TAR_GZ]\ndependencies = [{}]\nmoduleclass = 'lib'\n",
                deps.join(", ")
            );
            let path = root.join(&relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
            relative
        }

        fn request(root: &Path, list: &str) -> StackShardRequest {
            std::fs::write(root.join("build.list"), list).unwrap();
            StackShardRequest {
                build_list: root.join("build.list"),
                easyconfigs: vec![root.join("robot")],
                commit_trees: Vec::new(),
                pr_trees: Vec::new(),
                commit_cache: None,
                installed: Vec::new(),
                installed_modules_dirs: Vec::new(),
                shards: 4,
                weights: None,
                default_weight: 1,
                filter_deps: vec!["NoSuchFilter".into()],
                generated_at: Some("2026-11-04T00:00:00Z".into()),
                out_dir: root.join("out"),
                lift_cpu_prerequisites: false,
            }
        }

        #[test]
        fn explicit_recipe_alias_selects_its_own_dependency_closure() {
            for alias in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path();
                let robot = root.join("robot");
                ec(&robot, "OldLib", "1.0", &[]);
                ec(&robot, "NewLib", "1.0", &[]);
                let canonical = ec(&robot, "Alpha", "1.0", &[("OldLib", "1.0")]);
                let alternate = "a/Alpha/Alpha-1.0-GCCcore-15.2.0-site.eb";
                let text = read(robot.join(&canonical)).replace("OldLib", "NewLib");
                std::fs::write(robot.join(alternate), text).unwrap();
                let (selected, required, excluded) = if alias {
                    (alternate, "NewLib", "OldLib")
                } else {
                    (canonical.as_str(), "OldLib", "NewLib")
                };
                ec(&robot, "Beta", "1.0", &[(required, "1.0")]);
                let file = Path::new(selected).file_name().unwrap().to_str().unwrap();
                let args = request(root, &format!("{file}\nBeta-1.0-GCCcore-15.2.0.eb\n"));
                let run = run_stack_shard_with(&args, &MapHttp::default()).unwrap();
                assert_eq!(run.opaque, 0, "{:?}", run.warnings);
                let plan = plan_of(root);
                let closure = line(&plan, file)["closure"].as_array().unwrap().clone();
                assert!(
                    closure.contains(&Value::String(format!("{required}/1.0-GCCcore-15.2.0"))),
                    "{plan}"
                );
                assert!(
                    !closure.contains(&Value::String(format!("{excluded}/1.0-GCCcore-15.2.0"))),
                    "{plan}"
                );
                assert_eq!(
                    read(root.join("out/prelude.list")).trim(),
                    format!("{required}-1.0-GCCcore-15.2.0.eb")
                );
                assert_no_module_twice(&plan);
            }
        }

        fn read(path: PathBuf) -> String {
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        }

        fn plan_of(root: &Path) -> Value {
            serde_json::from_str(&read(root.join("out/plan.json"))).unwrap()
        }

        fn line<'a>(plan: &'a Value, file: &str) -> &'a Value {
            plan["lines"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["easyconfig"] == file)
                .unwrap_or_else(|| panic!("{file} not in {plan}"))
        }

        /// Every shard's modules, so a test can check no module builds twice.
        fn shard_modules(plan: &Value) -> Vec<Vec<String>> {
            plan["shards"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| {
                    s["modules"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|m| m.as_str().unwrap().to_string())
                        .collect()
                })
                .collect()
        }

        fn assert_no_module_twice(plan: &Value) {
            let prelude: Vec<String> = plan["prelude"]["modules"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["module"].as_str().unwrap().to_string())
                .collect();
            let mut seen: BTreeMap<String, usize> = BTreeMap::new();
            for module in shard_modules(plan).into_iter().flatten().chain(prelude) {
                *seen.entry(module).or_default() += 1;
            }
            let twice: Vec<&String> = seen
                .iter()
                .filter(|(_, n)| **n > 1)
                .map(|(m, _)| m)
                .collect();
            assert!(twice.is_empty(), "built by two jobs: {twice:?} in {plan}");
        }

        /// A fetched line's dependency that a robot-path line also needs goes
        /// to the prelude. The commit is downloaded into the cache; a module
        /// only the commit has, needed by two fetched lines, goes to the
        /// prelude with the line's source options.
        #[test]
        fn a_fetched_line_shares_its_dependencies_through_the_prelude() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let robot = root.join("robot");
            ec(&robot, "MathLib", "1.0", &[]);
            ec(&robot, "Alpha", "1.0", &[("MathLib", "1.0")]);
            let top = format!("easybuild-easyconfigs-{SHA}");
            let in_commit = |rel: &str| format!("easybuild/easyconfigs/{rel}");
            let staging = root.join("staging");
            let files: Vec<(String, String)> = [
                ec(
                    &staging,
                    "Fetched",
                    "1.0",
                    &[("MathLib", "1.0"), ("NewLib", "2.0")],
                ),
                ec(&staging, "Other", "1.0", &[("NewLib", "2.0")]),
                ec(&staging, "NewLib", "2.0", &[]),
                ec(&staging, "MathLib", "1.0", &[]),
            ]
            .iter()
            .map(|rel| (in_commit(rel), read(staging.join(rel))))
            .collect();
            let borrowed: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, t)| (p.as_str(), t.as_str()))
                .collect();
            let mut http = MapHttp::default();
            http.pages.insert(
                format!(
                    "https://codeload.github.com/easybuilders/easybuild-easyconfigs/tar.gz/{SHA}"
                ),
                archive(&top, &borrowed),
            );
            let mut args = request(
                root,
                &format!(
                    "Alpha-1.0-GCCcore-15.2.0.eb\n\
                     Fetched-1.0-GCCcore-15.2.0.eb --from-commit={SHA} --include-systems=snel-zen2\n\
                     Other-1.0-GCCcore-15.2.0.eb --from-commit={SHA}\n"
                ),
            );
            args.commit_cache = Some(root.join("cache"));
            let run = run_stack_shard_with(&args, &http).unwrap();
            assert_eq!(run.summary(), "shards=3 prelude=2 lines=3");
            assert!(run.warnings.is_empty(), "{:?}", run.warnings);

            let plan = plan_of(root);
            let fetched = line(&plan, "Fetched-1.0-GCCcore-15.2.0.eb");
            assert!(fetched["opaque"].is_null(), "{fetched}");
            assert_eq!(fetched["source"], format!("--from-commit={SHA}"));
            assert_eq!(fetched["module"], "Fetched/1.0-GCCcore-15.2.0");
            assert_eq!(
                fetched["closure"].as_array().unwrap().len(),
                3,
                "MathLib, NewLib, Fetched: {fetched}"
            );
            let alpha = line(&plan, "Alpha-1.0-GCCcore-15.2.0.eb");
            assert_ne!(alpha["shard"], fetched["shard"]);
            assert_no_module_twice(&plan);
            // MathLib is the robot path's (the commit's copy is the same
            // file), so it builds without a source option; NewLib only the
            // commit has.
            let prelude = read(root.join("out/prelude.list"));
            let mut prelude: Vec<&str> = prelude.lines().collect();
            prelude.sort_unstable();
            assert_eq!(
                prelude,
                [
                    "MathLib-1.0-GCCcore-15.2.0.eb".to_string(),
                    format!("NewLib-2.0-GCCcore-15.2.0.eb --from-commit={SHA}")
                ]
            );
            assert_eq!(
                plan["inputs"]["trees"],
                serde_json::json!({ format!("--from-commit={SHA}"): null })
            );
            assert!(plan["inputs"]["easyconfigs"]
                .get(format!("NewLib-2.0-GCCcore-15.2.0.eb --from-commit={SHA}"))
                .is_some());
            // One download for both lines.
            assert_eq!(http.asked.borrow().len(), 1);
        }

        /// petsc4py from a commit needs PETSc, which is a robot-path line of
        /// its own: the two build in one shard, PETSc first, and no other
        /// shard builds PETSc. Hypre, which PETSc and another line need, is
        /// in the prelude.
        #[test]
        fn petsc4py_from_a_commit_builds_after_the_petsc_line_in_its_shard() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let robot = root.join("robot");
            ec(&robot, "Hypre", "2.33.0", &[]);
            ec(&robot, "PETSc", "3.25.0", &[("Hypre", "2.33.0")]);
            ec(&robot, "MUMPS", "5.8.1", &[("Hypre", "2.33.0")]);
            let tree = root.join("commit-tree");
            ec(&tree, "petsc4py", "3.25.0", &[("PETSc", "3.25.0")]);
            let mut args = request(
                root,
                &format!(
                    "PETSc-3.25.0-GCCcore-15.2.0.eb --include-systems=snel-zen2\n\
                     MUMPS-5.8.1-GCCcore-15.2.0.eb\n\
                     petsc4py-3.25.0-GCCcore-15.2.0.eb --from-commit={SHA} --include-systems=snel-zen2\n"
                ),
            );
            args.commit_trees = vec![format!("{}={}", &SHA[..10], tree.display())];
            let run = run_stack_shard_with(&args, &MapHttp::default()).unwrap();
            assert_eq!(run.opaque, 0, "{:?}", run.warnings);

            let plan = plan_of(root);
            let petsc = line(&plan, "PETSc-3.25.0-GCCcore-15.2.0.eb");
            let petsc4py = line(&plan, "petsc4py-3.25.0-GCCcore-15.2.0.eb");
            assert!(petsc4py["opaque"].is_null(), "{petsc4py}");
            assert!(petsc4py["closure"]
                .as_array()
                .unwrap()
                .contains(&Value::from("PETSc/3.25.0-GCCcore-15.2.0")));
            assert_eq!(petsc["group"], petsc4py["group"]);
            assert_eq!(petsc["shard"], petsc4py["shard"]);
            let shard_file = format!("out/{}.list", petsc["shard"].as_str().unwrap());
            assert_eq!(
                read(root.join(shard_file)),
                format!(
                    "PETSc-3.25.0-GCCcore-15.2.0.eb --include-systems=snel-zen2\n\
                     petsc4py-3.25.0-GCCcore-15.2.0.eb --from-commit={SHA} --include-systems=snel-zen2\n"
                )
            );
            let with_petsc = shard_modules(&plan)
                .iter()
                .filter(|m| m.contains(&"PETSc/3.25.0-GCCcore-15.2.0".to_string()))
                .count();
            assert_eq!(with_petsc, 1);
            assert_eq!(
                read(root.join("out/prelude.list")),
                "Hypre-2.33.0-GCCcore-15.2.0.eb\n"
            );
            assert_no_module_twice(&plan);
        }

        /// A fetched line that cannot be resolved stays opaque, with the
        /// reason in the plan and in the warnings: its easyconfig is in
        /// neither tree, its tree needs something nothing provides, or its
        /// commit cannot be downloaded.
        #[test]
        fn an_unresolvable_fetched_line_stays_opaque_and_is_reported() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let robot = root.join("robot");
            ec(&robot, "MathLib", "1.0", &[]);
            ec(&robot, "Alpha", "1.0", &[("MathLib", "1.0")]);
            let tree = root.join("commit-tree");
            ec(&tree, "Needy", "1.0", &[("Nowhere", "9.9")]);
            let missing = "1111111111111111111111111111111111111111";
            let mut args = request(
                root,
                &format!(
                    "Alpha-1.0-GCCcore-15.2.0.eb\n\
                     Absent-1.0-GCCcore-15.2.0.eb --from-commit={SHA}\n\
                     Needy-1.0-GCCcore-15.2.0.eb --from-commit={SHA}\n\
                     Gone-1.0-GCCcore-15.2.0.eb --from-commit={missing}\n"
                ),
            );
            args.commit_trees = vec![format!("{SHA}={}", tree.display())];
            args.commit_cache = Some(root.join("cache"));
            let run = run_stack_shard_with(&args, &MapHttp::default()).unwrap();
            assert_eq!(run.opaque, 3);
            assert!(run.summary().ends_with(" opaque=3"), "{}", run.summary());

            let plan = plan_of(root);
            let absent = line(&plan, "Absent-1.0-GCCcore-15.2.0.eb");
            assert_eq!(
                absent["opaque"],
                "fetched by EasyBuild (--from-pr or --from-commit)"
            );
            assert!(
                absent["opaque_reason"]
                    .as_str()
                    .unwrap()
                    .contains("is in neither the tree"),
                "{absent}"
            );
            assert_eq!(
                absent["closure"],
                serde_json::json!(["Absent-1.0-GCCcore-15.2.0.eb"])
            );
            let needy = line(&plan, "Needy-1.0-GCCcore-15.2.0.eb");
            assert!(
                needy["opaque_reason"]
                    .as_str()
                    .unwrap()
                    .contains("needs Nowhere"),
                "{needy}"
            );
            let gone = line(&plan, "Gone-1.0-GCCcore-15.2.0.eb");
            assert!(
                gone["opaque_reason"].as_str().unwrap().contains("HTTP 404"),
                "{gone}"
            );
            assert!(plan["inputs"]["trees"][format!("--from-commit={missing}")]
                .as_str()
                .unwrap()
                .contains("HTTP 404"));
            assert_eq!(plan["opaque"].as_array().unwrap().len(), 3);
            for file in ["Absent", "Needy", "Gone"] {
                assert!(
                    run.warnings
                        .iter()
                        .any(|w| w.contains(&format!("{file}-1.0-GCCcore-15.2.0.eb: opaque"))),
                    "{file}: {:?}",
                    run.warnings
                );
            }
            // Each opaque line is still written to a shard, unchanged.
            let all: String = (1..=run.shards)
                .map(|s| read(root.join(format!("out/shard-{s:02}.list"))))
                .collect();
            assert!(all.contains(&format!(
                "Gone-1.0-GCCcore-15.2.0.eb --from-commit={missing}\n"
            )));
            assert!(!root.join("cache").join(missing).exists());
        }

        /// texlive on the robot path needs libGLU, which only a fetched
        /// line's commit provides. The libGLU line builds it first in the
        /// same shard, so texlive resolves and the two lines are one group.
        #[test]
        fn a_robot_line_needing_a_fetched_line_shares_its_shard() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let robot = root.join("robot");
            ec(&robot, "Mesa", "26.1.1", &[]);
            ec(&robot, "texlive", "20250308", &[("libGLU", "9.0.3")]);
            ec(&robot, "Other", "1.0", &[("Mesa", "26.1.1")]);
            let tree = root.join("commit-tree");
            ec(&tree, "libGLU", "9.0.3", &[("Mesa", "26.1.1")]);
            let mut args = request(
                root,
                &format!(
                    "libGLU-9.0.3-GCCcore-15.2.0.eb --from-commit={SHA}\n\
                     texlive-20250308-GCCcore-15.2.0.eb\n\
                     Other-1.0-GCCcore-15.2.0.eb\n"
                ),
            );
            args.commit_trees = vec![format!("{SHA}={}", tree.display())];
            let run = run_stack_shard_with(&args, &MapHttp::default()).unwrap();
            assert_eq!(run.opaque, 0, "{:?}", run.warnings);
            let plan = plan_of(root);
            let glu = line(&plan, "libGLU-9.0.3-GCCcore-15.2.0.eb");
            let tex = line(&plan, "texlive-20250308-GCCcore-15.2.0.eb");
            assert_eq!(glu["group"], tex["group"]);
            let shard_file = format!("out/{}.list", tex["shard"].as_str().unwrap());
            assert_eq!(
                read(root.join(shard_file)),
                format!(
                    "libGLU-9.0.3-GCCcore-15.2.0.eb --from-commit={SHA}\n\
                     texlive-20250308-GCCcore-15.2.0.eb\n"
                )
            );
            assert_eq!(
                read(root.join("out/prelude.list")),
                "Mesa-26.1.1-GCCcore-15.2.0.eb\n"
            );
            assert_no_module_twice(&plan);
        }

        /// A connect error is not an HTTP status. The plan is not written.
        struct OfflineHttp;

        impl crate::pr_overlap::HttpGet for OfflineHttp {
            fn get(&self, _url: &str) -> Result<crate::pr_overlap::HttpResponse, String> {
                Err("tcp connect error: Network is unreachable".into())
            }
        }

        #[test]
        fn a_cache_miss_with_no_network_fails_and_writes_nothing() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            ec(&root.join("robot"), "MathLib", "1.0", &[]);
            let mut args = request(
                root,
                &format!("Absent-1.0-GCCcore-15.2.0.eb --from-commit={SHA}\n"),
            );
            args.commit_cache = Some(root.join("cache"));
            let error = run_stack_shard_with(&args, &OfflineHttp).unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.starts_with(crate::shard_trees::CACHE_DOWNLOAD_FAILED),
                "{message}"
            );
            assert!(message.contains("Network is unreachable"), "{message}");
            assert!(message.contains("nothing written"), "{message}");
            assert!(!root.join("out").exists(), "nothing is written");
            assert!(!root.join("cache").join(SHA).exists());
        }

        #[test]
        fn a_bad_tree_spec_is_a_usage_error() {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            ec(&root.join("robot"), "MathLib", "1.0", &[]);
            let mut args = request(root, "MathLib-1.0-GCCcore-15.2.0.eb\n");
            args.commit_trees = vec!["main=/tmp/x".into()];
            let error = run_stack_shard_with(&args, &MapHttp::default()).unwrap_err();
            assert!(
                error.to_string().contains("--commit-tree wants COMMIT=DIR"),
                "{error}"
            );
            args.commit_trees.clear();
            args.pr_trees = vec!["x=/tmp/x".into()];
            let error = run_stack_shard_with(&args, &MapHttp::default()).unwrap_err();
            assert!(
                error.to_string().contains("--pr-tree wants N=DIR"),
                "{error}"
            );
            assert!(!root.join("out").exists());
        }
    }
}
