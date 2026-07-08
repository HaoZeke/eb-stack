//! eb-stack: parse EasyBuild easyconfigs, resolvo SAT co-select, planned CycloneDX SBOM.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use eb_stack::{
    emit_next_generation_from_path, load_json_file, lock_to_cyclonedx, parse_easyconfig_tree,
    solve_from_easyconfigs_with_extras, solve_to_files_with_extras, EmitParams, SolveExtraOut,
    StackLock, Toolchain,
};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "eb-stack",
    version,
    about = "Parse EasyBuild .eb files, SAT-solve a co-constrained stack lock, emit planned SBOM"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Parse a tree of `.eb` files, solve with resolvo, write lock + CycloneDX SBOM.
    Solve {
        /// Root directory of easyconfigs (walked recursively for `*.eb`)
        #[arg(long)]
        easyconfigs: PathBuf,
        /// Policy JSON (toolchain, roots, pins, require_upgrade)
        #[arg(long)]
        policy: PathBuf,
        /// Optional second tree (or same tree) used to build the baseline lock for upgrades
        #[arg(long)]
        baseline_easyconfigs: Option<PathBuf>,
        #[arg(long, default_value = "stack.lock.json")]
        lock_out: PathBuf,
        #[arg(long, default_value = "stack.cdx.json")]
        sbom_out: PathBuf,
        /// Optional plain-text build list: selected easyconfigs in dependency order
        #[arg(long)]
        build_list_out: Option<PathBuf>,
        /// Optional markdown stack diff vs baseline (pasteable into a PR)
        #[arg(long)]
        stack_diff_out: Option<PathBuf>,
    },
    /// Parse easyconfigs and print JSON candidates (debug / universe dump).
    Parse {
        #[arg(long)]
        easyconfigs: PathBuf,
        #[arg(long)]
        toolchain_name: Option<String>,
        #[arg(long)]
        toolchain_version: Option<String>,
    },
    /// Legacy: solve from pre-baked universe JSON (still tested).
    SolveJson {
        #[arg(long)]
        universe: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        baseline: Option<PathBuf>,
        #[arg(long, default_value = "stack.lock.json")]
        lock_out: PathBuf,
        #[arg(long, default_value = "stack.cdx.json")]
        sbom_out: PathBuf,
        /// Optional plain-text build list: selected easyconfigs in dependency order
        #[arg(long)]
        build_list_out: Option<PathBuf>,
        /// Optional markdown stack diff vs baseline lock (pasteable into a PR)
        #[arg(long)]
        stack_diff_out: Option<PathBuf>,
    },
    /// Emit CycloneDX from an existing lock.
    Sbom {
        #[arg(long)]
        lock: PathBuf,
        #[arg(long, default_value = "stack.cdx.json")]
        out: PathBuf,
    },
    /// Produce a next-generation easyconfig from an existing recipe.
    ///
    /// Rewrites `toolchain`, optional application `version`, and named
    /// dependency / build-dependency versions; preserves all other source
    /// content. Writes `{name}-{version}-{tcname}-{tcver}.eb` under `--out-dir`
    /// (or to an explicit `--out` file path).
    Bump {
        /// Source easyconfig (`.eb`) to rewrite
        #[arg(long)]
        source: PathBuf,
        /// Target toolchain name (e.g. foss)
        #[arg(long)]
        toolchain_name: String,
        /// Target toolchain version / generation (e.g. 2025b)
        #[arg(long)]
        toolchain_version: String,
        /// Optional new application version
        #[arg(long)]
        version: Option<String>,
        /// Dependency version override as `Name=version` (repeatable).
        /// Also applied to `builddependencies`. If `version` includes an
        /// operator (`>=`, `==`, …) it replaces the whole version field;
        /// otherwise any operator on the source entry is preserved.
        #[arg(long = "dep", value_name = "NAME=VERSION")]
        deps: Vec<String>,
        /// Output directory; file is written as the conventional basename
        #[arg(long, conflicts_with = "out")]
        out_dir: Option<PathBuf>,
        /// Explicit output file path (overrides conventional directory write)
        #[arg(long, conflicts_with = "out_dir")]
        out: Option<PathBuf>,
    },
}

