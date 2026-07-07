//! eb-stack: parse EasyBuild easyconfigs, resolvo SAT co-select, planned CycloneDX SBOM.

use anyhow::Result;
use clap::{Parser, Subcommand};
use eb_stack::{
    load_json_file, lock_to_cyclonedx, parse_easyconfig_tree, solve_from_easyconfigs, solve_to_files,
    StackLock,
};
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
    },
    /// Emit CycloneDX from an existing lock.
    Sbom {
        #[arg(long)]
        lock: PathBuf,
        #[arg(long, default_value = "stack.cdx.json")]
        out: PathBuf,
    },
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
        } => {
            let baseline = baseline_easyconfigs.unwrap_or_else(|| easyconfigs.clone());
            let lock = solve_from_easyconfigs(
                &easyconfigs,
                &policy,
                Some(&baseline),
                &lock_out,
                &sbom_out,
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
        } => {
            let lock = solve_to_files(
                &universe,
                &policy,
                baseline.as_deref(),
                &lock_out,
                &sbom_out,
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
        }
        Cmd::Sbom { lock, out } => {
            let lock: StackLock = load_json_file(&lock)?;
            let sbom = lock_to_cyclonedx(&lock);
            eb_stack::write_json_pretty(&out, &sbom)?;
            println!("wrote {}", out.display());
        }
    }
    Ok(())
}
