//! eb-stack: parse EasyBuild easyconfigs, resolvo SAT co-select, planned CycloneDX SBOM.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use eb_stack::{
    check_recipe_deps, emit_next_generation_auto_from_path_with_opts,
    emit_next_generation_from_path, ingest_foreign_to_easyconfig_with_opts, load_json_file,
    lock_to_cyclonedx, packaging_gate, parse_easyconfig_tree, parse_easyconfig_trees,
    resolve_easyconfig_file, scaffold_missing_companions,
    solve_from_easyconfigs_with_baseline_version_and_extras, solve_to_files_with_extras,
    write_ingest_result, AutoResolveOpts, EmitParams, ForeignFormat, IngestOpts, SolveExtraOut,
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
    /// Parse a tree of `.eb` files, solve with resolvo, write lock (+ optional SBOM).
    Solve {
        /// Easyconfig tree(s) (walked recursively for `*.eb`). Repeatable: later
        /// paths override earlier ones for the same name+version+toolchain
        /// (site overlay on upstream).
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        /// Policy JSON (toolchain, roots, pins, require_upgrade)
        #[arg(long)]
        policy: PathBuf,
        /// Optional second tree (or same tree) used to build the baseline lock for upgrades
        #[arg(long)]
        baseline_easyconfigs: Option<PathBuf>,
        /// Baseline toolchain generation (version only, e.g. 2025a) when the baseline
        /// tree has multiple generations of the policy toolchain family. Default: nearest
        /// lower generation than the policy target (see version ordering).
        #[arg(long, value_name = "VERSION")]
        baseline_toolchain_version: Option<String>,
        #[arg(long, default_value = "stack.lock.json")]
        lock_out: PathBuf,
        /// Optional planned CycloneDX SBOM path (omit to skip SBOM emission).
        #[arg(long)]
        sbom_out: Option<PathBuf>,
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
    /// Packaging check: resolve one recipe and verify deps exist in robot tree(s).
    ///
    /// Cross-toolchain deps (e.g. `quill` on GCCcore while the recipe is `gfbf`)
    /// are matched by identity, not policy-toolchain filter. Exit 1 if any dep
    /// is missing or packaging gates fail.
    ///
    /// With `--scaffold-missing DIR`, write draft companion `.eb` files for every
    /// missing dep under `DIR` (letter/name/ layout) so the overlay can be filled
    /// and re-checked — the packaging prep loop for stacks like eOn.
    CheckRecipe {
        /// Recipe `.eb` to validate
        #[arg(long)]
        recipe: PathBuf,
        /// Robot easyconfig tree(s); later paths override earlier (draft overlay).
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        /// Require these substrings in configopts (repeatable)
        #[arg(long = "require-configopt")]
        require_configopts: Vec<String>,
        /// Skip the missing-dep robot check (only packaging metadata gates)
        #[arg(long)]
        metadata_only: bool,
        /// Write scaffold companion easyconfigs for missing deps into this tree
        #[arg(long = "scaffold-missing", value_name = "DIR")]
        scaffold_missing: Option<PathBuf>,
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
        /// Optional planned CycloneDX SBOM path (omit to skip SBOM emission).
        #[arg(long)]
        sbom_out: Option<PathBuf>,
        /// Optional plain-text build list: selected easyconfigs in dependency order
        #[arg(long)]
        build_list_out: Option<PathBuf>,
        /// Optional markdown stack diff vs baseline lock (pasteable into a PR)
        #[arg(long)]
        stack_diff_out: Option<PathBuf>,
    },
    /// Emit CycloneDX from an existing lock (explicit SBOM command; always writes).
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
        /// New sha256 for the source tarball; only meaningful when `--version`
        /// changes the app version (the tarball name changes with it). When
        /// omitted on a version bump, the source checksum entry's key is
        /// still renamed to the new versioned tarball name but the checksum
        /// value is left stale and a warning is printed.
        #[arg(long, value_name = "SHA256")]
        source_checksum: Option<String>,
        /// Dependency version override as `Name=version` (repeatable).
        /// Also applied to `builddependencies`. If `version` includes an
        /// operator (`>=`, `==`, …) it replaces the whole version field;
        /// otherwise any operator on the source entry is preserved.
        /// When `--easyconfigs` is set, these override auto-resolved versions.
        #[arg(long = "dep", value_name = "NAME=VERSION")]
        deps: Vec<String>,
        /// Directory of easyconfigs used to auto-resolve dependency and
        /// build-dependency versions for the target toolchain generation
        /// (hierarchy-aware: GCCcore/GCC/gfbf/gompi/… members count).
        /// When set, hand `--dep` overrides are optional.
        #[arg(long, value_name = "DIR")]
        easyconfigs: Option<PathBuf>,
        /// Optional hierarchy fixture JSON (EasyBuild `get_toolchain_hierarchy`
        /// capture). Default: built-in fixture for known generations
        /// (e.g. foss-2024a).
        #[arg(long, value_name = "PATH")]
        hierarchy_fixture: Option<PathBuf>,
        /// Keep source dependency versions when auto-resolve finds no candidate
        /// (default: fail with a non-zero exit). Versionsuffix-pinned deps are
        /// never bumped regardless of this flag.
        #[arg(long)]
        keep_old_deps: bool,
        /// Output directory; file is written as the conventional basename
        #[arg(long, conflicts_with = "out")]
        out_dir: Option<PathBuf>,
        /// Explicit output file path (overrides conventional directory write)
        #[arg(long, conflicts_with = "out_dir")]
        out: Option<PathBuf>,
    },
    /// Serve the packaging workflow as MCP tools over stdio (JSON-RPC 2.0).
    ///
    /// Exposes `eb_check_recipe`, `eb_bump`, and `eb_solve` to MCP clients
    /// (claude, codex, omp, grok). Register with e.g.
    /// `claude mcp add eb-stack -- eb-stack mcp`.
    Mcp,
    /// Ingest a foreign recipe (conda-forge meta.yaml / Spack package.py)
    /// into a parseable EasyBuild scaffold.
    ///
    /// Mechanically derives name/version/sources/deps/configopts from the foreign
    /// recipe. With `--easyconfigs`, resolves dependency versions against the
    /// target generation hierarchy (same path as `bump`). Residuals (product
    /// patches, hand pins) still surface as warnings.
    Ingest {
        /// Foreign recipe path (`meta.yaml`, `recipe.yaml`, or `package.py`)
        #[arg(long)]
        source: PathBuf,
        /// Foreign format: `conda-forge`, `spack`, or `auto` (default)
        #[arg(long, default_value = "auto", value_name = "FORMAT")]
        format: String,
        /// Target toolchain name for the scaffold (default: foss)
        #[arg(long, default_value = "foss")]
        toolchain_name: String,
        /// Target toolchain version / generation (default: 2024a)
        #[arg(long, default_value = "2024a")]
        toolchain_version: String,
        /// Robot easyconfig tree(s) for hierarchy-aware dep version resolve.
        /// Repeatable; later paths win on conflict (site overlay).
        #[arg(long, value_name = "DIR")]
        easyconfigs: Vec<PathBuf>,
        /// Keep residual foreign versions when the robot has no candidate.
        #[arg(long)]
        keep_old_deps: bool,
        /// Optional hierarchy fixture JSON (escape hatch).
        #[arg(long, value_name = "PATH")]
        hierarchy_fixture: Option<PathBuf>,
        /// Explicit output `.eb` path
        #[arg(long, conflicts_with = "out_dir")]
        out: Option<PathBuf>,
        /// Output directory; writes letter/name/conventional basename layout
        #[arg(long, conflicts_with = "out")]
        out_dir: Option<PathBuf>,
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
            baseline_toolchain_version,
            lock_out,
            sbom_out,
            build_list_out,
            stack_diff_out,
        } => {
            if easyconfigs.is_empty() {
                bail!("at least one --easyconfigs path is required");
            }
            let baseline = baseline_easyconfigs
                .unwrap_or_else(|| easyconfigs[0].clone());
            let roots: Vec<&std::path::Path> =
                easyconfigs.iter().map(|p| p.as_path()).collect();
            let lock = solve_from_easyconfigs_with_baseline_version_and_extras(
                &roots,
                &policy,
                Some(&baseline),
                baseline_toolchain_version.as_deref(),
                &lock_out,
                sbom_out.as_deref(),
                SolveExtraOut {
                    build_list_out: build_list_out.as_deref(),
                    stack_diff_out: stack_diff_out.as_deref(),
                },
            )?;
            let roots_disp = easyconfigs
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "parsed easyconfigs under [{}] -> lock {} ({} packages, engine={})",
                roots_disp,
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
            if let Some(p) = &sbom_out {
                println!("planned SBOM {}", p.display());
            }
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
            let tree = parse_easyconfig_tree(&easyconfigs).map_err(|e| anyhow::anyhow!(e))?;
            if !tree.skipped.is_empty() {
                eprintln!(
                    "parse: skipped {} unparseable easyconfig(s) under {}",
                    tree.skip_count(),
                    easyconfigs.display()
                );
                for s in tree.skipped.iter().take(20) {
                    eprintln!("  skip {}: {}", s.path, s.error);
                }
                if tree.skipped.len() > 20 {
                    eprintln!("  ... and {} more", tree.skipped.len() - 20);
                }
            }
            let mut cands = tree.candidates;
            if let (Some(n), Some(v)) = (toolchain_name, toolchain_version) {
                cands.retain(|c| c.toolchain.name == n && c.toolchain.version == v);
            }
            println!("{}", serde_json::to_string_pretty(&cands)?);
        }
        Cmd::CheckRecipe {
            recipe,
            easyconfigs,
            require_configopts,
            metadata_only,
            scaffold_missing,
        } => {
            let resolved =
                resolve_easyconfig_file(&recipe).map_err(|e| anyhow::anyhow!(e))?;
            let reqs: Vec<&str> = require_configopts.iter().map(String::as_str).collect();
            let gate = packaging_gate(&resolved, &reqs);
            if !metadata_only {
                let roots: Vec<&std::path::Path> =
                    easyconfigs.iter().map(|p| p.as_path()).collect();
                let tree = parse_easyconfig_trees(&roots).map_err(|e| anyhow::anyhow!(e))?;
                if !tree.skipped.is_empty() {
                    eprintln!(
                        "robot parse: skipped {} ({:.1}% coverage)",
                        tree.skip_count(),
                        100.0 * tree.coverage()
                    );
                }
                let check = check_recipe_deps(&resolved, &tree.candidates);
                println!("{}", serde_json::to_string_pretty(&check)?);
                if let Err(errs) = &gate {
                    for e in errs {
                        eprintln!("packaging-gate: {e}");
                    }
                }
                if !check.ok() {
                    for m in &check.missing {
                        eprintln!(
                            "missing-dep [{}] {} {}: {}",
                            m.role, m.name, m.version, m.reason
                        );
                    }
                    if let Some(dir) = scaffold_missing.as_ref() {
                        let written = scaffold_missing_companions(
                            &check.missing,
                            dir,
                            &resolved.toolchain,
                        )
                        .map_err(|e| anyhow::anyhow!(e))?;
                        for s in &written {
                            if s.skipped_existing {
                                eprintln!(
                                    "scaffold-skip: {} (exists)",
                                    s.path
                                );
                            } else {
                                eprintln!(
                                    "scaffold-write: {} [{}/{} {}]",
                                    s.path, s.role, s.name, s.version
                                );
                            }
                        }
                        eprintln!(
                            "scaffold: {} companion path(s) under {} — fill sources/checksums then re-run check-recipe",
                            written.len(),
                            dir.display()
                        );
                    }
                    bail!(
                        "check-recipe failed: {} missing dep(s), packaging_gate={}",
                        check.missing.len(),
                        gate.is_ok()
                    );
                }
                if gate.is_err() {
                    bail!(
                        "check-recipe failed: packaging_gate failed (deps complete)"
                    );
                }
                eprintln!(
                    "check-recipe OK: {} {} ({}) easyblock={:?} moduleclass={:?} checksums={} found={}",
                    check.name,
                    check.version,
                    check.toolchain.label(),
                    check.easyblock,
                    check.moduleclass,
                    check.checksum_count,
                    check.found.len()
                );
            } else if let Err(errs) = gate {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name": resolved.name,
                        "version": resolved.version,
                        "toolchain": resolved.toolchain,
                        "easyblock": resolved.easyblock,
                        "configopts": resolved.configopts,
                        "moduleclass": resolved.moduleclass,
                        "checksums": resolved.checksums,
                        "homepage": resolved.homepage,
                    }))?
                );
                for e in &errs {
                    eprintln!("packaging-gate: {e}");
                }
                bail!("check-recipe metadata-only failed: {} issue(s)", errs.len());
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name": resolved.name,
                        "version": resolved.version,
                        "toolchain": resolved.toolchain,
                        "easyblock": resolved.easyblock,
                        "configopts": resolved.configopts,
                        "moduleclass": resolved.moduleclass,
                        "checksums": resolved.checksums,
                        "homepage": resolved.homepage,
                    }))?
                );
                eprintln!("check-recipe metadata OK");
            }
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
                sbom_out.as_deref(),
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
            if let Some(p) = &sbom_out {
                println!("planned SBOM {}", p.display());
            }
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
            source_checksum,
            deps,
            easyconfigs,
            hierarchy_fixture,
            keep_old_deps,
            out_dir,
            out,
        } => {
            let toolchain = Toolchain {
                name: toolchain_name,
                version: toolchain_version,
            };
            let hand = parse_dep_overrides(&deps)?;
            let result = if let Some(ec_dir) = easyconfigs {
                emit_next_generation_auto_from_path_with_opts(
                    &source,
                    &toolchain,
                    &ec_dir,
                    None,
                    hierarchy_fixture.as_deref(),
                    &hand,
                    version,
                    source_checksum,
                    &AutoResolveOpts {
                        keep_old: keep_old_deps,
                    },
                )
                .with_context(|| {
                    format!(
                        "bump {} with auto-resolve from {}",
                        source.display(),
                        ec_dir.display()
                    )
                })?
            } else {
                let params = EmitParams {
                    toolchain,
                    version,
                    dep_versions: hand,
                    source_checksum,
                };
                emit_next_generation_from_path(&source, &params)
                    .with_context(|| format!("bump {}", source.display()))?
            };
            for w in &result.warnings {
                eprintln!("WARNING: {w}");
            }
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
        Cmd::Mcp => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            eb_stack::mcp::run_server(stdin.lock(), stdout.lock())
                .context("mcp stdio server")?;
        }
        Cmd::Ingest {
            source,
            format,
            toolchain_name,
            toolchain_version,
            easyconfigs,
            keep_old_deps,
            hierarchy_fixture,
            out,
            out_dir,
        } => {
            let fmt = match format.to_ascii_lowercase().as_str() {
                "auto" => None,
                "conda" | "conda-forge" | "cf" => Some(ForeignFormat::CondaForge),
                "spack" => Some(ForeignFormat::Spack),
                other => bail!(
                    "unknown --format {other:?}; expected auto, conda-forge, or spack"
                ),
            };
            let toolchain = Toolchain {
                name: toolchain_name,
                version: toolchain_version,
            };
            let opts = IngestOpts {
                easyconfigs,
                keep_old_deps,
                hierarchy_fixture,
            };
            let result =
                ingest_foreign_to_easyconfig_with_opts(&source, fmt, &toolchain, &opts)
                    .with_context(|| format!("ingest {}", source.display()))?;
            for w in &result.warnings {
                eprintln!("WARNING: {w}");
            }
            let dest = write_ingest_result(
                &result,
                &toolchain,
                out.as_deref(),
                out_dir.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "wrote {} (from {} as {})",
                dest.display(),
                source.display(),
                result.recipe.format.as_str()
            );
        }
    }
    Ok(())
}