fn parse_dep_overrides(deps: &[String]) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    for d in deps {
        let Some((name, ver)) = d.split_once('=') else {
            bail!("--dep expects NAME=VERSION, got {d:?}");
        };
        let name = name.trim();
        let ver = ver.trim();
        if name.is_empty() || ver.is_empty() {
            bail!("--dep expects non-empty NAME=VERSION, got {d:?}");
        }
        map.insert(name.to_string(), ver.to_string());
    }
    Ok(map)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Solve {
            easyconfigs,
            policy,
            baseline_easyconfigs,
            lock_out,
            sbom_out,
            build_list_out,
            stack_diff_out,
        } => {
            let baseline = baseline_easyconfigs.unwrap_or_else(|| easyconfigs.clone());
            let lock = solve_from_easyconfigs_with_extras(
                &easyconfigs,
                &policy,
                Some(&baseline),
                &lock_out,
                &sbom_out,
                SolveExtraOut {
                    build_list_out: build_list_out.as_deref(),
                    stack_diff_out: stack_diff_out.as_deref(),
                },
            )?;
            println!(
                "parsed easyconfigs under {} -> lock {} ({} packages, engine={})",
                easyconfigs.display(),
                lock_out.display(),
                lock.packages.len(),
                lock.solver.engine
            );
            if let Some(g) = lock.package("GROMACS") {
                println!(
                    "GROMACS {} (toolchain {}-{}) <- {}",
                    g.version, g.toolchain.name, g.toolchain.version, g.easyconfig_path
                );
            }
            for p in &lock.packages {
                println!("  - {} {} ({})", p.name, p.version, p.easyconfig_path);
            }
            println!("planned SBOM {}", sbom_out.display());
            if let Some(p) = &build_list_out {
                println!("build list {}", p.display());
            }
            if let Some(p) = &stack_diff_out {
                println!("stack diff {}", p.display());
            }
        }
        Cmd::Parse {
            easyconfigs,
            toolchain_name,
            toolchain_version,
        } => {
            let mut cands = parse_easyconfig_tree(&easyconfigs).map_err(|e| anyhow::anyhow!(e))?;
            if let (Some(n), Some(v)) = (toolchain_name, toolchain_version) {
                cands.retain(|c| c.toolchain.name == n && c.toolchain.version == v);
            }
            println!("{}", serde_json::to_string_pretty(&cands)?);
        }
        Cmd::SolveJson {
            universe,
            policy,
            baseline,
            lock_out,
            sbom_out,
            build_list_out,
            stack_diff_out,
        } => {
            let lock = solve_to_files_with_extras(
                &universe,
                &policy,
                baseline.as_deref(),
                &lock_out,
                &sbom_out,
                SolveExtraOut {
                    build_list_out: build_list_out.as_deref(),
                    stack_diff_out: stack_diff_out.as_deref(),
                },
            )?;
            println!(
                "solve-json universe {} policy {} -> lock {} ({} packages, engine={})",
                universe.display(),
                policy.display(),
                lock_out.display(),
                lock.packages.len(),
                lock.solver.engine
            );
            if let Some(g) = lock.package("GROMACS") {
                println!(
                    "GROMACS {} (toolchain {}-{}) <- {}",
                    g.version, g.toolchain.name, g.toolchain.version, g.easyconfig_path
                );
            }
            for p in &lock.packages {
                println!("  - {} {} ({})", p.name, p.version, p.easyconfig_path);
            }
            println!("planned SBOM {}", sbom_out.display());
            if let Some(p) = &build_list_out {
                println!("build list {}", p.display());
            }
            if let Some(p) = &stack_diff_out {
                println!("stack diff {}", p.display());
            }
        }
        Cmd::Sbom { lock, out } => {
            let lock: StackLock = load_json_file(&lock)?;
            let sbom = lock_to_cyclonedx(&lock);
            eb_stack::write_json_pretty(&out, &sbom)?;
            println!("wrote {}", out.display());
        }
        Cmd::Bump {
            source,
            toolchain_name,
            toolchain_version,
            version,
            deps,
            out_dir,
            out,
        } => {
            let params = EmitParams {
                toolchain: Toolchain {
                    name: toolchain_name,
                    version: toolchain_version,
                },
                version,
                dep_versions: parse_dep_overrides(&deps)?,
            };
            let result = emit_next_generation_from_path(&source, &params)
                .with_context(|| format!("bump {}", source.display()))?;
            let dest = if let Some(path) = out {
                path
            } else if let Some(dir) = out_dir {
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("create out-dir {}", dir.display()))?;
                dir.join(&result.filename)
            } else {
                // Default: write beside CWD using conventional name.
                PathBuf::from(&result.filename)
            };
            if let Some(parent) = dest.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("create parent {}", parent.display()))?;
                }
            }
            std::fs::write(&dest, &result.text)
                .with_context(|| format!("write {}", dest.display()))?;
            println!("wrote {} (from {})", dest.display(), source.display());
        }
    }
    Ok(())
}
