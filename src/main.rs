//! Version-one command surface for canonical package planning and build campaigns.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use eb_stack::campaign::{
    claim_finding, resolve_finding, run_campaign as execute_campaign, CampaignRequest,
    CampaignStatus, FindingResolution,
};
use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::package_sources::{PackageSourceRoots, SourceRootKind};
use eb_stack::target::{doctor_target, resolve_target_layers, BuildTarget, TargetConfigLayer};
use eb_stack::{
    check_duplicate_upstream, check_maintainer_acceptability, check_maintainer_acceptability_text,
    check_recipe_deps, format_style, format_style_file, inspect_new_package, is_registry_name,
    lint_style, load_json_file, materialize_registry_name, packaging_gate, parse_easyconfig_trees,
    plan_new_package, plan_package_bump, plan_package_closure_with_sources,
    resolve_easyconfig_file, resolve_package_catalog_layers,
    solve_from_easyconfigs_with_baseline_version_and_extras, write_json_pretty,
    write_package_bundle, write_package_closure, BumpPackageRequest, ForeignFormat,
    NewPackageRequest, PackageBundle, PackageCatalogLayer, SolveExtraOut, StackLock, Toolchain,
    UreqClient,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "eb-stack",
    version,
    about = "Canonical SBOM, build-manifest, Resolvo, EasyBuild, and campaign workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Parse, solve, and emit package artifacts.
    Package {
        #[command(subcommand)]
        command: PackageCommand,
    },
    /// Check and format EasyBuild recipes.
    Recipe {
        #[command(subcommand)]
        command: RecipeCommand,
    },
    /// Solve EasyBuild stack locks and SBOMs.
    Stack {
        #[command(subcommand)]
        command: StackCommand,
    },
    /// Inspect declarative build targets.
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// Run and inspect persisted build campaigns.
    Campaign {
        #[command(subcommand)]
        command: CampaignCommand,
    },
    /// Read the reproduction grind's score artifacts.
    Repro {
        #[command(subcommand)]
        command: ReproCommand,
    },
    /// Serve the same workflows over MCP stdio.
    Mcp,
}

