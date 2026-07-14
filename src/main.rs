//! Version-one command surface for canonical package planning and build campaigns.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use eb_stack::package::StackPolicy;
use eb_stack::package_config::ProfileConfigLayer;
use eb_stack::{
    check_recipe_deps, emit_next_generation_auto_from_path_with_opts,
    emit_next_generation_from_path, format_style, format_style_file, inspect_new_package,
    lint_style, load_json_file, lock_to_cyclonedx, packaging_gate, parse_easyconfig_trees,
    plan_new_package, resolve_easyconfig_file, scaffold_missing_companions,
    solve_from_easyconfigs_with_baseline_version_and_extras, write_json_pretty,
    write_package_bundle, AutoResolveOpts, EmitParams, ForeignFormat, NewPackageRequest,
    PackageBundle, SolveExtraOut, StackLock, Toolchain,
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
    #[arg(long = "profile-config")]
    profile_configs: Vec<PathBuf>,
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
    #[arg(long)]
    easyconfigs: Option<PathBuf>,
    #[arg(long)]
    hierarchy_fixture: Option<PathBuf>,
    #[arg(long)]
    keep_old_deps: bool,
    #[arg(long, conflicts_with = "out")]
    out_dir: Option<PathBuf>,
    #[arg(long, conflicts_with = "out_dir")]
    out: Option<PathBuf>,
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
        #[arg(long)]
        scaffold_missing: Option<PathBuf>,
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
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Package { command } => run_package(command),
        Command::Recipe { command } => run_recipe(command),
        Command::Stack { command } => run_stack(command),
        Command::Target { command } => run_target(command),
        Command::Campaign { command } => run_campaign(command),
        Command::Mcp => eb_stack::mcp::serve_stdio().map_err(anyhow::Error::msg),
    }
}

fn run_package(command: PackageCommand) -> Result<()> {
    match command {
        PackageCommand::Inspect(args) => {
            let toolchain = toolchain(&args.toolchain_name, &args.toolchain_version);
            let layers = load_profile_layers(&args.profile_configs)?;
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
            let bundle = plan_new_package(&NewPackageRequest {
                source: args.inspect.source,
                format: parse_format(&args.inspect.format)?,
                toolchain,
                profile_layers: load_profile_layers(&args.inspect.profile_configs)?,
                easyconfig_roots: args.easyconfigs,
                stack_policy,
            })?;
            let written = write_package_bundle(&bundle, &args.inspect.out_dir)?;
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
        PackageCommand::Bump(args) => run_package_bump(args),
    }
}

fn run_package_bump(args: PackageBumpArgs) -> Result<()> {
    let toolchain = toolchain(&args.toolchain_name, &args.toolchain_version);
    let overrides = parse_dep_overrides(&args.dependencies)?;
    let result = if let Some(easyconfigs) = args.easyconfigs.as_deref() {
        emit_next_generation_auto_from_path_with_opts(
            &args.source,
            &toolchain,
            args.version.clone(),
            args.source_checksum.clone(),
            easyconfigs,
            args.hierarchy_fixture.as_deref(),
            &overrides,
            &AutoResolveOpts {
                keep_old: args.keep_old_deps,
                use_consensus: true,
            },
        )?
    } else {
        emit_next_generation_from_path(
            &args.source,
            &EmitParams {
                toolchain: toolchain.clone(),
                version: args.version.clone(),
                dep_versions: overrides,
                source_checksum: args.source_checksum.clone(),
            },
        )?
    };
    let destination = output_path(
        args.out.as_deref(),
        args.out_dir.as_deref(),
        &result.filename,
    );
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&destination, result.text)?;
    println!("easyconfig={}", destination.display());
    for warning in result.warnings {
        eprintln!("warning={warning}");
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
            scaffold_missing,
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
            if let Some(directory) = scaffold_missing.as_deref() {
                scaffold_missing_companions(&check.missing, directory, &resolved.toolchain)
                    .map_err(anyhow::Error::msg)?;
            }
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
                println!("{}", serde_json::to_string_pretty(&result)?);
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
            bail!(
                "target configuration is not loaded by this build: {}",
                display_paths(&configs)
            )
        }
        TargetCommand::Doctor { configs, target } => {
            bail!(
                "target {target} cannot be checked before loading: {}",
                display_paths(&configs)
            )
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
        } => bail!(
            "campaign target {target} is not loaded from {} (bundle={}, state={})",
            display_paths(&configs),
            bundle.display(),
            state.display()
        ),
        CampaignCommand::Status { state } => {
            let value: serde_json::Value = load_json_file(&state)?;
            println!("{}", serde_json::to_string_pretty(&value)?);
            Ok(())
        }
    }
}

fn parse_format(value: &str) -> Result<Option<ForeignFormat>> {
    match value {
        "auto" => Ok(None),
        "conda-forge" | "conda" => Ok(Some(ForeignFormat::CondaForge)),
        "spack" => Ok(Some(ForeignFormat::Spack)),
        _ => bail!("--format must be auto, conda-forge, or spack"),
    }
}

fn load_profile_layers(paths: &[PathBuf]) -> Result<Vec<ProfileConfigLayer>> {
    paths
        .iter()
        .map(|path| {
            ProfileConfigLayer::from_path(path)
                .with_context(|| format!("load profile config {}", path.display()))
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

fn output_path(explicit: Option<&Path>, directory: Option<&Path>, filename: &str) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or_else(|| directory.map(|directory| directory.join(filename)))
        .unwrap_or_else(|| PathBuf::from(filename))
}

fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(",")
}
