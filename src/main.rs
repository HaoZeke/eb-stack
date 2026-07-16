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
    check_recipe_deps, format_style, format_style_file, inspect_new_package, lint_style,
    load_json_file, lock_to_cyclonedx, packaging_gate, parse_easyconfig_trees, plan_new_package,
    plan_package_bump, plan_package_closure_with_sources, resolve_easyconfig_file,
    resolve_package_catalog_layers, solve_from_easyconfigs_with_baseline_version_and_extras,
    write_json_pretty, write_package_bundle, write_package_closure, BumpPackageRequest,
    ForeignFormat, NewPackageRequest, PackageBundle, PackageCatalogLayer, SolveExtraOut, StackLock,
    Toolchain,
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
    /// Serve the same workflows over MCP stdio.
    Mcp,
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
    /// Optional package-source catalog layers for recursive robot-hole closure.
    ///
    /// Explicit catalog entries are ordered overrides. Argument order is layer
    /// order. Closure also activates when `--package-sources` or per-kind
    /// source roots are configured.
    #[arg(long = "package-catalog", value_name = "CATALOG.toml")]
    package_catalogs: Vec<PathBuf>,
    /// Optional package-neutral source-root TOML layers (EasyBuild / conda-forge / Spack).
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
            let (plan, sbom) = inspect_new_package(
                &args.source,
                parse_format(&args.format)?,
                &toolchain,
                &layers,
            )?;
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
            let request = NewPackageRequest {
                source: args.inspect.source,
                format: parse_format(&args.inspect.format)?,
                toolchain,
                source_checksums: args.source_checksums,
                package_layers: load_package_layers(&args.inspect.package_configs)?,
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

fn run_recipe(command: RecipeCommand) -> Result<()> {
    match command {
        RecipeCommand::Check {
            recipe,
            easyconfigs,
            require_configopts,
            metadata_only,
        } => {
            let resolved = resolve_easyconfig_file(&recipe).map_err(anyhow::Error::msg)?;
            let required = require_configopts
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let gate = packaging_gate(&resolved, &required);
            if metadata_only {
                if let Err(errors) = gate {
                    bail!("packaging gate failed: {}", errors.join("; "));
                }
                println!("recipe metadata resolves");
                return Ok(());
            }
            let roots = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let tree = parse_easyconfig_trees(&roots).map_err(anyhow::Error::msg)?;
            let check = check_recipe_deps(&resolved, &tree.candidates);
            println!("{}", serde_json::to_string_pretty(&check)?);
            if let Err(errors) = gate {
                bail!("packaging gate failed: {}", errors.join("; "));
            }
            if !check.ok() {
                bail!("recipe has {} unresolved dependencies", check.missing.len());
            }
            println!("recipe resolves");
            Ok(())
        }
        RecipeCommand::Lint { paths } => {
            let mut findings = Vec::new();
            for path in paths {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("read {}", path.display()))?;
                findings.extend(lint_style(&text));
            }
            println!("{}", serde_json::to_string_pretty(&findings)?);
            if !findings.is_empty() {
                bail!("{} style findings", findings.len());
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
        StackCommand::Sbom { lock, out } => {
            let lock: StackLock = load_json_file(&lock)?;
            let sbom = lock_to_cyclonedx(&lock);
            write_json_pretty(&out, &sbom)?;
            println!("sbom={}", out.display());
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

fn load_package_source_roots(args: &PackagePlanArgs) -> Result<PackageSourceRoots> {
    let mut roots = PackageSourceRoots {
        schema_version: 1,
        source_roots: Vec::new(),
    };
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
    Ok(roots)
}

fn parse_format(value: &str) -> Result<Option<ForeignFormat>> {
    match value {
        "auto" => Ok(None),
        "conda-forge" | "conda" => Ok(Some(ForeignFormat::CondaForge)),
        "spack" => Ok(Some(ForeignFormat::Spack)),
        _ => bail!("--format must be auto, conda-forge, or spack"),
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