#[derive(Subcommand, Debug)]
enum ReproCommand {
    /// Summarize a collected reproduction run for the scoreboard.
    ///
    /// Reads the per-case JSON a suite run wrote under `EB_REPRO_SCORES`
    /// and prints the scoreboard's own table shape. With `--ratchet`, it
    /// also holds the run to the committed allowance counts and exits
    /// nonzero on any disagreement, which is the same gate the suite
    /// applies per case.
    Summary {
        /// Directory of per-case score artifacts.
        #[arg(long)]
        scores: PathBuf,
        /// The committed allowance counts to check the run against.
        #[arg(long)]
        ratchet: Option<PathBuf>,
        /// Print the collected scores as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PackageCommand {
    /// Parse a foreign recipe into a canonical build manifest and planned SBOM.
    Inspect(PackageInspectArgs),
    /// Resolve every declared profile and emit a canonical artifact bundle.
    Plan(PackagePlanArgs),
    /// Retarget an existing EasyBuild recipe using hierarchy + Resolvo selection.
    Bump(PackageBumpArgs),
}

#[derive(clap::Args, Debug)]
struct PackageInspectArgs {
    #[arg(long)]
    source: PathBuf,
    #[arg(long, default_value = "auto")]
    format: String,
    #[arg(long, default_value = "foss")]
    toolchain_name: String,
    #[arg(long)]
    toolchain_version: String,
    #[arg(long = "package-config")]
    package_configs: Vec<PathBuf>,
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(clap::Args, Debug)]
struct PackagePlanArgs {
    #[command(flatten)]
    inspect: PackageInspectArgs,
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    #[arg(long)]
    stack_policy: PathBuf,
    /// Positional SHA-256 override; repeat once for every source artifact.
    #[arg(long = "source-checksum", value_name = "SHA256")]
    source_checksums: Vec<String>,
    /// Repository index giving versions for dependencies that state none.
    ///
    /// The format is the one CRAN publishes as `PACKAGES`: stanzas of
    /// `Field: value` separated by blank lines. A bare dependency name is
    /// normal in CRAN and on PyPI, and an `exts_list` entry still needs one
    /// concrete version.
    #[arg(long = "package-index", value_name = "FILE")]
    package_index: Option<PathBuf>,
    /// Optional package-source catalog layers for recursive robot-hole closure.
    ///
    /// Explicit catalog entries are ordered overrides. Argument order is layer
    /// order. Closure also activates when `--package-sources` or per-kind
    /// source roots are configured.
    #[arg(long = "package-catalog", value_name = "CATALOG.toml")]
    package_catalogs: Vec<PathBuf>,
    /// Optional package-neutral source-root TOML layers (EasyBuild / conda-forge / Spack / Cargo).
    #[arg(long = "package-sources", value_name = "SOURCES.toml")]
    package_sources: Vec<PathBuf>,
    /// Ordered EasyBuild easyconfig trees used to discover cross-generation recipes.
    #[arg(long = "easybuild-source", value_name = "DIR")]
    easybuild_sources: Vec<PathBuf>,
    /// Ordered conda-forge recipe or feedstock trees for foreign discovery.
    #[arg(long = "conda-source", value_name = "DIR")]
    conda_sources: Vec<PathBuf>,
    /// Ordered Spack package trees for foreign discovery.
    #[arg(long = "spack-source", value_name = "DIR")]
    spack_sources: Vec<PathBuf>,
    /// Ordered Cargo.toml / crates.io trees for PyO3 leftover discovery.
    #[arg(long = "cargo-source", value_name = "DIR")]
    cargo_sources: Vec<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct PackageBumpArgs {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    toolchain_name: String,
    #[arg(long)]
    toolchain_version: String,
    #[arg(long)]
    version: Option<String>,
    #[arg(long)]
    source_checksum: Option<String>,
    #[arg(long = "dep", value_name = "NAME=VERSION")]
    dependencies: Vec<String>,
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    #[arg(long)]
    hierarchy_fixture: Option<PathBuf>,
    #[arg(long)]
    stack_policy: Option<PathBuf>,
    /// Fail when a patch's fate across the version bump cannot be decided
    /// from tree evidence, instead of carrying it with a review flag.
    #[arg(long)]
    strict_patches: bool,
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(Subcommand, Debug)]
enum RecipeCommand {
    /// Resolve a recipe and verify package metadata plus robot dependencies.
    Check {
        #[arg(long)]
        recipe: PathBuf,
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        #[arg(long = "require-configopt")]
        require_configopts: Vec<String>,
        #[arg(long)]
        metadata_only: bool,
        /// Classify the source URLs and verify a seeded checksum came from the
        /// same artifact class. Fails the check on a class conflict.
        #[arg(long)]
        verify_sources: bool,
        /// Ecosystem a checksum was copied from, e.g. conda-forge or spack.
        #[arg(long, requires = "verify_sources")]
        seeded_from: Option<String>,
        /// The source URL that seeded checksum was computed over.
        #[arg(long, requires = "seeded_from")]
        seeded_source_url: Option<String>,
        /// An unpacked source tree for the commit the recipe pins. The version
        /// its build system declares is compared against the recipe version,
        /// which is the one question a checksum cannot answer.
        #[arg(long, requires = "verify_sources")]
        source_tree: Option<PathBuf>,
        /// A git remote, when the seeding recipe built from a checkout.
        #[arg(long, requires = "seeded_from")]
        seeded_git: Option<String>,
        /// Report every statement the parser could not model, with its line,
        /// and fail if there are any. The parse itself is unchanged: this
        /// surfaces what tolerant mode silently skips.
        #[arg(long)]
        strict: bool,
    },
    /// Report EasyBuild E501 style findings.
    Lint {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// Mechanically format EasyBuild E501 findings.
    Format {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum StackCommand {
    /// Parse EasyBuild trees and solve a jointly consistent stack.
    Solve {
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        baseline_easyconfigs: Option<PathBuf>,
        #[arg(long)]
        baseline_toolchain_version: Option<String>,
        #[arg(long, default_value = "stack.lock.json")]
        lock_out: PathBuf,
        #[arg(long)]
        sbom_out: Option<PathBuf>,
        #[arg(long)]
        build_list_out: Option<PathBuf>,
        #[arg(long)]
        stack_diff_out: Option<PathBuf>,
    },
    /// Emit CycloneDX from an existing stack lock.
    Sbom {
        #[arg(long)]
        lock: PathBuf,
        #[arg(long, default_value = "stack.cdx.json")]
        out: PathBuf,
    },
    /// Order the builds a set of roots needs, as a graph rather than a stack.
    ///
    /// Unlike `solve`, this does not pick one version per package: it takes
    /// what the recipes pin and sequences them, so a generation that carries
    /// two builds of one name gets both, in an order that respects every
    /// dependency.
    Order {
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        /// Package to build, optionally pinned as `name==version`. Repeatable.
        #[arg(long = "root", required = true)]
        roots: Vec<String>,
        #[arg(long, default_value = "build-order.txt")]
        out: PathBuf,
        /// Take the oldest admissible version of an unpinned requirement
        /// instead of the newest, for reproducing what an older tree built.
        #[arg(long)]
        oldest: bool,
        /// Also write the order as an easystack for `eb --easystack`.
        #[arg(long)]
        easystack_out: Option<PathBuf>,
        /// Write one input hash per module: a digest over the easyconfig's own
        /// bytes and the hashes of everything it needs. Two plans with the same
        /// hashes are the same plan, and a changed hash is a rebuild.
        #[arg(long)]
        hashes_out: Option<PathBuf>,
    },
    /// Write the lock as an EasyBuild easystack, the format `eb --easystack`
    /// consumes and EESSI's software layer is built from.
    Easystack {
        #[arg(long)]
        lock: PathBuf,
        #[arg(long, default_value = "stack.yml")]
        out: PathBuf,
        /// Per-easyconfig `eb` option, as `<file.eb>:<option>=<value>`, with
        /// the option spelled as on the command line without its dashes:
        /// `--option CUDA-12.8.0.eb:accept-eula-for=CUDA`. Repeatable.
        #[arg(long = "option")]
        options: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
enum TargetCommand {
    /// List targets from layered public TOML configuration.
    List {
        #[arg(long = "config", required = true)]
        configs: Vec<PathBuf>,
    },
    /// Validate transport, executor, runtime, and EasyBuild workload routing.
    Doctor {
        #[arg(long = "config", required = true)]
        configs: Vec<PathBuf>,
        #[arg(long)]
        target: String,
    },
}

#[derive(Subcommand, Debug)]
enum CampaignCommand {
    /// Start or resume a persisted package build campaign.
    Run {
        #[arg(long)]
        bundle: PathBuf,
        #[arg(long = "config", required = true)]
        configs: Vec<PathBuf>,
        #[arg(long)]
        target: String,
        #[arg(long)]
        state: PathBuf,
    },
    /// Print persisted campaign state and claim ladder.
    Status {
        #[arg(long)]
        state: PathBuf,
    },
    /// Coordinate typed finding repair across campaign workers.
    Finding {
        #[command(subcommand)]
        command: CampaignFindingCommand,
    },
}

#[derive(Subcommand, Debug)]
enum CampaignFindingCommand {
    /// Claim an open finding for one worker.
    Claim {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        owner: String,
    },
    /// Resolve a claimed finding with durable evidence.
    Resolve {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        owner: String,
        #[arg(long)]
        action: String,
        #[arg(long)]
        evidence: String,
        #[arg(long = "change")]
        changes: Vec<String>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Package { command } => run_package(command),
        Command::Recipe { command } => run_recipe(command),
        Command::Stack { command } => run_stack(command),
        Command::Target { command } => run_target(command),
        Command::Campaign { command } => run_campaign(command),
        Command::Repro { command } => run_repro(command),
        Command::Mcp => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            eb_stack::mcp::run_server(stdin.lock(), stdout.lock()).context("MCP stdio server")?;
            Ok(())
        }
    }
}

fn run_package(command: PackageCommand) -> Result<()> {
    match command {
        PackageCommand::Inspect(args) => {
            let toolchain = toolchain(&args.toolchain_name, &args.toolchain_version);
            let layers = load_package_layers(&args.package_configs)?;
            let format = parse_format(&args.format)?;
            let source = resolve_ingest_source(&args.source, format, &args.out_dir)?;
            let (plan, sbom) = inspect_new_package(&source, format, &toolchain, &layers)?;
            let written = write_package_bundle(
                &PackageBundle {
                    plan,
                    sbom,
                    locks: Vec::new(),
                    easyconfigs: Vec::new(),
                },
                &args.out_dir,
            )?;
            println!("manifest={}", written.manifest.display());
            println!("sbom={}", written.sbom.display());
            Ok(())
        }
        PackageCommand::Plan(args) => {
            let toolchain = toolchain(
                &args.inspect.toolchain_name,
                &args.inspect.toolchain_version,
            );
            let stack_policy = load_stack_policy(&args.stack_policy)?;
            let source_roots = load_package_source_roots(&args)?;
            let use_closure =
                !args.package_catalogs.is_empty() || !source_roots.source_roots.is_empty();
            let format = parse_format(&args.inspect.format)?;
            let source =
                resolve_ingest_source(&args.inspect.source, format, &args.inspect.out_dir)?;
            let request = NewPackageRequest {
                source,
                format,
                toolchain,
                source_checksums: args.source_checksums,
                package_layers: load_package_layers(&args.inspect.package_configs)?,
                package_index: load_package_index(args.package_index.as_deref())?,
                easyconfig_roots: args.easyconfigs,
                stack_policy,
            };
            if !use_closure {
                let bundle = plan_new_package(&request)?;
                let written = write_package_bundle(&bundle, &args.inspect.out_dir)?;
                println!("manifest={}", written.manifest.display());
                println!("sbom={}", written.sbom.display());
                for path in written.locks {
                    println!("lock={}", path.display());
                }
                for path in written.easyconfigs {
                    println!("easyconfig={}", path.display());
                }
                for path in written.patches {
                    println!("patch={}", path.display());
                }
                return Ok(());
            }

            let mut layers = Vec::with_capacity(args.package_catalogs.len());
            for path in &args.package_catalogs {
                layers.push(
                    PackageCatalogLayer::from_path(path)
                        .with_context(|| format!("load package catalog {}", path.display()))?,
                );
            }
            let catalog = resolve_package_catalog_layers(&layers)
                .context("resolve package-source catalog layers")?;
            let closure = plan_package_closure_with_sources(&request, &catalog, &source_roots)?;
            let written = write_package_closure(&closure, &args.inspect.out_dir)?;
            println!("closure_plan={}", written.closure_plan.display());
            println!("closure_sbom={}", written.closure_sbom.display());
            println!("build_order={}", written.build_order.display());
            println!("manifest={}", written.root.manifest.display());
            println!("sbom={}", written.root.sbom.display());
            for path in &written.root.locks {
                println!("lock={}", path.display());
            }
            for path in &written.root.easyconfigs {
                println!("easyconfig={}", path.display());
            }
            for path in &written.root.patches {
                println!("patch={}", path.display());
            }
            for companion in &written.companions {
                println!("companion_manifest={}", companion.manifest.display());
                println!("companion_sbom={}", companion.sbom.display());
                for path in &companion.locks {
                    println!("companion_lock={}", path.display());
                }
                for path in &companion.easyconfigs {
                    println!("easyconfig={}", path.display());
                }
                for path in &companion.patches {
                    println!("companion_patch={}", path.display());
                }
            }
            Ok(())
        }
        PackageCommand::Bump(args) => run_package_bump(args),
    }
}

fn run_package_bump(args: PackageBumpArgs) -> Result<()> {
    let toolchain = toolchain(&args.toolchain_name, &args.toolchain_version);
    let stack_policy = if let Some(path) = args.stack_policy.as_deref() {
        load_stack_policy(path)?
    } else {
        unconstrained_stack_policy(&toolchain)
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source: args.source,
        toolchain,
        version: args.version,
        source_checksum: args.source_checksum,
        easyconfig_roots: args.easyconfigs,
        hierarchy_fixture: args.hierarchy_fixture,
        overrides: parse_dep_overrides(&args.dependencies)?,
        stack_policy,
        strict_patches: args.strict_patches,
    })?;
    let written = write_package_bundle(&bundle, &args.out_dir)?;
    println!("manifest={}", written.manifest.display());
    println!("sbom={}", written.sbom.display());
    for path in written.locks {
        println!("lock={}", path.display());
    }
    for path in written.easyconfigs {
        println!("easyconfig={}", path.display());
    }
    Ok(())
}

fn run_repro(command: ReproCommand) -> Result<()> {
    match command {
        ReproCommand::Summary {
            scores,
            ratchet,
            json,
        } => {
            let collected = eb_stack::read_case_scores(&scores)
                .with_context(|| format!("read repro scores from {}", scores.display()))?;
            if collected.is_empty() {
                anyhow::bail!(
                    "no score artifacts in {}; run the reproduction suite with EB_REPRO_SCORES set to that directory",
                    scores.display()
                );
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&collected)?);
            } else {
                print!("{}", eb_stack::render_scoreboard_table(&collected));
            }
            let Some(ratchet_path) = ratchet else {
                return Ok(());
            };
            let ratchet = eb_stack::ReproRatchet::read(&ratchet_path)
                .with_context(|| format!("read ratchet {}", ratchet_path.display()))?;
            let violations = eb_stack::check_ratchet(&ratchet, &collected);
            if violations.is_empty() {
                println!(
                    "ratchet: {} cases hold at {}",
                    collected.len(),
                    ratchet.total
                );
                return Ok(());
            }
            for violation in &violations {
                eprintln!("ratchet: {violation}");
            }
            anyhow::bail!("{} ratchet violation(s)", violations.len())
        }
    }
}

fn run_recipe(command: RecipeCommand) -> Result<()> {
    match command {
        RecipeCommand::Check {
            recipe,
            easyconfigs,
            require_configopts,
            metadata_only,
            verify_sources: check_sources,
            seeded_from,
            seeded_source_url,
            seeded_git,
            source_tree,
            strict,
        } => {
            let resolved = if strict {
                let (resolved, skipped) = eb_stack::resolve_easyconfig_file_reporting(&recipe)
                    .map_err(anyhow::Error::msg)?;
                for statement in &skipped {
                    println!("skipped {statement}");
                }
                if !skipped.is_empty() {
                    bail!(
                        "strict: {} statement(s) could not be parsed; \
                         the recipe still resolves without them",
                        skipped.len()
                    );
                }
                println!("strict: every statement parsed");
                resolved
            } else {
                resolve_easyconfig_file(&recipe).map_err(anyhow::Error::msg)?
            };
            if check_sources {
                let seed = seeded_from.map(|origin| eb_stack::SeededChecksum {
                    origin,
                    source_url: seeded_source_url,
                    git: seeded_git,
                    sha256: resolved.checksums.first().cloned(),
                });
                let mut findings = eb_stack::verify_sources(&resolved.source_urls, seed.as_ref());
                if let Some(tree) = source_tree.as_deref() {
                    let declared = eb_stack::declared_version(tree);
                    if let Some(found) = declared.as_ref() {
                        println!("declared_version={} from={}", found.value, found.source);
                    }
                    findings.extend(eb_stack::verify_declared_version(
                        &resolved.version,
                        declared.as_ref(),
                    ));
                }
                for url in &resolved.source_urls {
                    println!("source_class={} url={url}", eb_stack::classify_url(url));
                }
                for finding in &findings {
                    println!("{finding}");
                }
                let conflicts: Vec<&eb_stack::SourceFinding> = findings
                    .iter()
                    .filter(|f| f.level == eb_stack::FindingLevel::Error)
                    .collect();
                if !conflicts.is_empty() {
                    bail!(
                        "source verification failed: {}",
                        conflicts
                            .iter()
                            .map(|f| f.message.as_str())
                            .collect::<Vec<_>>()
                            .join("; ")
                    );
                }
            }
            let source_text = std::fs::read_to_string(&recipe)
                .with_context(|| format!("read {}", recipe.display()))?;
            let required = require_configopts
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let gate = packaging_gate(&resolved, &required);
            let maintainer = check_maintainer_acceptability(&resolved, &source_text);
            println!(
                "maintainer_acceptability={}",
                serde_json::to_string_pretty(&maintainer)?
            );
            if !maintainer.ok_for_upstream() {
                let errors: Vec<_> = maintainer
                    .findings
                    .iter()
                    .filter(|f| f.is_error())
                    .map(|f| format!("{}: {}", f.code, f.message))
                    .collect();
                bail!(
                    "maintainer-acceptability failed (easybuild-easyconfigs #26435 class): {}",
                    errors.join("; ")
                );
            }
            if metadata_only {
                if let Err(errors) = gate {
                    bail!("packaging gate failed: {}", errors.join("; "));
                }
                println!("recipe metadata resolves");
                return Ok(());
            }
            let roots = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let tree = parse_easyconfig_trees(&roots).map_err(anyhow::Error::msg)?;
            // Needs the robot tree, so it runs here rather than with the
            // text-only maintainer gates above.
            let duplicates = check_duplicate_upstream(&resolved, &tree.candidates);
            if !duplicates.is_empty() {
                println!(
                    "duplicate_upstream={}",
                    serde_json::to_string_pretty(&duplicates)?
                );
            }
            let check = check_recipe_deps(&resolved, &tree.candidates);
            println!("{}", serde_json::to_string_pretty(&check)?);
            if let Some(dup) = duplicates.iter().find(|f| f.is_error()) {
                bail!("{}: {}", dup.code, dup.message);
            }
            if let Err(errors) = gate {
                bail!("packaging gate failed: {}", errors.join("; "));
            }
            if !check.ok() {
                bail!("recipe has {} unresolved dependencies", check.missing.len());
            }
            // A clean `missing` list earns the word "resolves" only when the
            // matches were filtered by a real toolchain hierarchy. Without one
            // they are name-and-version matches that can span generations, so
            // say that instead of claiming a resolve the evidence cannot carry.
            match &check.unverified_toolchain {
                Some(note) => println!("recipe dependencies found, toolchain unverified: {note}"),
                None => println!("recipe resolves"),
            }
            Ok(())
        }
        RecipeCommand::Lint { paths } => {
            let mut findings = Vec::new();
            let mut maintainer_all = Vec::new();
            for path in paths {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("read {}", path.display()))?;
                findings.extend(lint_style(&text));
                let report = check_maintainer_acceptability_text(&text);
                for f in report.findings {
                    maintainer_all.push(serde_json::json!({
                        "path": path.display().to_string(),
                        "code": f.code,
                        "severity": f.severity,
                        "message": f.message,
                        "evidence": f.evidence,
                    }));
                }
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "style": findings,
                    "maintainer_acceptability": maintainer_all,
                }))?
            );
            let maintainer_errors = maintainer_all
                .iter()
                .filter(|v| v.get("severity").and_then(|s| s.as_str()) == Some("error"))
                .count();
            if !findings.is_empty() || maintainer_errors > 0 {
                bail!(
                    "{} style findings, {} maintainer-acceptability errors",
                    findings.len(),
                    maintainer_errors
                );
            }
            Ok(())
        }
        RecipeCommand::Format {
            paths,
            out,
            dry_run,
        } => {
            if out.is_some() && paths.len() != 1 {
                bail!("--out requires exactly one recipe path");
            }
            for (index, path) in paths.iter().enumerate() {
                let destination = out.as_deref().filter(|_| index == 0);
                let result = if dry_run {
                    let text = std::fs::read_to_string(path)
                        .with_context(|| format!("read {}", path.display()))?;
                    format_style(&text)
                } else {
                    format_style_file(path, destination)?
                };
                println!(
                    "{}: rewritten={} remaining={}",
                    path.display(),
                    result.lines_rewritten,
                    result.remaining.len()
                );
                for finding in result.remaining {
                    println!(
                        "{}:{}:{}: {} {}",
                        path.display(),
                        finding.line,
                        finding.column,
                        finding.code,
                        finding.message
                    );
                }
            }
            Ok(())
        }
    }
}

