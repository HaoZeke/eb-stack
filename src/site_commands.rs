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

/// A rejected command line. The CLI exits 2 when this is the cause.
#[derive(Debug)]
pub struct CommandUsage {
    message: String,
}

impl std::fmt::Display for CommandUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandUsage {}

fn usage(message: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(CommandUsage {
        message: message.to_string(),
    })
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
    /// `<commit>=<dir>` pins.
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
    /// `<commit>=<dir>` easyblock overlays.
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
    /// Deployed easystacks.
    pub easystacks: Vec<PathBuf>,
    /// Easyconfig trees, later ones taking precedence.
    pub easyconfigs: Vec<PathBuf>,
    /// `<commit>=<dir>` pins.
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
    /// `<commit>=<dir>` easyblock overlays.
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
    /// Directory the report was written to.
    pub out_dir: PathBuf,
}

impl RebuildSetRun {
    /// The line `eb-stack rebuild-set` prints.
    pub fn summary(&self) -> String {
        format!(
            "changed={} rows={} must-rebuild={} review={} no-rebuild={} missing={} easystacks={} skipped_easyconfigs={} out={}",
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
            "component={} version={} generation={} carriers={} changed={} must-rebuild={} review={} no-rebuild={} out={}",
            self.component,
            self.version,
            self.generation,
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
pub fn run_pr_overlap(args: &PrOverlapRequest) -> Result<PrOverlapRun, PrOverlapError> {
    if args.kind == OverlapKind::SoftwareLayer && args.installed.is_none() {
        return Err(PrOverlapError::InstalledRequired);
    }
    run_pr_overlap_inner(args).map_err(PrOverlapError::Failed)
}

fn run_pr_overlap_inner(args: &PrOverlapRequest) -> Result<PrOverlapRun> {
    use pr_overlap::{record_to_toml, render_markdown, to_toml, PrInput};
    let mut prs: Vec<PrInput> = Vec::new();
    for path in &args.prs {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--pr {}: not readable", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("--pr {}: not JSON", path.display()))?;
        let inputs: Vec<PrInput> = if value.is_array() {
            serde_json::from_value(value)
        } else {
            serde_json::from_value(value).map(|one| vec![one])
        }
        .with_context(|| format!("--pr {}: pull request record", path.display()))?;
        prs.extend(inputs);
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
    let config = OverlapConfig {
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
        let installed = installed
            .as_ref()
            .expect("software-layer checks --installed");
        prs.iter()
            .filter_map(|pr| pr_record(pr, &robot, installed, &config))
            .collect()
    };
    let index = overlap_index(&records, &config);
    let repo_url = args.repo_url.clone().unwrap_or_else(|| {
        if easyconfigs {
            "https://github.com/easybuilders/easybuild-easyconfigs".to_string()
        } else {
            "https://github.com/EESSI/software-layer".to_string()
        }
    });

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
    std::fs::write(
        args.out_dir.join("overlap.md"),
        render_markdown(&records, &index, &repo_url),
    )?;
    if args.merge_order || !args.merged.is_empty() {
        if let Some(number) = merge_order::unknown_merged(
            &prs.iter().map(|pr| pr.number).collect::<Vec<_>>(),
            &args.merged,
        ) {
            return Err(usage(format!(
                "--merged {number}: no pull request with that number"
            )));
        }
        let mut order = merge_order::plan(&records, &index, &args.merged);
        order.conflicts = merge_order::conflicts_from_records(&records);
        std::fs::write(args.out_dir.join("merge-order.toml"), to_toml(&order)?)?;
        let mut markdown = std::fs::read_to_string(args.out_dir.join("overlap.md"))?;
        markdown.push_str(&merge_order::render_section(&order, &repo_url));
        std::fs::write(args.out_dir.join("overlap.md"), markdown)?;
    }
    if !args.impact.is_empty() {
        for number in &args.impact {
            if args.merged.contains(number) {
                return Err(usage(format!(
                    "--impact {number}: already listed in --merged"
                )));
            }
        }
        let report =
            merge_order::impact_report(&records, &args.impact, &config.generated_at, &args.merged)
                .map_err(|number| {
                    usage(format!(
                        "--impact {number}: no pull request with that number"
                    ))
                })?;
        std::fs::write(args.out_dir.join("merge-impact.toml"), to_toml(&report)?)?;
    }
    Ok(PrOverlapRun {
        prs: prs.len(),
        records: records.len(),
        shared_modules: index.shared.len(),
        skipped_easyconfigs,
        out_dir: args.out_dir.clone(),
    })
}

/// Write the rebuild-set report.
pub fn run_rebuild_set(args: &RebuildSetRequest) -> Result<RebuildSetRun> {
    let changed = read_changed_names(&args.changed)?;
    let (robot, skipped_easyconfigs) = load_robot_trees(&args.easyconfigs, &args.commit_trees)?;
    let installed = load_installed_modules(&args.installed, &args.installed_modulerc)?;
    let filter_specs = dep_filter::specs_from_cli_or_env(&args.filter_deps);
    let filter_deps = FilterDeps::parse(&filter_specs)
        .map_err(|error| usage(format!("--filter-deps: {error}")))?;
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
    let document = rebuild_set::rebuild_set(&rebuild_set::RebuildQuery {
        base: robot.base,
        commits: robot.commits,
        easystacks,
        changed,
        installed,
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
    Ok(RebuildSetRun {
        changed: document.changed.len(),
        rows: document.rows.len(),
        must_rebuild: document.counts.must_rebuild,
        review: document.counts.review,
        no_rebuild: document.counts.no_rebuild,
        missing: document.missing.len(),
        easystacks: stacks.len(),
        skipped_easyconfigs,
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
    if let Some(dir) = &args.osv_cache {
        let client = crate::osv::load_recorded_osv(dir)
            .map_err(|error| anyhow!("--osv-cache {}: {error}", dir.display()))?;
        return run_component_reach_with(args, &client);
    }
    run_component_reach_with(args, &crate::registry::UreqClient)
}

fn run_component_reach_with(
    args: &ComponentReachRequest,
    client: &dyn crate::registry::RegistryClient,
) -> Result<ComponentReachRun> {
    use crate::component_reach::{
        component_reach, cve_reach, cve_to_toml, fetch_easystack_urls, render_cve_markdown,
        render_markdown, to_toml, ReachQuery, ReachScope,
    };
    use crate::osv::{parse_purl, resolve_vulnerability};

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
    std::fs::create_dir_all(&args.out_dir)?;
    if let Some(cve) = &args.cve {
        let resolution = resolve_vulnerability(cve, client).map_err(|error| anyhow!(error))?;
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
}