fn run_stack(command: StackCommand) -> Result<()> {
    match command {
        StackCommand::Solve {
            easyconfigs,
            policy,
            baseline_easyconfigs,
            baseline_toolchain_version,
            lock_out,
            sbom_out,
            build_list_out,
            stack_diff_out,
        } => {
            let baseline = baseline_easyconfigs
                .as_deref()
                .or_else(|| easyconfigs.first().map(PathBuf::as_path));
            let roots = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let lock = solve_from_easyconfigs_with_baseline_version_and_extras(
                &roots,
                &policy,
                baseline,
                baseline_toolchain_version.as_deref(),
                &lock_out,
                sbom_out.as_deref(),
                SolveExtraOut {
                    build_list_out: build_list_out.as_deref(),
                    stack_diff_out: stack_diff_out.as_deref(),
                },
            )?;
            println!(
                "lock={} packages={}",
                lock_out.display(),
                lock.packages.len()
            );
            Ok(())
        }
        StackCommand::Order {
            easyconfigs,
            roots,
            out,
            oldest,
            easystack_out,
            hashes_out,
        } => {
            let trees: Vec<&Path> = easyconfigs.iter().map(PathBuf::as_path).collect();
            let parsed =
                eb_stack::parse_easyconfig_trees(&trees).map_err(|e| anyhow::anyhow!(e))?;
            let choice = if oldest {
                eb_stack::Choice::Oldest
            } else {
                eb_stack::Choice::Newest
            };
            let order = eb_stack::build_order(&parsed.candidates, &roots, choice)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            std::fs::write(&out, eb_stack::format_order(&order))?;
            let multi = eb_stack::build_order::multi_build_names(&order);
            for (name, builds) in &multi {
                println!("multiple builds of {name}: {}", builds.join(", "));
            }
            if let Some(path) = easystack_out.as_deref() {
                // Straight from the ordered list: a lock is sorted by name so
                // two locks diff cleanly, and writing that out would discard
                // the sequence this command exists to produce.
                let paths: Vec<&str> = order.iter().map(|c| c.easyconfig_path.as_str()).collect();
                std::fs::write(
                    path,
                    eb_stack::easystack::easystack_from_paths(
                        &paths,
                        &eb_stack::EasystackOptions::new(),
                    ),
                )?;
                println!("easystack={}", path.display());
            }
            if let Some(path) = hashes_out.as_deref() {
                let graph = eb_stack::build_order::build_graph(&parsed.candidates, &roots, choice)
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let sequence: Vec<eb_stack::ModuleKey> =
                    order.iter().map(eb_stack::ModuleKey::of).collect();
                let recipe_paths: std::collections::BTreeMap<eb_stack::ModuleKey, String> = order
                    .iter()
                    .map(|c| (eb_stack::ModuleKey::of(c), c.easyconfig_path.clone()))
                    .collect();
                let hashes = eb_stack::input_hashes(&graph, &sequence, &recipe_paths);
                let incomplete = hashes.values().filter(|h| !h.complete).count();
                let mut text = String::new();
                for key in &sequence {
                    if let Some(h) = hashes.get(key) {
                        text.push_str(&format!("{}  {}\n", h.hash, key));
                    }
                }
                std::fs::write(path, text)?;
                println!(
                    "hashes={} modules={} incomplete={}",
                    path.display(),
                    hashes.len(),
                    incomplete
                );
            }
            println!("order={} builds={}", out.display(), order.len());
            Ok(())
        }
        StackCommand::Easystack { lock, out, options } => {
            let lock: StackLock = load_json_file(&lock)?;
            let mut parsed = eb_stack::EasystackOptions::new();
            for spec in &options {
                let (file, rest) = spec.split_once(':').ok_or_else(|| {
                    anyhow::anyhow!("--option wants <file.eb>:<option>=<value>, got {spec}")
                })?;
                let (key, value) = rest.split_once('=').ok_or_else(|| {
                    anyhow::anyhow!("--option wants <file.eb>:<option>=<value>, got {spec}")
                })?;
                parsed
                    .entry(file.to_string())
                    .or_default()
                    .insert(key.to_string(), value.to_string());
            }
            let yaml = eb_stack::lock_to_easystack(&lock, &parsed);
            std::fs::write(&out, &yaml)?;
            let entries = yaml.lines().filter(|l| l.starts_with("- ")).count();
            println!(
                "easystack={} entries={} of={}",
                out.display(),
                entries,
                lock.packages.len()
            );
            Ok(())
        }
        StackCommand::Sbom { lock, out } => {
            let lock: StackLock = load_json_file(&lock)?;
            // The checksums and source URLs live in the easyconfigs the lock
            // names, so read them: without those a component cannot be verified
            // against the bytes it was planned from.
            let artifacts = eb_stack::artifact_facts_for_lock(&lock);
            let sbom = eb_stack::lock_to_cyclonedx_with_facts(
                &lock,
                eb_stack::SbomFacts {
                    artifacts: Some(&artifacts),
                    ..eb_stack::SbomFacts::default()
                },
            );
            write_json_pretty(&out, &sbom)?;
            println!(
                "sbom={} components={} with_artifact_facts={}",
                out.display(),
                lock.packages.len(),
                artifacts.len()
            );
            Ok(())
        }
    }
}

fn run_target(command: TargetCommand) -> Result<()> {
    match command {
        TargetCommand::List { configs } => {
            let targets = load_targets(&configs)?;
            println!("{}", serde_json::to_string_pretty(&targets)?);
            Ok(())
        }
        TargetCommand::Doctor { configs, target } => {
            let targets = load_targets(&configs)?;
            let target_config = targets
                .iter()
                .find(|candidate| candidate.name == target)
                .with_context(|| format!("target {target} is not configured"))?;
            let report = doctor_target(target_config)?;
            let ok = report.ok();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "target": report.target,
                    "ok": ok,
                    "checks": report.checks,
                }))?
            );
            if !ok {
                bail!("target {target} doctor failed");
            }
            Ok(())
        }
    }
}

fn run_campaign(command: CampaignCommand) -> Result<()> {
    match command {
        CampaignCommand::Run {
            bundle,
            configs,
            target,
            state,
        } => {
            let targets = load_targets(&configs)?;
            let target_config = targets
                .into_iter()
                .find(|candidate| candidate.name == target)
                .with_context(|| format!("target {target} is not configured"))?;
            let campaign = execute_campaign(&CampaignRequest {
                bundle,
                target: target_config,
                state_path: state,
            })?;
            println!("{}", serde_json::to_string_pretty(&campaign)?);
            if campaign.status == CampaignStatus::Failed {
                bail!("campaign build failed with typed findings in its state file");
            }
            Ok(())
        }
        CampaignCommand::Status { state } => {
            let value: serde_json::Value = load_json_file(&state)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
        CampaignCommand::Finding { command } => {
            let state = match command {
                CampaignFindingCommand::Claim { state, id, owner } => {
                    claim_finding(&state, &id, &owner)?
                }
                CampaignFindingCommand::Resolve {
                    state,
                    id,
                    owner,
                    action,
                    evidence,
                    changes,
                } => resolve_finding(
                    &state,
                    &id,
                    &owner,
                    FindingResolution {
                        action,
                        evidence,
                        changes,
                    },
                )?,
            };
            println!("{}", serde_json::to_string_pretty(&state)?);
            Ok(())
        }
    }
}

/// Read a repository index, when one was given.
///
/// Absent, the map is empty and a dependency that states no version stays an
/// error rather than being installed at a version nobody chose.
fn load_package_index(
    path: Option<&std::path::Path>,
) -> Result<std::collections::BTreeMap<String, eb_stack::ecosystem::IndexEntry>> {
    let Some(path) = path else {
        return Ok(Default::default());
    };
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read package index {}", path.display()))?;
    Ok(eb_stack::parse_package_index(&text))
}

fn load_package_source_roots(args: &PackagePlanArgs) -> Result<PackageSourceRoots> {
    let mut roots = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
    // `--easyconfigs` is both the solve robot and a closure discovery root.
    // A dependency that exists there only at another generation is a hole the
    // closure planner fills with a companion bump, which is what
    // package_plan_reuses_robot_roots_for_cross_generation_bumps pins. Dropping
    // it turns that case into an unsatisfiable solve instead. Separating the
    // two roles is defensible, but it is a contract change: retire the test and
    // say so in the CLI reference in the same commit, or overlay planning
    // silently loses cross-generation companions.
    for path in &args.easyconfigs {
        roots.push(SourceRootKind::EasyBuild, path.clone());
    }
    for path in &args.package_sources {
        let layer = PackageSourceRoots::from_path(path)
            .with_context(|| format!("load package sources {}", path.display()))?;
        roots.extend_from(&layer);
    }
    for path in &args.easybuild_sources {
        roots.push(SourceRootKind::EasyBuild, path.clone());
    }
    for path in &args.conda_sources {
        roots.push(SourceRootKind::CondaForge, path.clone());
    }
    for path in &args.spack_sources {
        roots.push(SourceRootKind::Spack, path.clone());
    }
    for path in &args.cargo_sources {
        roots.push(SourceRootKind::Cargo, path.clone());
    }
    Ok(roots)
}

fn resolve_ingest_source(
    source: &Path,
    format: Option<ForeignFormat>,
    out_dir: &Path,
) -> Result<PathBuf> {
    if source.is_file() {
        return Ok(source.to_path_buf());
    }
    if !is_registry_name(source) {
        bail!(
            "source {} is not a file or a registry name",
            source.display()
        );
    }
    let format = format.ok_or_else(|| {
        anyhow::anyhow!("--format pypi|cran|cargo is required when --source is a registry name")
    })?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid registry name {}", source.display()))?;
    let ingest = materialize_registry_name(name, format, &UreqClient, &out_dir.join("ingest"))
        .with_context(|| format!("fetch {name} as {}", format.as_str()))?;
    Ok(ingest.dump)
}

fn parse_format(value: &str) -> Result<Option<ForeignFormat>> {
    match value {
        "auto" => Ok(None),
        "conda-forge" | "conda" => Ok(Some(ForeignFormat::CondaForge)),
        "spack" => Ok(Some(ForeignFormat::Spack)),
        "pypi" => Ok(Some(ForeignFormat::Pypi)),
        "cran" => Ok(Some(ForeignFormat::Cran)),
        "cargo" | "crates" | "crates.io" => Ok(Some(ForeignFormat::Cargo)),
        "luarocks" | "lua" => Ok(Some(ForeignFormat::Luarocks)),
        "raku" | "perl6" => Ok(Some(ForeignFormat::Raku)),
        _ => {
            bail!("--format must be auto, conda-forge, spack, pypi, cran, cargo, luarocks, or raku")
        }
    }
}

fn load_package_layers(paths: &[PathBuf]) -> Result<Vec<PackageConfigLayer>> {
    paths
        .iter()
        .map(|path| {
            PackageConfigLayer::from_path(path)
                .with_context(|| format!("load package config {}", path.display()))
        })
        .collect()
}

fn load_stack_policy(path: &Path) -> Result<StackPolicy> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read stack policy {}", path.display()))?;
    if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
        serde_json::from_str(&text)
            .with_context(|| format!("parse stack policy JSON {}", path.display()))
    } else {
        toml::from_str(&text).with_context(|| format!("parse stack policy TOML {}", path.display()))
    }
}

fn unconstrained_stack_policy(toolchain: &Toolchain) -> StackPolicy {
    StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "unconstrained".into(),
        toolchain: toolchain.clone(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    }
}

fn load_targets(paths: &[PathBuf]) -> Result<Vec<BuildTarget>> {
    let layers = paths
        .iter()
        .map(|path| {
            TargetConfigLayer::from_path(path)
                .with_context(|| format!("load target config {}", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    resolve_target_layers(&layers).map_err(anyhow::Error::msg)
}

fn parse_dep_overrides(values: &[String]) -> Result<HashMap<String, String>> {
    let mut dependencies = HashMap::new();
    for value in values {
        let Some((name, version)) = value.split_once('=') else {
            bail!("--dep expects NAME=VERSION, got {value:?}");
        };
        if name.trim().is_empty() || version.trim().is_empty() {
            bail!("--dep expects non-empty NAME=VERSION, got {value:?}");
        }
        dependencies.insert(name.trim().to_string(), version.trim().to_string());
    }
    Ok(dependencies)
}

fn toolchain(name: &str, version: &str) -> Toolchain {
    Toolchain {
        name: name.to_string(),
        version: version.to_string(),
    }
}
