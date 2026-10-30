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
    lint_recipe_files, lint_recipe_files_against, load_json_file, materialize_registry_name,
    packaging_gate, parse_easyconfig_trees, plan_new_package_with, plan_package_bump_with,
    plan_package_closure_with_sources, resolve_easyconfig_file, resolve_package_catalog_layers,
    write_json_pretty, write_package_bundle, write_package_closure, BumpPackageRequest,
    ForeignFormat, NewPackageRequest, PackageBundle, PackageCatalogLayer, PackageEmitOptions,
    SolveExtraOut, StackLock, Toolchain, UreqClient,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn usage(message: impl std::fmt::Display) -> anyhow::Error {
    eb_stack::cli_parse::usage(message)
}

fn is_usage(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.is::<eb_stack::cli_parse::UsageError>())
}

#[derive(Parser, Debug)]
#[command(
    name = "eb-stack",
    version,
    about = "Canonical SBOM, build-manifest, Resolvo, EasyBuild, and campaign workflows",
    after_long_help = "Exit status is 0 when the command finished, 1 when the work failed, and 2 when the command line was rejected.\n\nA directory of artifacts is --out-dir. A single output file is --out.\n\nExamples:\n  eb-stack package inspect --source recipe.yaml --format conda-forge --toolchain foss/2026.1 --out-dir bundle\n  eb-stack --help"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Parse, solve, and emit package artifacts.
    #[command(
        after_long_help = "Examples:\n  eb-stack package inspect --source recipe.yaml --format conda-forge --toolchain foss/2026.1 --out-dir bundle"
    )]
    Package {
        #[command(subcommand)]
        command: Box<PackageCommand>,
    },
    /// Check and format EasyBuild recipes.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe lint recipe.eb\n  eb-stack recipe check --recipe recipe.eb --easyconfigs easyconfigs/"
    )]
    Recipe {
        #[command(subcommand)]
        command: RecipeCommand,
    },
    /// Solve EasyBuild stack locks and SBOMs.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack solve --easyconfigs easyconfigs/ --toolchain foss/2026.1 --root zlib"
    )]
    Stack {
        #[command(subcommand)]
        command: Box<StackCommand>,
    },
    /// Inspect declarative build targets.
    #[command(
        after_long_help = "Examples:\n  eb-stack target list --config target.toml\n  eb-stack target doctor --config target.toml --target local"
    )]
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// Run and inspect persisted build campaigns.
    #[command(after_long_help = "Examples:\n  eb-stack campaign status --state campaign.json")]
    Campaign {
        #[command(subcommand)]
        command: CampaignCommand,
    },
    /// Read the reproduction grind's score artifacts.
    #[command(after_long_help = "Examples:\n  eb-stack repro summary --scores scores/")]
    Repro {
        #[command(subcommand)]
        command: ReproCommand,
    },
    /// Serve each CLI workflow over stdio, including pr-overlap, rebuild-set,
    /// component-reach, and easyblock-reach.
    #[command(
        after_long_help = "Examples:\n  eb-stack mcp --workspace .\n\nThe server reads JSON-RPC on stdin and writes it on stdout. --workspace sets EB_STACK_WORKSPACE for the recipe file tools. When it is omitted, an existing EB_STACK_WORKSPACE is kept; otherwise the server uses its current directory."
    )]
    Mcp {
        /// Workspace root for the recipe file tools.
        ///
        /// Sets `EB_STACK_WORKSPACE` for this process. When omitted, an existing
        /// `EB_STACK_WORKSPACE` is kept; otherwise the server uses its current directory.
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// Which easyconfigs an easyblock or framework change reaches.
    #[command(
        after_long_help = "Examples:\n  eb-stack easyblock-reach --easyblocks easybuild/easyblocks --easyconfigs easyconfigs/ --file easybuild/easyblocks/generic/cmakemake.py"
    )]
    EasyblockReach(Box<EasyblockReachArgs>),
    /// Which open pull requests would build the same modules.
    ///
    /// Reads each pull request's changes as data (never runs them). For a
    /// software-layer pull request, closes the added easystack entries over
    /// the robot path against an installed module list. For an easyconfigs
    /// pull request, closes the changed easyconfigs over the base tree with
    /// those files overlaid. Writes one TOML record per pull request plus an
    /// overlap index and a Markdown report. No build and no model.
    #[command(
        after_long_help = "Examples:\n  eb-stack pr-overlap --pr pr.json --easyconfigs easyconfigs/ --installed installed.txt"
    )]
    PrOverlap(Box<PrOverlapArgs>),
    /// Which installed modules an in-place easyconfig patch forces a rebuild of.
    ///
    /// The closure is everything that depends on the patched module within
    /// its toolchain generation. Resolves the deployed easystacks, then the
    /// installed reverse dependents of each changed easyconfig. Classification
    /// uses the easyconfig graph only: a runtime dependency needs no rebuild,
    /// a builddependency only needs review, and the patched easyconfig or a
    /// bundle that ships it as an extension must be rebuilt.
    #[command(
        after_long_help = "Examples:\n  eb-stack rebuild-set --changed zlib-1.3.1-GCCcore-15.2.0.eb --easystack stack.yml --easyconfigs easyconfigs/ --installed installed.txt"
    )]
    RebuildSet(Box<RebuildSetArgs>),
    /// List what carries a component in one generation, then the rebuild closure.
    #[command(
        after_long_help = "Examples:\n  eb-stack component-reach --component curl --version '>=8,<9' --generation 2026.1 --easyconfigs easyconfigs/"
    )]
    ComponentReach(Box<ComponentReachArgs>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum OverlapKind {
    /// Easystack additions on a software-layer pull request.
    #[value(name = "software-layer")]
    SoftwareLayer,
    /// Easyconfig files on an easybuild-easyconfigs pull request.
    #[value(name = "easyconfigs")]
    Easyconfigs,
}

/// The error clap printed when `--installed` was a required flag.
const INSTALLED_REQUIRED: &str = "\
error: the following required arguments were not provided:
  --installed <INSTALLED>

Usage: eb-stack pr-overlap --pr <PRS>... --easyconfigs <EASYCONFIGS> --installed <INSTALLED>

For more information, try '--help'.
";

#[derive(clap::Args, Debug)]
struct PrOverlapArgs {
    /// Pull request JSON: one object `{number, head_sha, title, easystacks:
    /// [{path, base, head}], easyconfigs: [{path, base, head}]}` or an array
    /// of them.
    ///
    /// Repeat the flag, or pass several paths after one flag. A shell glob
    /// `in/prs/*.json` is the second form.
    #[arg(long = "pr", num_args = 1.., required_unless_present = "open")]
    prs: Vec<PathBuf>,
    /// Read open easyconfigs pull requests from `OWNER/NAME` over HTTPS.
    ///
    /// Requires `--kind easyconfigs`. Those pull requests are added to any
    /// `--pr` files. `GITHUB_TOKEN` or `GH_TOKEN` is sent to api.github.com when set. A repository that is not `owner/name`
    /// is an error before a request is sent.
    #[arg(long)]
    open: Option<String>,
    /// Git ref for the base text of files read by `--open`.
    ///
    /// Defaults to `develop`. A branch such as `release/2026.1` is a valid
    /// ref. A ref git would reject is an error before a request is sent.
    #[arg(long = "base-ref")]
    base_ref: Option<String>,
    /// `software-layer` (the default) reads easystack additions. `easyconfigs`
    /// reads changed `.eb` files.
    #[arg(long, value_enum, default_value = "software-layer")]
    kind: OverlapKind,
    /// Easyconfig tree of the EasyBuild release the stack uses. Repeatable;
    /// later trees take precedence.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Easyconfigs for one `from-commit` pin, as `<commit>=<dir>`. Repeatable.
    #[arg(long = "commit-tree")]
    commit_trees: Vec<String>,
    /// Installed modules, one `Name/VersionFull` per line.
    ///
    /// Required for `--kind software-layer`. For `--kind easyconfigs`,
    /// `to_build` is the closure minus this list when the flag is set, and
    /// the new modules when it is absent.
    #[arg(long)]
    installed: Option<PathBuf>,
    /// Stack version whose easystacks count. Ignored for `--kind easyconfigs`.
    #[arg(long, default_value = "2026.06")]
    eessi_version: String,
    /// Label for where the installed list came from, e.g. a CPU target.
    #[arg(long, default_value = "")]
    installed_label: String,
    /// Skip easystack paths containing this substring. Repeatable.
    #[arg(long = "exclude-path")]
    exclude_paths: Vec<String>,
    /// Keep only easystack paths containing this substring. Repeatable.
    /// With none set, every path that survives `--exclude-path` is kept.
    #[arg(long = "include-path")]
    include_paths: Vec<String>,
    /// Dependencies to drop the way EasyBuild's `--filter-deps` drops them.
    ///
    /// Comma-separated. A name drops every version and the whole subtree under
    /// it. `name=version` drops that version. `name=[low:high]` drops a
    /// version range, with `[`/`]` inclusive and the opposite bracket
    /// exclusive. When the flag is omitted, `EASYBUILD_FILTER_DEPS` is used
    /// if it is set.
    #[arg(long, value_delimiter = ',')]
    filter_deps: Vec<String>,
    /// Lmod `.modulerc.lua` file, or a module tree to scan for them.
    ///
    /// `module_version("Name/full", "alias")` makes `Name/alias` installed
    /// when `Name/full` is. Repeatable.
    #[arg(long = "installed-modulerc")]
    installed_modulerc: Vec<PathBuf>,
    /// Installed easyblocks package (`easybuild/easyblocks`). Repeatable.
    ///
    /// Passing any root checks every easyblock class. A software-specific
    /// class none of them defines is `easyblock <class> missing` and is not
    /// counted as a module to build.
    #[arg(long = "easyblocks")]
    easyblocks: Vec<PathBuf>,
    /// Easyblocks added by one `include-easyblocks-from-commit`, as
    /// `<commit>=<dir>`. The directory is the easyblock files that commit
    /// touches, which is what EasyBuild includes ahead of the installed
    /// package. Repeatable.
    #[arg(long = "easyblocks-commit")]
    easyblocks_commits: Vec<String>,
    /// Timestamp to record (RFC 3339); defaults to now, in UTC.
    #[arg(long)]
    generated_at: Option<String>,
    /// Write `merge-order.toml`: an order that builds each shared module once.
    #[arg(long)]
    merge_order: bool,
    /// Pull request already merged. Repeat the flag. Implies `--merge-order`.
    #[arg(long)]
    merged: Vec<u64>,
    /// Pull request to judge on its own. Repeat the flag.
    ///
    /// Writes `merge-impact.toml`: other pull requests whose build set shrinks,
    /// or whose depends-on edge from this pull request resolves. This is the
    /// effect of merging this pull request alone, not the residual after
    /// earlier steps of `--merge-order`.
    #[arg(long)]
    impact: Vec<u64>,
    /// Repository URL used for links in the Markdown report.
    ///
    /// Defaults to `https://github.com/EESSI/software-layer`, or to
    /// `https://github.com/easybuilders/easybuild-easyconfigs` for
    /// `--kind easyconfigs`.
    #[arg(long)]
    repo_url: Option<String>,
    /// Output directory: `prs/<N>.toml`, `overlap.toml`, `overlap.md`.
    #[arg(long, default_value = "pr-overlap")]
    out_dir: PathBuf,
    /// Close every entry of each changed easystack, not only added lines.
    ///
    /// The EESSI bot builds every entry of a changed easystack that is not
    /// installed yet. With one `--installed` list, that set is `to_build`.
    /// `--arch-installed` implies this flag.
    #[arg(long)]
    bot_build_set: bool,
    /// Installed modules for one architecture, as `ARCH=FILE`. Repeatable.
    ///
    /// Implies `--bot-build-set`. Each file is one `Name/VersionFull` per
    /// line, the same as `--installed`. The name before `=` is the label
    /// recorded for that architecture. Replaces `--installed`.
    #[arg(long = "arch-installed")]
    arch_installed: Vec<String>,
}

#[derive(clap::Args, Debug)]
struct RebuildSetArgs {
    /// Patched easyconfig filename. Repeatable. A value that is a file and
    /// does not end in `.eb` is read as a list, one filename per line.
    #[arg(long = "changed", required_unless_present = "findings")]
    changed: Vec<String>,
    /// Deployed easystack. Repeatable. `from-commit` is resolved with
    /// `--commit-tree`, the same way as `pr-overlap`. Without it, every
    /// installed module is a root, read as `Name-VersionFull.eb`.
    #[arg(long = "easystack")]
    easystacks: Vec<PathBuf>,
    /// Vulnerability scanner export (CSV with `severity`, `vuln_id`,
    /// `package`, and `used_in`). Every installed module in `used_in` is
    /// patched; writes `findings.json`, `findings.md`, and `findings.csv`.
    #[arg(long)]
    findings: Option<PathBuf>,
    /// Easyconfig tree of the EasyBuild release the stack uses. Repeatable;
    /// later trees take precedence.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Easyconfigs for one `from-commit` pin, as `<commit>=<dir>`. Repeatable.
    #[arg(long = "commit-tree")]
    commit_trees: Vec<String>,
    /// Installed modules, one `Name/VersionFull` per line.
    #[arg(long)]
    installed: PathBuf,
    /// Label for where the installed list came from, for example a CPU target.
    #[arg(long, default_value = "")]
    installed_label: String,
    /// Dependencies to drop the way EasyBuild's `--filter-deps` drops them.
    ///
    /// Comma-separated, same matching as `pr-overlap`. When the flag is
    /// omitted, `EASYBUILD_FILTER_DEPS` is used if it is set.
    #[arg(long, value_delimiter = ',')]
    filter_deps: Vec<String>,
    /// Lmod `.modulerc.lua` file, or a module tree to scan for them. Repeatable.
    #[arg(long = "installed-modulerc")]
    installed_modulerc: Vec<PathBuf>,
    /// Installed easyblocks package (`easybuild/easyblocks`). Repeatable.
    ///
    /// A missing class is recorded and left in the set: the module is already
    /// installed.
    #[arg(long = "easyblocks")]
    easyblocks: Vec<PathBuf>,
    /// Easyblocks added by one `include-easyblocks-from-commit`, as
    /// `<commit>=<dir>`. Repeatable.
    #[arg(long = "easyblocks-commit")]
    easyblocks_commits: Vec<String>,
    /// Timestamp to record (RFC 3339); defaults to now, in UTC.
    #[arg(long)]
    generated_at: Option<String>,
    /// Installed EESSI prefix (`$EESSI_SOFTWARE_PATH`).
    ///
    /// Static archives are read from `<prefix>/software/<name>/<version>/lib*/`.
    /// A patched module with no `*.a` there cannot have been linked
    /// statically, so a builddependency-only dependent is no-rebuild.
    #[arg(long)]
    prefix: Option<PathBuf>,
    /// Compare defined dynamic symbols of the remaining review dependents
    /// with the patched library's exports. Requires `--prefix`.
    ///
    /// A dependent that defines one of those exports is must-rebuild. A
    /// dependent that defines none is no-rebuild. An empty export set stays
    /// in review. The scan sees dynamic symbols only: a hidden or inlined
    /// static link is a false no-rebuild, and a short name shared with
    /// another library is a false must-rebuild.
    #[arg(long)]
    check_static: bool,
    /// Output directory: `rebuild-set.json`, `rebuild-set.md`, `rebuilds/`.
    #[arg(long, default_value = "rebuild-set")]
    out_dir: PathBuf,
}

#[derive(clap::Args, Debug)]
struct ComponentReachArgs {
    /// Component name. ASCII case is ignored, so `curl` matches `cURL`.
    ///
    /// Pass this, `--purl`, or `--cve`.
    #[arg(long, required_unless_present_any = ["purl", "cve"], conflicts_with = "cve")]
    component: Option<String>,
    /// Exact version, `*`, or a requirement such as `>=8,<9`.
    ///
    /// Required with `--component`. With `--purl`, it replaces the version
    /// in the URL. Ignored with `--cve`, which takes versions from OSV.
    #[arg(long)]
    version: Option<String>,
    /// Package URL, for example `pkg:cargo/rustls-webpki@0.102.8`.
    #[arg(long, conflicts_with_all = ["component", "cve"])]
    purl: Option<String>,
    /// CVE id, or another OSV id. Affected packages come from OSV.
    #[arg(long, conflicts_with_all = ["component", "purl", "version"])]
    cve: Option<String>,
    /// Toolchain generation, for example `2026.1`.
    #[arg(long)]
    generation: String,
    /// Easyconfig tree. Repeatable; later trees take precedence.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Installed modules, one `Name/version-toolchain` per line.
    #[arg(long)]
    installed: Option<PathBuf>,
    /// Deployed easystack. Repeatable. With none, the generation is the stack.
    #[arg(long = "easystack")]
    easystacks: Vec<PathBuf>,
    /// Deployed easystack fetched over HTTPS. Repeatable.
    #[arg(long = "easystack-url")]
    easystack_urls: Vec<String>,
    /// Compat-layer components, `name version` or `name==version` per line.
    #[arg(long)]
    compat_layer: Option<PathBuf>,
    /// Dependencies to drop before the closure, EasyBuild `--filter-deps`.
    #[arg(long, value_delimiter = ',')]
    filter_deps: Vec<String>,
    /// Timestamp to record (RFC 3339); defaults to now, in UTC.
    #[arg(long)]
    generated_at: Option<String>,
    /// Output directory: `component-reach.toml`, `.json`, and `.md`.
    #[arg(long, default_value = "component-reach")]
    out_dir: PathBuf,
    /// Directory of recorded OSV responses (`queries.json` plus one body per
    /// query). Absent, `--cve` asks api.osv.dev. Hidden: the reach tests use
    /// it so they do not depend on the live records.
    #[arg(long, hide = true)]
    osv_cache: Option<PathBuf>,
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
    #[command(
        after_long_help = "Examples:\n  eb-stack repro summary --scores scores/\n  eb-stack repro summary --scores scores/ --ratchet tests/repro_ratchet.json --json"
    )]
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
    #[command(
        after_long_help = "Examples:\n  eb-stack package inspect --source recipe.yaml --format conda-forge --toolchain foss/2026.1 --out-dir bundle"
    )]
    Inspect(PackageInspectArgs),
    /// Resolve every declared profile and emit a canonical artifact bundle.
    #[command(
        after_long_help = "Examples:\n  eb-stack package plan --source recipe.yaml --format conda-forge --toolchain foss/2026.1 --easyconfigs easyconfigs/ --stack-policy stack.toml --out-dir bundle"
    )]
    Plan(PackagePlanArgs),
    /// Retarget an existing EasyBuild recipe using hierarchy + Resolvo selection.
    #[command(
        after_long_help = "Examples:\n  eb-stack package bump --source zlib-1.3.1-foss-2023b.eb --toolchain foss/2024a --easyconfigs easyconfigs/ --out-dir bundle\n  eb-stack package bump --source tqdm-4.67.3-GCCcore-14.3.0.eb --toolchain GCCcore/15.2.0 --latest --fetch-checksums --easyconfigs easyconfigs/ --out-dir bundle"
    )]
    Bump(PackageBumpArgs),
    /// Report the newest stable upstream release of EasyBuild recipes.
    ///
    /// The backend comes from each recipe's `source_urls` (PyPI, GitHub,
    /// GitLab, CRAN, crates.io, or the HTTP directory). Every listed version
    /// is printed as kept, or dropped with the reason: drafts, pre-releases,
    /// yanked releases, and the per-package override. GitHub requests send
    /// `GITHUB_TOKEN` or `GH_TOKEN` when set.
    #[command(
        after_long_help = "Examples:\n  eb-stack package latest --source easyconfigs/t/tqdm/tqdm-4.67.3-GCCcore-15.2.0.eb\n  eb-stack package latest --source a.eb --source b.eb --overrides site-overrides.toml --json"
    )]
    Latest(PackageLatestArgs),
}

#[derive(clap::Args, Debug)]
struct PackageLatestArgs {
    /// Easyconfig to check. Repeatable.
    #[arg(long = "source", required = true)]
    sources: Vec<PathBuf>,
    /// Overrides TOML, applied after the ones eb-stack ships. Repeatable.
    #[arg(long = "overrides")]
    overrides: Vec<PathBuf>,
    /// Print one JSON array instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args, Debug)]
struct PackageInspectArgs {
    /// Foreign recipe file, or a registry name when `--format` is `pypi`, `cran`, or `cargo`.
    #[arg(long)]
    source: PathBuf,
    /// Recipe language. `auto` detects a file. `conda`, `crates`, `crates.io`, `lua`, and `perl6` are accepted spellings.
    #[arg(long, default_value = "auto")]
    format: String,
    /// Accepted for compatibility. Hidden. Prefer `--toolchain name/version`.
    #[arg(long, hide = true)]
    toolchain_name: Option<String>,
    /// Accepted for compatibility. Hidden. Prefer `--toolchain name/version`.
    ///
    /// An empty version is kept.
    #[arg(long, hide = true)]
    toolchain_version: Option<String>,
    /// Target toolchain as `name/version` (`foss/2026.1`) or `system`.
    ///
    /// `system` and `SYSTEM` select the SYSTEM toolchain. Its version is
    /// empty. `system/` is the same value. Same spelling as `stack solve
    /// --toolchain`.
    #[arg(
        long = "toolchain",
        value_name = "NAME/VERSION",
        required_unless_present_any = ["toolchain_name", "toolchain_version"],
        conflicts_with_all = ["toolchain_name", "toolchain_version"]
    )]
    toolchain_spec: Option<String>,
    /// Package-config TOML. Repeatable. Later layers override earlier ones.
    #[arg(long = "package-config")]
    package_configs: Vec<PathBuf>,
    /// Directory for `package.plan.json` and `package.sbom.cdx.json`.
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(clap::Args, Debug)]
struct PackagePlanArgs {
    #[command(flatten)]
    inspect: PackageInspectArgs,
    /// Easyconfig tree used as the robot. Repeatable; later trees win on one identity.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Stack policy TOML, or JSON when the path ends in `.json`.
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
    /// Render an easyblock skeleton for the package into the bundle.
    ///
    /// Writes `easyblocks/<letter>/<module>.py`, ships it with the recipes,
    /// names the derived class as the easyblock, and moves the variant
    /// options it sets out of the emitted recipes. Without the flag a package
    /// that owes an easyblock gets a `missing-easyblock` residual instead.
    #[arg(long = "emit-easyblock")]
    emit_easyblock: bool,
}

#[derive(clap::Args, Debug)]
struct PackageBumpArgs {
    /// Easyconfig to retarget.
    #[arg(long)]
    source: PathBuf,
    /// Accepted for compatibility. Hidden. Prefer `--toolchain name/version`.
    #[arg(long, hide = true)]
    toolchain_name: Option<String>,
    /// Accepted for compatibility. Hidden. Prefer `--toolchain name/version`.
    ///
    /// An empty version is kept.
    #[arg(long, hide = true)]
    toolchain_version: Option<String>,
    /// Target toolchain as `name/version` (`foss/2024a`) or `system`.
    ///
    /// `system` and `SYSTEM` select the SYSTEM toolchain. Its version is
    /// empty. `system/` is the same value. Same spelling as `stack solve
    /// --toolchain`.
    #[arg(
        long = "toolchain",
        value_name = "NAME/VERSION",
        required_unless_present_any = ["toolchain_name", "toolchain_version"],
        conflicts_with_all = ["toolchain_name", "toolchain_version"]
    )]
    toolchain_spec: Option<String>,
    /// Package version to write. Absent, the recipe version is kept.
    #[arg(long, conflicts_with = "latest")]
    version: Option<String>,
    /// Move to the newest stable upstream release, as `package latest`
    /// decides it from the recipe's `source_urls` and the overrides.
    #[arg(long)]
    latest: bool,
    /// Overrides TOML for `--latest` and `--latest-secondary`, applied after
    /// the ones eb-stack ships. Repeatable. A later source is keyed by the
    /// component its file name starts with (`libatomic_ops`).
    #[arg(long = "overrides")]
    overrides: Vec<PathBuf>,
    /// Move every later source whose version a `local_*` variable holds
    /// (gc's `libatomic_ops-%s.tar.gz` % `local_libatomic_version`) to its
    /// newest stable upstream release, read as `--latest` reads the
    /// package's own, and write its digest. Each move is recorded as a
    /// `secondary-source` residual.
    #[arg(long = "latest-secondary", requires = "fetch_checksums")]
    latest_secondary: bool,
    /// Drop a patch the operator decided against, as `NAME=REASON`.
    /// Repeatable. The reason is recorded in the patch decision beside what
    /// the new source alone said. A name the recipe or its own extension
    /// does not list fails the bump.
    #[arg(
        long = "drop-patch",
        value_name = "NAME=REASON",
        requires = "fetch_checksums"
    )]
    drop_patches: Vec<String>,
    /// SHA-256 of the new source. One value. `package plan` repeats this flag, once per artifact.
    #[arg(long)]
    source_checksum: Option<String>,
    /// Locked dependency pin, `NAME=VERSION`. Repeatable. A name the recipe does not depend on stays out of the recipe.
    #[arg(long = "dep", value_name = "NAME=VERSION")]
    dependencies: Vec<String>,
    /// Easyconfig tree used as the robot. Repeatable; later trees win on one identity.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Toolchain hierarchy JSON, instead of the hierarchies this build knows.
    #[arg(long)]
    hierarchy_fixture: Option<PathBuf>,
    /// Stack policy TOML, or JSON when the path ends in `.json`. Absent, the toolchain is unconstrained.
    #[arg(long)]
    stack_policy: Option<PathBuf>,
    /// Fail when a patch's fate across the version bump cannot be decided
    /// from tree evidence or, with `--fetch-checksums`, from the new source,
    /// instead of carrying it with a review flag.
    #[arg(long)]
    strict_patches: bool,
    /// Package-config TOML applied after the source recipe is parsed. Repeatable.
    #[arg(long = "package-config")]
    package_configs: Vec<PathBuf>,
    /// Render an easyblock skeleton for the package into the bundle, as
    /// `package plan --emit-easyblock` does.
    #[arg(long = "emit-easyblock")]
    emit_easyblock: bool,
    /// On a version bump without `--source-checksum`, download the new
    /// source from the recipe's `source_urls` and write its SHA-256, as
    /// `eb --inject-checksums` does. Anything that cannot be fetched is left
    /// empty and reported in `sources.fetch.json` and as a residual. Patches
    /// the tree cannot decide are then tried against the unpacked source:
    /// carried when they apply, dropped when they reverse-apply or their
    /// files are gone.
    #[arg(long = "fetch-checksums")]
    fetch_checksums: bool,
    /// Directory for the manifest, planned SBOM, locks, and recipes.
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(Subcommand, Debug)]
enum RecipeCommand {
    /// Print the easyblock class name EasyBuild derives for a software name.
    ///
    /// An easyconfig that names no `easyblock` is claiming a software-specific
    /// one exists, because there is no default: EasyBuild derives this name,
    /// tries to import it, and stops when it cannot. So this answers whether a
    /// recipe is relying on something that has to be there.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe easyblock zlib\n  eb-stack recipe easyblock --charmap"
    )]
    Easyblock {
        /// Software name, spelled as the easyconfig's `name`.
        ///
        /// Hyphen-leading values are allowed: a software name is not a flag,
        /// and the encoding exists precisely because names carry punctuation.
        #[arg(required_unless_present = "charmap", allow_hyphen_values = true)]
        name: Option<String>,
        /// Emit the encoding table as JSON instead, for consumers that apply
        /// the substitution themselves rather than shelling out per name.
        #[arg(long)]
        charmap: bool,
    },
    /// Emit template constants, derived-key rules, or resolved values.
    ///
    /// `--export` prints the constant table and the derived-key rules.
    /// `--name` and `--version` together print the values those rules produce
    /// for that pair. A consumer that cannot run the parser still resolves
    /// `%(version)s` and friends by applying these rules, which is why the
    /// rule vocabulary is closed.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe templates --export\n  eb-stack recipe templates --name zlib --version 1.3.1"
    )]
    Templates {
        /// Software name, as the easyconfig spells `name`.
        #[arg(long, allow_hyphen_values = true)]
        name: Option<String>,
        /// Version, as the easyconfig spells `version`.
        #[arg(long)]
        version: Option<String>,
        /// Emit the spec rather than resolved values.
        #[arg(long)]
        export: bool,
    },
    /// Emit the known toolchain hierarchies as JSON, parent and members.
    ///
    /// Without `--export` this is the fixture generations. `--export` prints
    /// the whole table, including the synthetic GCC-family rule. `members`
    /// keeps the framework's order, most minimal subtoolchain first, so a
    /// consumer reads it rather than sorting it.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe hierarchy\n  eb-stack recipe hierarchy --export"
    )]
    Hierarchy {
        /// Emit the whole table, including the synthetic GCC-family rule.
        #[arg(long)]
        export: bool,
    },
    /// Resolve a recipe and verify package metadata plus robot dependencies.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe check --recipe recipe.eb --easyconfigs easyconfigs/ --metadata-only"
    )]
    Check {
        /// Easyconfig to check.
        #[arg(long)]
        recipe: PathBuf,
        /// Easyconfig tree used as the robot. Repeatable; later trees win. A `.eb` file stands for the directory it sits in.
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        /// Easyblock modules, as directories or files, to resolve the recipe's
        /// class against. A bundle's own `easyblocks/` is searched first
        /// without being named. Naming any root asserts the set is complete:
        /// a software-specific class found in none of them fails the check.
        #[arg(long = "easyblocks")]
        easyblock_roots: Vec<PathBuf>,
        /// A configopts flag the recipe must contain.
        ///
        /// Hyphen-leading values are allowed: the flag is written the way it
        /// appears in the recipe, dashes included, and is not itself an
        /// eb-stack option.
        #[arg(long = "require-configopt", allow_hyphen_values = true)]
        require_configopts: Vec<String>,
        /// Stop after metadata, checksums, and required config flags. Do not walk the robot.
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
        /// Report every statement that failed to parse, with its line, and
        /// fail if there are any. Control-flow blocks stay unreported: they
        /// are the tolerant skip, and listing them would bury the statements
        /// that actually failed. The parse itself is unchanged.
        #[arg(long)]
        strict: bool,
    },
    /// Report the upstream CI, test-suite, and review rules for easyconfigs, and merge
    /// `eb --check-contrib` when `eb` is installed. Fails on error-severity findings.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe lint recipe.eb\n  eb-stack recipe lint recipe.eb --base recipe.eb.orig"
    )]
    Lint {
        /// Easyconfig files to lint.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// The version the change started from; reports templates replaced by
        /// Python expressions (EB_FMT_TEMPLATE_REMOVED).
        #[arg(long)]
        base: Option<PathBuf>,
    },
    /// Mechanically format EasyBuild E501 findings.
    #[command(
        after_long_help = "Examples:\n  eb-stack recipe format recipe.eb\n  eb-stack recipe format recipe.eb --out formatted.eb\n  eb-stack recipe format recipe.eb --dry-run"
    )]
    Format {
        /// Easyconfig files to format. With `--out`, exactly one path.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Write the formatted recipe here instead of replacing the input. Requires one path.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Print what would change and do not write.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(clap::Args, Debug)]
struct SolveArgs {
    /// Easyconfig tree to solve. Repeatable; later trees win on one identity.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Stack policy TOML or JSON. The same file `package plan --stack-policy` reads.
    ///
    /// Omit it and pass `--toolchain` and `--root` instead.
    #[arg(
        long = "stack-policy",
        alias = "policy",
        value_name = "FILE",
        conflicts_with_all = ["toolchain", "roots"]
    )]
    policy: Option<PathBuf>,
    /// Target toolchain as `name/version` (`foss/2026.1`) or `system`.
    ///
    /// `system` and `SYSTEM` select the SYSTEM toolchain. Used with --root
    /// in place of a policy file.
    #[arg(long)]
    toolchain: Option<String>,
    /// Application the stack exists to provide. Repeatable. Used with
    /// --toolchain in place of a policy file.
    #[arg(long = "root", requires = "toolchain")]
    roots: Vec<String>,
    /// Take roots from a Jenkins build list as well: the package name of
    /// every non-comment line. Repeatable. Used with --toolchain.
    #[arg(long = "roots-from-build-list", requires = "toolchain")]
    roots_from_build_list: Vec<PathBuf>,
    /// Previous-generation easyconfig tree. Absent, the first `--easyconfigs` tree is the baseline.
    #[arg(long)]
    baseline_easyconfigs: Option<PathBuf>,
    /// Toolchain version to treat as the baseline inside that tree. Absent, the newest generation below the target.
    #[arg(long)]
    baseline_toolchain_version: Option<String>,
    /// Compare against what these easystacks install instead of a previous
    /// generation: each entry and its pinned closure, read from the
    /// --easyconfigs trees. Repeatable; pass every file of a software
    /// layer version so `Unchanged` in the stack diff means "provided".
    #[arg(long = "baseline-easystack", conflicts_with_all = ["baseline_easyconfigs", "baseline_toolchain_version"])]
    baseline_easystacks: Vec<PathBuf>,
    /// easybuild-easyconfigs checkout to read `from-commit` entries of the
    /// baseline easystacks from. Without it they match by filename.
    #[arg(long, requires = "baseline_easystacks")]
    baseline_commits_from: Option<PathBuf>,
    /// Dependency names to drop from every recipe, as EasyBuild's
    /// --filter-deps (comma-separated). EESSI-extend sets this list.
    #[arg(long, value_delimiter = ',')]
    filter_deps: Vec<String>,
    /// Another toolchain family to keep in the easystack baseline, as
    /// `name/version` (e.g. `lfoss/2026.1`). Repeatable.
    #[arg(long = "baseline-also-toolchain", requires = "baseline_easystacks")]
    baseline_also_toolchains: Vec<String>,
    /// Lock file to write. A single file, so this is `--lock-out` rather than `--out-dir`.
    #[arg(long, default_value = "stack.lock.json")]
    lock_out: PathBuf,
    /// CycloneDX file to write beside the lock. Absent, no SBOM is written.
    #[arg(long)]
    sbom_out: Option<PathBuf>,
    /// Build list to write, one module per line, in dependency order.
    #[arg(long)]
    build_list_out: Option<PathBuf>,
    /// Leave out of the build list every module the baseline provides
    /// (same name, version, toolchain and versionsuffix). With an easystack
    /// baseline this is the site layer.
    #[arg(long)]
    build_list_excludes_baseline: bool,
    /// Also write the build list in the Jenkins build-list format:
    /// filenames with per-line `eb` flags.
    #[arg(long)]
    jenkins_build_list_out: Option<PathBuf>,
    /// Previous Jenkins build list whose per-package site flags (hooks,
    /// site easyblocks, EULA acceptances) carry over. Repeatable.
    #[arg(long = "jenkins-flags-from", requires = "jenkins_build_list_out")]
    jenkins_flags_from: Vec<PathBuf>,
    /// Systems CUDA modules are restricted to, comma-separated, written as
    /// `--include-systems` on their lines.
    #[arg(long, requires = "jenkins_build_list_out", value_delimiter = ',')]
    jenkins_gpu_systems: Vec<String>,
    /// Write only the roots to the Jenkins list, one line per application.
    /// The pipeline's `eb --robot` builds their dependencies.
    #[arg(long, requires = "jenkins_build_list_out")]
    jenkins_roots_only: bool,
    /// Markdown diff of this solve against the baseline.
    #[arg(long)]
    stack_diff_out: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct BatchArgs {
    /// Root easyconfig file name. Repeatable.
    #[arg(long = "root")]
    roots: Vec<String>,
    /// File with one root easyconfig file name per line.
    #[arg(long)]
    roots_file: Option<PathBuf>,
    /// Robot-path easyconfig tree. Repeatable; later trees win.
    #[arg(long, required = true)]
    easyconfigs: Vec<PathBuf>,
    /// Commit and directory for an open pull request's from-commit pin,
    /// as `COMMIT=DIR`. Repeatable.
    #[arg(long = "commit-tree")]
    commit_trees: Vec<String>,
    /// Installed modules, one `Name/VersionFull` per line.
    #[arg(long)]
    installed: PathBuf,
    /// Modulerc file with `module_version` aliases. Repeatable.
    #[arg(long = "installed-modulerc")]
    installed_modulerc: Vec<PathBuf>,
    /// Open software-layer pull requests, `pr-overlap --pr` JSON. Repeatable.
    #[arg(long = "open-prs")]
    open_prs: Vec<PathBuf>,
    /// Also read the open pull requests of `OWNER/NAME` over HTTPS, as
    /// `EESSI/software-layer`. `GITHUB_TOKEN` or `GH_TOKEN` is sent to
    /// api.github.com when set.
    #[arg(long)]
    open_repo: Option<String>,
    /// EESSI version, `2026.06`.
    #[arg(long)]
    eessi_version: String,
    /// EasyBuild release the entries are built with, `5.4.1`.
    #[arg(long)]
    easybuild_version: String,
    /// Toolchain generation, `2026.1`.
    #[arg(long)]
    generation: String,
    /// `--filter-deps` as EESSI-extend sets it. Absent, `EASYBUILD_FILTER_DEPS` is read.
    #[arg(long, value_delimiter = ',')]
    filter_deps: Vec<String>,
    /// Most roots per pull request.
    #[arg(long, default_value_t = 3)]
    size: usize,
    /// Architecture to judge the plan against, `riscv64` or
    /// `aarch64/generic`. Repeatable. Absent checks every architecture
    /// in the unsupported list, because a shared easystack is built on
    /// each of them.
    #[arg(long = "arch")]
    arch: Vec<String>,
    /// Replace the bundled per-architecture unsupported list.
    #[arg(long)]
    unsupported: Option<PathBuf>,
    /// easybuild-easyconfigs checkout with release tags: a root the
    /// release lacks gets a `from-commit` pin.
    #[arg(long)]
    easyconfigs_repo: Option<PathBuf>,
    /// RFC 3339 time written into the plan. Absent is now.
    #[arg(long)]
    generated_at: Option<String>,
    /// Output directory.
    #[arg(long)]
    out_dir: PathBuf,
}

#[derive(Subcommand, Debug)]
enum StackCommand {
    /// Parse EasyBuild trees and solve a jointly consistent stack.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack solve --easyconfigs easyconfigs/ --toolchain foss/2026.1 --root zlib --lock-out stack.lock.json\n  eb-stack stack solve --easyconfigs easyconfigs/ --stack-policy stack.toml"
    )]
    Solve(Box<SolveArgs>),
    /// Emit CycloneDX from an existing stack lock.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack sbom --lock stack.lock.json --out stack.cdx.json"
    )]
    Sbom {
        /// Stack lock JSON written by `stack solve`.
        #[arg(long)]
        lock: PathBuf,
        /// CycloneDX file to write. A single file, so this is `--out` rather than `--out-dir`.
        #[arg(long, default_value = "stack.cdx.json")]
        out: PathBuf,
    },
    /// Order the builds a set of roots needs, as a graph rather than a stack.
    ///
    /// Unlike `solve`, this does not pick one version per package: it takes
    /// what the recipes pin and sequences them, so a generation that carries
    /// two builds of one name gets both, in an order that respects every
    /// dependency.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack order --easyconfigs easyconfigs/ --root 'zlib==1.3.1'"
    )]
    Order {
        /// Easyconfig tree the roots are closed over. Repeatable; later trees win.
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        /// Package to build, optionally pinned as `name==version`. Repeatable.
        #[arg(long = "root", required = true)]
        roots: Vec<String>,
        /// Build-order file to write, one module per line.
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
        /// Dependency names to drop from every recipe, as EasyBuild's
        /// --filter-deps (comma-separated). EESSI-extend sets this list.
        /// Absent, `EASYBUILD_FILTER_DEPS` is read, the same way `stack solve`
        /// reads it.
        #[arg(long, value_delimiter = ',')]
        filter_deps: Vec<String>,
    },
    /// What a site provides that a stack lacks, and the recipe that closes it.
    ///
    /// Every package in the `--site` module listings is `in-stack` (the
    /// stack's `--installed` list has it, matched without regard to case),
    /// `recipe` (an easyconfig on the generation's toolchains or the system
    /// toolchain exists; the newest is named), `no-recipe` (the newest
    /// easyconfig on any toolchain is named as the bump source), or
    /// `excluded`. Writes `gap.tsv`, `roots.txt` for `stack batch
    /// --roots-file`, and `bump-sources.txt`.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack gap --site snellius-2025.txt --site snellius-2024.txt --installed eessi-2026.06-zen4.txt --easyconfigs easybuild-easyconfigs/easybuild/easyconfigs --generation 2026.1 --exclude-file site-only.tsv --out-dir gap"
    )]
    Gap {
        /// Site module listing, `Name/Version` per line or a bare name. Repeatable.
        #[arg(long = "site", required = true)]
        site: Vec<PathBuf>,
        /// The stack's installed modules.
        #[arg(long)]
        installed: PathBuf,
        /// Easyconfig tree to find recipes in. Repeatable.
        #[arg(long, required = true)]
        easyconfigs: Vec<PathBuf>,
        /// Toolchain generation, `2026.1`.
        #[arg(long)]
        generation: String,
        /// `name<TAB>reason` lines for names the stack does not carry
        /// (licensed, vendor, site portal).
        #[arg(long)]
        exclude_file: Option<PathBuf>,
        /// Names filtered to the compatibility layer. Absent, `EASYBUILD_FILTER_DEPS` is read.
        #[arg(long, value_delimiter = ',')]
        filter_deps: Vec<String>,
        /// Output directory.
        #[arg(long)]
        out_dir: PathBuf,
    },
    /// Plan software-layer pull requests whose builds do not overlap.
    ///
    /// Each root's new-module set is its closure over the robot path minus the
    /// installed modules, as `pr-overlap` computes it for one easystack line.
    /// Roots whose sets meet ship in one pull request; a root whose set meets
    /// an open pull request's waits for it. The rest are packed `--size` per
    /// pull request. Each batch directory gets the EESSI easystacks, a lock, a
    /// CycloneDX SBOM, `reproduce.sh` with this exact command and the
    /// `eb --dry-run` lines, `batch.json`, and a pull-request body.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack batch --roots-file roots.txt --easyconfigs easybuild-easyconfigs/easybuild/easyconfigs --installed installed-zen4.txt --open-prs open-prs.json --eessi-version 2026.06 --easybuild-version 5.4.1 --generation 2026.1 --easyconfigs-repo easybuild-easyconfigs --out-dir batches"
    )]
    /// Plan software-layer pull requests whose builds do not overlap.
    ///
    /// Each root's new-module set is its closure over the robot path minus the
    /// installed modules, as `pr-overlap` computes it for one easystack line.
    /// Roots whose sets meet ship in one pull request; a root whose set meets
    /// an open pull request's waits for it. The rest are packed `--size` per
    /// pull request. Each batch directory gets the EESSI easystacks, a lock, a
    /// CycloneDX SBOM, `reproduce.sh` with this exact command and the
    /// `eb --dry-run` lines, `batch.json`, and a pull-request body.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack batch --roots-file roots.txt --easyconfigs easybuild-easyconfigs/easybuild/easyconfigs --installed installed-zen4.txt --open-prs open-prs.json --eessi-version 2026.06 --easybuild-version 5.4.1 --generation 2026.1 --easyconfigs-repo easybuild-easyconfigs --out-dir batches"
    )]
    Batch(Box<BatchArgs>),
    /// Refuse a plan an architecture cannot build, and flag shared entries
    /// that architecture has never installed.
    ///
    /// An entry in `data/arch-unsupported.toml` fails the plan when the
    /// easystack names it or a dependency closes over it. The message names
    /// the easyconfig and the architecture. With `--installed`, an entry the
    /// architecture has never installed, and that is not in `--additional-arch`,
    /// is flagged.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack arch --arch riscv64 --easystack eessi-2026.06-eb-5.4.0-2026.1.yml --easyconfigs easybuild-easyconfigs/easybuild/easyconfigs\n  eb-stack stack arch --arch riscv64 --easystack eessi-2026.06-eb-5.4.0-2026.1.yml --installed installed-riscv64.txt --additional-arch additional_arch/riscv64"
    )]
    Arch {
        /// Architecture family, `riscv64`, or a target such as
        /// `riscv64/generic`. Repeatable. Absent checks every architecture
        /// in the unsupported list.
        #[arg(long = "arch")]
        arch: Vec<String>,
        /// Easystack whose entries are the plan. Repeatable.
        #[arg(long, required = true)]
        easystack: Vec<PathBuf>,
        /// Robot tree the dependency closure is read from. Repeatable.
        /// Absent checks only the filenames the easystack names.
        #[arg(long)]
        easyconfigs: Vec<PathBuf>,
        /// Installed modules for the cheap check, one `Name/VersionFull` per
        /// line. A bare path applies to the sole `--arch`. `riscv64=path`
        /// binds the file to that architecture. Repeatable.
        #[arg(long = "installed")]
        installed: Vec<String>,
        /// That architecture's `additional_arch` easystack, or a directory of
        /// them. A bare path applies to the sole `--arch`. `riscv64=path`
        /// binds it. Repeatable.
        #[arg(long = "additional-arch")]
        additional_arch: Vec<String>,
        /// Replace the bundled unsupported list.
        #[arg(long)]
        unsupported: Option<PathBuf>,
        /// Probe source URLs for x86_64, aarch64, and riscv64 before build.
        ///
        /// This is opt-in because it performs network requests (HEAD plus
        /// ranged GET fallback) against every resolved source URL in the plan.
        #[arg(long = "probe-sources")]
        probe_sources: bool,
        /// Extra attempts after the first source probe request.
        #[arg(
            long = "probe-retries",
            default_value_t = 2,
            requires = "probe_sources"
        )]
        probe_retries: u32,
        /// Timeout in seconds for one source probe request.
        #[arg(
            long = "probe-timeout-s",
            default_value_t = 8,
            requires = "probe_sources"
        )]
        probe_timeout_s: u64,
    },
    /// Find the patch and source files an easystack's easyconfigs need
    /// before the bot looks for them.
    ///
    /// Patches of the recipe, its extensions and its components, sources no
    /// URL downloads, and patch files named only in `checksums` are looked
    /// up the way EasyBuild does: beside the easyconfig, then under
    /// `<letter>/<name>/` on the robot path. A `from-commit` pin puts that
    /// commit's tree on the robot path. A patch or easyconfig found nowhere
    /// fails the command, and the line names the easyconfig, the file, and
    /// where it was looked for. A source with no download URL is printed as
    /// a `no_url` line and does not fail it.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack files --easystack easystacks/software.eessi.io/2023.06/rebuilds/20261022-eb-5.4.1-2022b-emSign-cert.yml --easyconfigs easybuild-easyconfigs-5.4.1/easybuild/easyconfigs --easyconfigs-repo easybuild-easyconfigs\n  eb-stack stack files --easystack eessi-2025.06-eb-5.2.0-2025b.yml --easyconfigs easybuild-easyconfigs/easybuild/easyconfigs --commit-tree 7e74f85c3268=pr-25365/easybuild/easyconfigs"
    )]
    Files {
        /// Easystack whose entries are checked. Repeatable.
        #[arg(long, required = true)]
        easystack: Vec<PathBuf>,
        /// Robot-path easyconfig tree of the EasyBuild release the bot runs.
        /// Repeatable.
        #[arg(long)]
        easyconfigs: Vec<PathBuf>,
        /// Easyconfigs for one `from-commit` pin, as `<commit>=<dir>`.
        /// Repeatable.
        #[arg(long = "commit-tree")]
        commit_trees: Vec<String>,
        /// easybuild-easyconfigs git checkout a `from-commit` pin is read
        /// from with `git ls-tree` and `git show`, when no `--commit-tree`
        /// names it. The commit has to be fetched.
        #[arg(long)]
        easyconfigs_repo: Option<PathBuf>,
        /// Source directory EasyBuild also searches. Repeatable.
        #[arg(long)]
        sourcepath: Vec<PathBuf>,
        /// Read an easystack path that starts with `FROM` from `TO`, as
        /// `FROM=TO`. Repeatable.
        #[arg(long = "path-map")]
        path_map: Vec<String>,
    },
    /// Write the lock as an EasyBuild easystack, the format `eb --easystack`
    /// consumes and EESSI's software layer is built from.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack easystack --lock stack.lock.json --out stack.yml\n  eb-stack stack easystack --lock stack.lock.json --eessi-version 2026.06 --easybuild-version 5.4.1 --generation 2026.1 --out-dir software-layer"
    )]
    Easystack {
        /// Stack lock JSON written by `stack solve`.
        #[arg(long)]
        lock: PathBuf,
        /// Easystack YAML to write.
        #[arg(long, default_value = "stack.yml")]
        out: PathBuf,
        /// Per-easyconfig `eb` option, as `<file.eb>:<option>=<value>`, with
        /// the option spelled as on the command line without its dashes:
        /// `--option CUDA-12.8.0.eb:accept-eula-for=CUDA`. Repeatable.
        #[arg(long = "option")]
        options: Vec<String>,
        /// Write EESSI software-layer easystacks for this EESSI version
        /// instead of one `--out` file: one file per EasyBuild release and
        /// generation, `001-system` for toolchain-free recipes, `accel/nvidia/`
        /// for CUDA builds. Needs `--easybuild-version`, `--generation`, `--out-dir`.
        #[arg(long, requires_all = ["easybuild_version", "generation", "out_dir"])]
        eessi_version: Option<String>,
        /// EasyBuild release the EESSI entries are built with, `5.4.1`.
        #[arg(long, requires = "eessi_version")]
        easybuild_version: Option<String>,
        /// Toolchain generation of the entries, `2026.1`.
        #[arg(long, requires = "eessi_version")]
        generation: Option<String>,
        /// Software-layer root to write the EESSI easystacks under.
        #[arg(long, requires = "eessi_version")]
        out_dir: Option<PathBuf>,
    },
    /// Regroup a Jenkins build list under the section headers of a previous
    /// one, so each package sits in the category the site already files it
    /// under. Entries keep their flags and whether they are commented out.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack sections next.list --from previous.list --out grouped.list"
    )]
    Sections {
        /// The build list to regroup.
        list: PathBuf,
        /// Previous build list whose `#### Title ####` headers, and the
        /// packages under them, decide where each entry goes.
        #[arg(long = "from", required = true)]
        from: PathBuf,
        /// Header for entries no section of the previous list names.
        #[arg(long, default_value = "New in this generation")]
        unfiled: String,
        /// Grouped build list to write.
        #[arg(long)]
        out: PathBuf,
    },
    /// Report from-commit pins already contained in an EasyBuild release.
    ///
    /// A `from-commit` pin is checked in `--easyconfigs-repo` against the tag
    /// `easybuild-easyconfigs-vX.Y.Z`. An `include-easyblocks-from-commit` pin
    /// is checked in `--easyblocks-repo` against `easybuild-easyblocks-vX.Y.Z`.
    /// The release is `--easybuild-version`, or the `eb-X.Y.Z` in the easystack
    /// file name when that flag is omitted. The flag wins when both are set.
    /// Exits 1 when any pin is `released`. With `--rewrite`, the pins whose
    /// released file is the same are dropped from the easystacks in place,
    /// a pin whose released file differs is kept, and the exit status is 0.
    #[command(
        after_long_help = "Examples:\n  eb-stack stack from-commit --easystack 20261011-eb-5.4.0-UCX.yml --easyconfigs-repo ~/src/easybuild-easyconfigs\n  eb-stack stack from-commit --easystack rebuild.yml --easybuild-version 5.4.0 --easyconfigs-repo ~/src/easybuild-easyconfigs --json\n  eb-stack stack from-commit --easystack rebuild.yml --easyconfigs-repo ~/src/easybuild-easyconfigs --rewrite"
    )]
    FromCommit {
        /// Easystack YAML to read. Repeat for more than one file.
        #[arg(long = "easystack", required = true)]
        easystacks: Vec<PathBuf>,
        /// easybuild-easyconfigs checkout. Required when an entry sets `from-commit`.
        #[arg(long)]
        easyconfigs_repo: Option<PathBuf>,
        /// easybuild-easyblocks checkout. Required when an entry sets
        /// `include-easyblocks-from-commit`.
        #[arg(long)]
        easyblocks_repo: Option<PathBuf>,
        /// EasyBuild release `X.Y.Z`. Overrides `eb-X.Y.Z` in the easystack file name.
        #[arg(long)]
        easybuild_version: Option<String>,
        /// Print the report as JSON.
        #[arg(long)]
        json: bool,
        /// Drop each released pin whose file is the same in the release,
        /// editing the easystacks in place. A pin whose released file differs
        /// is kept.
        #[arg(long)]
        rewrite: bool,
    },
}

#[derive(Subcommand, Debug)]
enum TargetCommand {
    /// List targets from layered public TOML configuration.
    #[command(
        after_long_help = "Examples:\n  eb-stack target list --config base.toml --config site.toml"
    )]
    List {
        /// Target-config TOML. Repeatable. Later files override earlier ones.
        #[arg(long = "config", required = true)]
        configs: Vec<PathBuf>,
    },
    /// Validate transport, executor, runtime, and EasyBuild workload routing.
    #[command(
        after_long_help = "Examples:\n  eb-stack target doctor --config target.toml --target local"
    )]
    Doctor {
        /// Target-config TOML. Repeatable. Later files override earlier ones.
        #[arg(long = "config", required = true)]
        configs: Vec<PathBuf>,
        /// Target name to probe, as declared in the config.
        #[arg(long)]
        target: String,
    },
}

#[derive(Subcommand, Debug)]
enum CampaignCommand {
    /// Start or resume a persisted package build campaign.
    #[command(
        after_long_help = "Examples:\n  eb-stack campaign run --bundle bundle/ --config target.toml --target local --state campaign.json"
    )]
    Run {
        /// Package bundle directory written by `package plan` or `package bump`.
        #[arg(long)]
        bundle: PathBuf,
        /// Target-config TOML. Repeatable. Later files override earlier ones.
        #[arg(long = "config", required = true)]
        configs: Vec<PathBuf>,
        /// Target name to build on, as declared in the config.
        #[arg(long)]
        target: String,
        /// Campaign state JSON. Created on the first run and resumed after that.
        #[arg(long)]
        state: PathBuf,
    },
    /// Print persisted campaign state and claim ladder.
    #[command(after_long_help = "Examples:\n  eb-stack campaign status --state campaign.json")]
    Status {
        /// Campaign state JSON written by `campaign run`.
        #[arg(long)]
        state: PathBuf,
    },
    /// Coordinate typed finding repair across campaign workers.
    #[command(
        after_long_help = "Examples:\n  eb-stack campaign finding claim --state campaign.json --id FINDING --owner worker-a"
    )]
    Finding {
        #[command(subcommand)]
        command: CampaignFindingCommand,
    },
}

#[derive(Subcommand, Debug)]
enum CampaignFindingCommand {
    /// Claim an open finding for one worker.
    #[command(
        after_long_help = "Examples:\n  eb-stack campaign finding claim --state campaign.json --id FINDING --owner worker-a"
    )]
    Claim {
        /// Campaign state JSON written by `campaign run`.
        #[arg(long)]
        state: PathBuf,
        /// Finding identifier, as stored in the state file.
        #[arg(long)]
        id: String,
        /// Worker name that will own the claim.
        #[arg(long)]
        owner: String,
    },
    /// Resolve a claimed finding with durable evidence.
    #[command(
        after_long_help = "Examples:\n  eb-stack campaign finding resolve --state campaign.json --id FINDING --owner worker-a --action 'pinned the dep' --evidence 'lock updated' --change recipe.eb"
    )]
    Resolve {
        /// Campaign state JSON written by `campaign run`.
        #[arg(long)]
        state: PathBuf,
        /// Finding identifier, as stored in the state file.
        #[arg(long)]
        id: String,
        /// Worker name that owns the claim.
        #[arg(long)]
        owner: String,
        /// What was done to the recipe or the target.
        #[arg(long)]
        action: String,
        /// Why that action is enough to close the finding.
        #[arg(long)]
        evidence: String,
        /// File the action changed. Repeatable.
        #[arg(long = "change")]
        changes: Vec<String>,
    },
}

fn main() {
    // A rejected command line exits in clap, before any compatibility
    // warning. The warning is only for a spelling this subcommand accepts.
    let matches = <Cli as clap::CommandFactory>::command().get_matches();
    warn_retired_spellings(&matches);
    if let Err(error) = dispatch() {
        // Display, not Debug: Debug appends a stack backtrace, which is a
        // compiler trace and not the reason the command failed.
        eprintln!("error: {error:#}");
        // 2 is a rejected command line. 1 is work that was attempted and failed.
        let code = if is_usage(&error) { 2 } else { 1 };
        std::process::exit(code);
    }
}

/// Warn when a hidden compatibility spelling was the one clap accepted.
fn warn_retired_spellings(matches: &clap::ArgMatches) {
    if let Some(("package", package)) = matches.subcommand() {
        if let Some((name, args)) = package.subcommand() {
            if (name == "inspect" || name == "plan" || name == "bump")
                && split_toolchain_resolves(args, name != "bump")
            {
                warn_toolchain_flag(args, "toolchain_name", "--toolchain-name");
                warn_toolchain_flag(args, "toolchain_version", "--toolchain-version");
            }
        }
    }
    if let Some(("stack", stack)) = matches.subcommand() {
        if stack.subcommand().is_some_and(|(name, _)| name == "solve") {
            warn_policy_alias();
        }
    }
}

/// A split pair the command will reject is not accepted, so it is not warned.
fn split_toolchain_resolves(args: &clap::ArgMatches, default_name: bool) -> bool {
    let name = args.get_one::<String>("toolchain_name").map(String::as_str);
    let version = args
        .get_one::<String>("toolchain_version")
        .map(String::as_str);
    eb_stack::cli_parse::resolve_toolchain(name, version, None, default_name, "--toolchain").is_ok()
}

fn warn_toolchain_flag(args: &clap::ArgMatches, id: &str, flag: &str) {
    if args.value_source(id) == Some(clap::parser::ValueSource::CommandLine) {
        eprintln!("warning: {flag} is accepted for compatibility. Use --toolchain name/version.");
    }
}

fn warn_policy_alias() {
    let mut seen_stack = false;
    let mut seen_solve = false;
    for argument in std::env::args().skip(1) {
        if argument == "--" {
            break;
        }
        if !seen_stack {
            if argument == "stack" {
                seen_stack = true;
            }
            continue;
        }
        if !seen_solve {
            if argument == "solve" {
                seen_solve = true;
            }
            continue;
        }
        let flag = argument
            .split_once('=')
            .map_or(argument.as_str(), |(name, _)| name);
        if flag == "--policy" {
            eprintln!("warning: --policy is accepted for compatibility. Use --stack-policy.");
        }
    }
}

fn dispatch() -> Result<()> {
    match Cli::parse().command {
        Command::Package { command } => run_package(*command),
        Command::Recipe { command } => run_recipe(command),
        Command::Stack { command } => run_stack(*command),
        Command::Target { command } => run_target(command),
        Command::Campaign { command } => run_campaign(command),
        Command::Repro { command } => run_repro(command),
        Command::Mcp { workspace } => {
            if let Some(workspace) = workspace {
                std::env::set_var("EB_STACK_WORKSPACE", &workspace);
            }
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            eb_stack::mcp::run_server(stdin.lock(), stdout.lock()).context("MCP stdio server")?;
            Ok(())
        }
        Command::EasyblockReach(args) => run_easyblock_reach(*args),
        Command::PrOverlap(args) => run_pr_overlap(*args),
        Command::RebuildSet(args) => run_rebuild_set(*args),
        Command::ComponentReach(args) => run_component_reach(*args),
    }
}

#[derive(clap::Args, Debug)]
struct EasyblockReachArgs {
    /// Easyblock trees. Earlier roots win a class name.
    #[arg(long)]
    easyblocks: Vec<PathBuf>,
    /// Framework trees, indexed the same way as easyblocks.
    #[arg(long)]
    framework: Vec<PathBuf>,
    /// Easyconfig files or trees.
    #[arg(long)]
    easyconfigs: Vec<PathBuf>,
    /// A changed Python file. Repeat for each file.
    #[arg(long)]
    file: Vec<PathBuf>,
    /// Commit range passed to `git diff --name-only`, for example `base..HEAD`.
    #[arg(long)]
    range: Option<String>,
    /// Pull request number. Fetched as `refs/pull/N/head`.
    #[arg(long)]
    pr: Option<u64>,
    /// Remote to fetch the pull request from.
    #[arg(long, default_value = "origin")]
    remote: String,
    /// Base branch for the pull request merge-base.
    #[arg(long, default_value = "develop")]
    base: String,
    /// Clone `git` reads. Defaults to the first `--easyblocks` path.
    #[arg(long)]
    repo: Option<PathBuf>,
    /// Installed module names, one per line.
    #[arg(long)]
    installed: Option<PathBuf>,
    /// Also list modules that depend on a reached module.
    #[arg(long)]
    reverse: bool,
    /// RFC 3339 timestamp stored in the report. Defaults to now.
    #[arg(long)]
    generated_at: Option<String>,
    /// Directory for `reach.toml`, `reach.json`, and `reach.md`.
    #[arg(long, default_value = "easyblock-reach")]
    out_dir: PathBuf,
}

fn run_easyblock_reach(args: EasyblockReachArgs) -> Result<()> {
    let report = eb_stack::site_commands::run_easyblock_reach(
        &eb_stack::site_commands::EasyblockReachRequest {
            easyblocks: args.easyblocks,
            framework: args.framework,
            easyconfigs: args.easyconfigs,
            file: args.file,
            range: args.range,
            pr: args.pr,
            remote: args.remote,
            base: args.base,
            repo: args.repo,
            installed: args.installed,
            reverse: args.reverse,
            generated_at: args.generated_at,
            out_dir: args.out_dir,
        },
    )?;
    println!("{}", report.summary());
    Ok(())
}

fn run_pr_overlap(args: PrOverlapArgs) -> Result<()> {
    let report =
        match eb_stack::site_commands::run_pr_overlap(&eb_stack::site_commands::PrOverlapRequest {
            prs: args.prs,
            kind: match args.kind {
                OverlapKind::SoftwareLayer => eb_stack::site_commands::OverlapKind::SoftwareLayer,
                OverlapKind::Easyconfigs => eb_stack::site_commands::OverlapKind::Easyconfigs,
            },
            easyconfigs: args.easyconfigs,
            commit_trees: args.commit_trees,
            installed: args.installed,
            eessi_version: args.eessi_version,
            installed_label: args.installed_label,
            exclude_paths: args.exclude_paths,
            include_paths: args.include_paths,
            filter_deps: args.filter_deps,
            installed_modulerc: args.installed_modulerc,
            easyblocks: args.easyblocks,
            easyblocks_commits: args.easyblocks_commits,
            generated_at: args.generated_at,
            merge_order: args.merge_order,
            merged: args.merged,
            impact: args.impact,
            repo_url: args.repo_url,
            out_dir: args.out_dir,
            open_repo: args.open,
            base_ref: args.base_ref,
            bot_build_set: args.bot_build_set,
            arch_installed: args.arch_installed,
        }) {
            Ok(report) => report,
            Err(eb_stack::site_commands::PrOverlapError::InstalledRequired) => {
                eprint!("{INSTALLED_REQUIRED}");
                std::process::exit(2);
            }
            Err(eb_stack::site_commands::PrOverlapError::Failed(error)) => return Err(error),
        };
    println!("{}", report.summary());
    Ok(())
}

fn run_rebuild_set(args: RebuildSetArgs) -> Result<()> {
    let report =
        eb_stack::site_commands::run_rebuild_set(&eb_stack::site_commands::RebuildSetRequest {
            changed: args.changed,
            easystacks: args.easystacks,
            findings: args.findings,
            easyconfigs: args.easyconfigs,
            commit_trees: args.commit_trees,
            installed: args.installed,
            installed_label: args.installed_label,
            filter_deps: args.filter_deps,
            installed_modulerc: args.installed_modulerc,
            easyblocks: args.easyblocks,
            easyblocks_commits: args.easyblocks_commits,
            generated_at: args.generated_at,
            prefix: args.prefix,
            check_static: args.check_static,
            out_dir: args.out_dir,
        })?;
    println!("{}", report.summary());
    Ok(())
}

fn run_component_reach(args: ComponentReachArgs) -> Result<()> {
    let report = eb_stack::site_commands::run_component_reach(
        &eb_stack::site_commands::ComponentReachRequest {
            component: args.component,
            version: args.version,
            purl: args.purl,
            cve: args.cve,
            generation: args.generation,
            easyconfigs: args.easyconfigs,
            installed: args.installed,
            easystacks: args.easystacks,
            easystack_urls: args.easystack_urls,
            compat_layer: args.compat_layer,
            filter_deps: args.filter_deps,
            generated_at: args.generated_at,
            out_dir: args.out_dir,
            osv_cache: args.osv_cache,
        },
    )?;
    println!("{}", report.summary());
    Ok(())
}

fn run_package(command: PackageCommand) -> Result<()> {
    match command {
        PackageCommand::Inspect(args) => {
            let toolchain = resolve_toolchain(
                args.toolchain_name.as_deref(),
                args.toolchain_version.as_deref(),
                args.toolchain_spec.as_deref(),
                true,
            )?;
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
            let toolchain = resolve_toolchain(
                args.inspect.toolchain_name.as_deref(),
                args.inspect.toolchain_version.as_deref(),
                args.inspect.toolchain_spec.as_deref(),
                true,
            )?;
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
            if use_closure && args.emit_easyblock {
                return Err(usage(
                    "--emit-easyblock does not combine with package-closure planning",
                ));
            }
            let options = PackageEmitOptions {
                easyblock_skeleton_root: args.emit_easyblock.then(|| args.inspect.out_dir.clone()),
            };
            if !use_closure {
                let bundle = plan_new_package_with(&request, &options)?;
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
                for path in written.easyblocks {
                    println!("easyblock={}", path.display());
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
            for path in &written.root.easyblocks {
                println!("easyblock={}", path.display());
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
                for path in &companion.easyblocks {
                    println!("easyblock={}", path.display());
                }
            }
            Ok(())
        }
        PackageCommand::Bump(args) => run_package_bump(args),
        PackageCommand::Latest(args) => run_package_latest(args),
    }
}

fn run_package_latest(args: PackageLatestArgs) -> Result<()> {
    let overrides = eb_stack::upstream::load_overrides(&args.overrides)?;
    let mut reports = Vec::new();
    for source in &args.sources {
        let ec = eb_stack::eb_parse::resolve_easyconfig_file(source)
            .with_context(|| format!("--source {}", source.display()))?;
        reports.push(eb_stack::upstream::latest(&ec, &overrides, &UreqClient)?);
    }
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for report in &reports {
            print!("{}", eb_stack::upstream::render_text(report));
        }
    }
    Ok(())
}

fn run_package_bump(args: PackageBumpArgs) -> Result<()> {
    let toolchain = resolve_toolchain(
        args.toolchain_name.as_deref(),
        args.toolchain_version.as_deref(),
        args.toolchain_spec.as_deref(),
        false,
    )?;
    // Usage is exit 2. A missing recipe is exit 1, so a bad `--dep` is
    // reported before the source file is read.
    let overrides = parse_dep_overrides(&args.dependencies)?;
    std::fs::metadata(&args.source)
        .with_context(|| format!("--source {}", args.source.display()))?;
    let stack_policy = if let Some(path) = args.stack_policy.as_deref() {
        load_stack_policy(path)?
    } else {
        unconstrained_stack_policy(&toolchain)
    };
    let options = PackageEmitOptions {
        easyblock_skeleton_root: args.emit_easyblock.then(|| args.out_dir.clone()),
    };
    let version = if args.latest {
        let ec = eb_stack::eb_parse::resolve_easyconfig_file(&args.source)
            .with_context(|| format!("--source {}", args.source.display()))?;
        let overrides = eb_stack::upstream::load_overrides(&args.overrides)?;
        let report = eb_stack::upstream::latest(&ec, &overrides, &UreqClient)?;
        print!("{}", eb_stack::upstream::render_text(&report));
        eb_stack::upstream::bump_version(&report)?
    } else {
        args.version
    };
    let request = BumpPackageRequest {
        source: args.source,
        toolchain,
        version,
        source_checksum: args.source_checksum,
        easyconfig_roots: args.easyconfigs,
        hierarchy_fixture: args.hierarchy_fixture,
        overrides,
        stack_policy,
        strict_patches: args.strict_patches,
        package_layers: load_package_layers(&args.package_configs)?,
    };
    let (bundle, fetched) = if args.fetch_checksums {
        let decisions = eb_stack::source_fetch::BumpDecisions {
            drop_patches: parse_patch_drops(&args.drop_patches)?,
            latest_secondary: args.latest_secondary,
            overrides: if args.latest_secondary {
                eb_stack::upstream::load_overrides(&args.overrides)?
            } else {
                Default::default()
            },
        };
        eb_stack::source_fetch::plan_package_bump_fetching_with(
            &request,
            &options,
            &UreqClient,
            &decisions,
        )?
    } else {
        (plan_package_bump_with(&request, &options)?, Vec::new())
    };
    let written = write_package_bundle(&bundle, &args.out_dir)?;
    if !fetched.is_empty() {
        let path = args.out_dir.join("sources.fetch.json");
        std::fs::write(&path, serde_json::to_string_pretty(&fetched)?)
            .with_context(|| format!("write {}", path.display()))?;
        println!("fetch={}", path.display());
    }
    println!("manifest={}", written.manifest.display());
    println!("sbom={}", written.sbom.display());
    for path in written.locks {
        println!("lock={}", path.display());
    }
    for path in written.easyconfigs {
        println!("easyconfig={}", path.display());
    }
    for path in written.easyblocks {
        println!("easyblock={}", path.display());
    }
    Ok(())
}

/// `--drop-patch NAME=REASON` values, in order. Both sides must be present:
/// a drop without a reason records a decision nobody can review.
fn parse_patch_drops(values: &[String]) -> Result<Vec<(String, String)>> {
    values
        .iter()
        .map(|value| {
            let (name, reason) = value
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("--drop-patch {value}: expected NAME=REASON"))?;
            let (name, reason) = (name.trim(), reason.trim());
            if name.is_empty() || reason.is_empty() {
                anyhow::bail!("--drop-patch {value}: both NAME and REASON are required");
            }
            Ok((name.to_string(), reason.to_string()))
        })
        .collect()
}

/// Say which easyblock class the recipe resolves to and where it is defined.
///
/// The bundle's `easyblocks/` directory, found beside the `easyconfigs/` tree
/// the recipe sits in, is searched before the named roots, matching the
/// precedence `--include-easyblocks` has over the installed package.
fn check_easyblock_resolution(
    recipe: &std::path::Path,
    resolved: &eb_stack::eb_parse::ResolvedEasyconfig,
    explicit_roots: &[PathBuf],
) -> Result<()> {
    let derived = resolved.easyblock.is_none();
    let class = resolved
        .easyblock
        .clone()
        .unwrap_or_else(|| eb_stack::eb_easyblock::encode_class_name(&resolved.name));
    let bundle_root = recipe
        .ancestors()
        .find(|ancestor| {
            ancestor
                .file_name()
                .is_some_and(|name| name == "easyconfigs")
        })
        .and_then(|easyconfigs| easyconfigs.parent())
        .map(|bundle| bundle.join("easyblocks"))
        .filter(|easyblocks| easyblocks.is_dir());
    let mut roots = Vec::new();
    if let Some(bundle) = bundle_root.as_deref() {
        roots.push(bundle);
    }
    roots.extend(explicit_roots.iter().map(PathBuf::as_path));
    let index = eb_stack::eb_easyblock::index_easyblocks(&roots).map_err(anyhow::Error::msg)?;
    let found = index.get(&class);
    println!(
        "easyblock_resolution={}",
        serde_json::to_string_pretty(&serde_json::json!({
            "class": class,
            "derived_from_name": derived,
            "defined_in": found.map(|(path, _)| path.display().to_string()),
            "bases": found.map(|(_, class)| class.bases.clone()),
            "extra_options": found.map(|(_, class)| class.extra_options.clone()),
            "searched": roots.iter().map(|root| root.display().to_string()).collect::<Vec<_>>(),
        }))?
    );
    if found.is_none()
        && !explicit_roots.is_empty()
        && class.starts_with(eb_stack::eb_easyblock::EASYBLOCK_CLASS_PREFIX)
    {
        bail!(
            "No software-specific easyblock '{class}' found for {}: none of the easyblock roots defines it",
            resolved.name
        );
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
            let report =
                eb_stack::workflows::repro_summary(&eb_stack::workflows::ReproSummaryRequest {
                    scores,
                    ratchet,
                    json,
                })?;
            print!("{}", report.body);
            if !report.violations.is_empty() {
                for violation in &report.violations {
                    eprintln!("ratchet: {violation}");
                }
                bail!("{} ratchet violation(s)", report.violations.len());
            }
            if let Some(line) = &report.ratchet_line {
                print!("{line}");
            }
            Ok(())
        }
    }
}

fn run_recipe(command: RecipeCommand) -> Result<()> {
    match command {
        RecipeCommand::Easyblock { name, charmap } => {
            let report = eb_stack::workflows::recipe_easyblock(name.as_deref(), charmap)?;
            print!("{}", report.stdout);
            Ok(())
        }
        RecipeCommand::Templates {
            name,
            version,
            export,
        } => {
            let report =
                eb_stack::workflows::recipe_templates(name.as_deref(), version.as_deref(), export)?;
            print!("{}", report.stdout);
            Ok(())
        }
        RecipeCommand::Hierarchy { export } => {
            let report = eb_stack::workflows::recipe_hierarchy(export)?;
            print!("{}", report.stdout);
            Ok(())
        }
        RecipeCommand::Check {
            recipe,
            easyconfigs,
            easyblock_roots,
            require_configopts,
            metadata_only,
            verify_sources: check_sources,
            seeded_from,
            seeded_source_url,
            seeded_git,
            source_tree,
            strict,
        } => {
            let recipe_text = std::fs::read_to_string(&recipe)
                .with_context(|| format!("--recipe {}", recipe.display()))?;
            if let Some((line, column, message)) = eb_stack::python_syntax_error(&recipe_text) {
                bail!(
                    "{}:{line}:{column}: {message}; EasyBuild cannot parse this easyconfig, so fix the syntax before checking it",
                    recipe.display()
                );
            }
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
                .with_context(|| format!("--recipe {}", recipe.display()))?;
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
            check_easyblock_resolution(&recipe, &resolved, &easyblock_roots)?;
            if metadata_only {
                if let Err(errors) = gate {
                    bail!("packaging gate failed: {}", errors.join("; "));
                }
                println!("recipe metadata resolves");
                return Ok(());
            }
            let roots = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let tree = parse_easyconfig_trees(&roots).map_err(|error| {
                let listed = easyconfigs
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::Error::msg(error).context(format!("--easyconfigs {listed}"))
            })?;
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
        RecipeCommand::Lint { paths, base } => {
            if let Some(base) = base.as_deref() {
                std::fs::metadata(base).with_context(|| format!("--base {}", base.display()))?;
            }
            for path in &paths {
                std::fs::metadata(path).with_context(|| format!("recipe {}", path.display()))?;
            }
            let path_refs = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let report = match base.as_deref() {
                Some(base) => lint_recipe_files_against(&path_refs, Some(base))?,
                None => lint_recipe_files(&path_refs)?,
            };
            let mut style = Vec::new();
            let mut maintainer_all = Vec::new();
            for file in &report.files {
                for finding in &file.findings {
                    let mut item = serde_json::json!({
                        "path": file.path.display().to_string(),
                        "line": finding.line,
                        "column": finding.column,
                        "code": finding.code,
                        "message": finding.message,
                        "severity": finding.severity,
                        "mechanical": finding.mechanical,
                    });
                    if !finding.origin.is_empty() {
                        item["origin"] = serde_json::json!(finding.origin);
                    }
                    if finding.info {
                        item["info"] = serde_json::json!(true);
                    }
                    if finding.in_review {
                        item["in_review"] = serde_json::json!(true);
                    }
                    style.push(item);
                }
                let text = std::fs::read_to_string(&file.path)
                    .with_context(|| format!("read {}", file.path.display()))?;
                let maintainer = check_maintainer_acceptability_text(&text);
                for f in maintainer.findings {
                    maintainer_all.push(serde_json::json!({
                        "path": file.path.display().to_string(),
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
                    "clean": report.clean(),
                    "error_free": report.error_free(),
                    "style": style,
                    "check_contrib": report.check_contrib,
                    "maintainer_acceptability": maintainer_all,
                }))?
            );
            let maintainer_errors = maintainer_all
                .iter()
                .filter(|v| v.get("severity").and_then(|s| s.as_str()) == Some("error"))
                .count();
            let style_errors = report
                .files
                .iter()
                .flat_map(|file| file.findings.iter())
                .filter(|finding| finding.severity == eb_stack::Severity::Error)
                .count();
            if style_errors > 0 || maintainer_errors > 0 {
                bail!(
                    "{style_errors} style errors ({} findings in total), {maintainer_errors} maintainer-acceptability errors",
                    style.len(),
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
                return Err(usage("--out requires exactly one recipe path"));
            }
            for (index, path) in paths.iter().enumerate() {
                std::fs::metadata(path).with_context(|| format!("recipe {}", path.display()))?;
                let destination = out.as_deref().filter(|_| index == 0);
                let result = if dry_run {
                    let text = std::fs::read_to_string(path)
                        .with_context(|| format!("recipe {}", path.display()))?;
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
        StackCommand::Solve(args) => {
            let SolveArgs {
                easyconfigs,
                policy,
                toolchain,
                mut roots,
                roots_from_build_list,
                baseline_easyconfigs,
                baseline_toolchain_version,
                baseline_easystacks,
                baseline_commits_from,
                filter_deps,
                baseline_also_toolchains,
                lock_out,
                sbom_out,
                build_list_out,
                build_list_excludes_baseline,
                jenkins_build_list_out,
                jenkins_flags_from,
                jenkins_gpu_systems,
                jenkins_roots_only,
                stack_diff_out,
            } = *args;
            for path in &roots_from_build_list {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading build list {}", path.display()))?;
                for line in eb_stack::jenkins::parse_build_list(&text) {
                    match eb_stack::jenkins::package_name(&line.file) {
                        Some(name) if !roots.iter().any(|r| r == name) => roots.push(name.into()),
                        Some(_) => {}
                        None => eprintln!("{}: no package name in {}", path.display(), line.file),
                    }
                }
            }
            if toolchain.is_some() && roots.is_empty() {
                return Err(usage(
                    "--toolchain needs at least one --root or --roots-from-build-list",
                ));
            }
            // A policy is a file when there is one to reuse, and a pair of
            // flags when there is not. Writing JSON by hand to answer two
            // questions is a step nobody should need for the common case.
            let generated_policy = policy.is_none();
            let written_policy;
            let policy = match policy {
                Some(path) => {
                    std::fs::metadata(&path)
                        .with_context(|| format!("--stack-policy {}", path.display()))?;
                    path
                }
                None => {
                    let spec = toolchain.as_deref().ok_or_else(|| {
                        usage("--toolchain wants name/version, for example foss/2026.1")
                    })?;
                    let parsed = eb_stack::cli_parse::parse_toolchain_spec(spec, "--toolchain")?;
                    let name = parsed.name.as_str();
                    let version = parsed.version.as_str();
                    let policy = serde_json::json!({
                        "toolchain": {"name": name, "version": version},
                        "roots": roots,
                        "objective": "prefer_newer",
                    });
                    // One file per invocation. Two solves for the same
                    // toolchain must not read each other's roots, including a
                    // second solve in this process while the first still reads.
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_nanos())
                        .unwrap_or(0);
                    written_policy = std::env::temp_dir().join(format!(
                        "eb-stack-policy-{}-{}-{}-{stamp}.json",
                        name,
                        version,
                        std::process::id()
                    ));
                    std::fs::write(&written_policy, serde_json::to_vec_pretty(&policy)?)?;
                    written_policy
                }
            };
            let stacks = baseline_easystacks
                .iter()
                .map(PathBuf::as_path)
                .collect::<Vec<_>>();
            let also = baseline_also_toolchains
                .iter()
                .map(|t| {
                    t.split_once('/')
                        .map(|(name, version)| Toolchain {
                            name: name.into(),
                            version: version.into(),
                        })
                        .ok_or_else(|| {
                            usage(format!(
                                "--baseline-also-toolchain wants name/version, got {t}"
                            ))
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            let baseline = if stacks.is_empty() {
                match baseline_easyconfigs
                    .as_deref()
                    .or_else(|| easyconfigs.first().map(PathBuf::as_path))
                {
                    Some(root) => eb_stack::BaselineSource::Tree {
                        root,
                        toolchain_version: baseline_toolchain_version.as_deref(),
                    },
                    None => eb_stack::BaselineSource::None,
                }
            } else {
                eb_stack::BaselineSource::Easystacks {
                    files: &stacks,
                    commits_repo: baseline_commits_from.as_deref(),
                    also_toolchains: &also,
                }
            };
            let jenkins_only = if jenkins_roots_only {
                let policy_roots: Vec<String> = match serde_json::from_slice::<serde_json::Value>(
                    &std::fs::read(&policy)
                        .with_context(|| format!("reading policy {}", policy.display()))?,
                )?
                .get("roots")
                .and_then(|r| r.as_array())
                {
                    Some(list) => list
                        .iter()
                        .filter_map(|r| r.as_str().map(str::to_string))
                        .collect(),
                    None => bail!("--jenkins-roots-only needs roots in {}", policy.display()),
                };
                Some(policy_roots)
            } else {
                None
            };
            let mut jenkins_previous = Vec::new();
            for path in &jenkins_flags_from {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading build list {}", path.display()))?;
                jenkins_previous.extend(eb_stack::jenkins::parse_build_list(&text));
            }
            let roots = easyconfigs.iter().map(PathBuf::as_path).collect::<Vec<_>>();
            let solved = eb_stack::solve_with_baseline_source(
                &roots,
                &policy,
                baseline,
                &lock_out,
                sbom_out.as_deref(),
                SolveExtraOut {
                    build_list_out: build_list_out.as_deref(),
                    stack_diff_out: stack_diff_out.as_deref(),
                    build_list_excludes_baseline,
                    filter_deps: &filter_deps,
                    jenkins_build_list_out: jenkins_build_list_out.as_deref(),
                    jenkins: eb_stack::jenkins::JenkinsOptions {
                        previous: jenkins_previous,
                        gpu_systems: jenkins_gpu_systems,
                        only: jenkins_only,
                    },
                },
            );
            if generated_policy {
                let _ = std::fs::remove_file(&policy);
            }
            let lock = solved.map_err(|error| {
                let head = error.to_string();
                if head.starts_with("IO ") || head.starts_with("parse ") {
                    let listed = easyconfigs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    error.context(format!("--easyconfigs {listed}"))
                } else {
                    error
                }
            })?;
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
            filter_deps,
        } => {
            let report =
                eb_stack::workflows::stack_order(&eb_stack::workflows::StackOrderRequest {
                    easyconfigs,
                    roots,
                    out,
                    oldest,
                    easystack_out,
                    hashes_out,
                    filter_deps,
                })?;
            print!("{}", report.stdout);
            Ok(())
        }
        StackCommand::Sections {
            list,
            from,
            unfiled,
            out,
        } => {
            let report =
                eb_stack::workflows::stack_sections(&eb_stack::workflows::StackSectionsRequest {
                    list,
                    from,
                    unfiled,
                    out,
                })?;
            print!("{}", report.stdout);
            Ok(())
        }
        StackCommand::Gap {
            site,
            installed,
            easyconfigs,
            generation,
            exclude_file,
            filter_deps,
            out_dir,
        } => {
            let run = eb_stack::site_commands::run_stack_gap(
                &eb_stack::site_commands::StackGapRequest {
                    site,
                    installed,
                    easyconfigs,
                    generation,
                    exclude_file,
                    filter_deps,
                    out_dir,
                },
            )?;
            println!("{}", run.summary());
            Ok(())
        }
        StackCommand::Batch(args) => {
            let BatchArgs {
                mut roots,
                roots_file,
                easyconfigs,
                commit_trees,
                installed,
                installed_modulerc,
                open_prs,
                open_repo,
                eessi_version,
                easybuild_version,
                generation,
                filter_deps,
                size,
                arch,
                unsupported,
                easyconfigs_repo,
                generated_at,
                out_dir,
            } = *args;
            if let Some(path) = roots_file {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("--roots-file {}", path.display()))?;
                roots.extend(
                    text.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty() && !l.starts_with('#'))
                        .map(String::from),
                );
            }
            let command_line = std::env::args()
                .map(|a| eb_stack::target::shell_quote(&a))
                .collect::<Vec<_>>()
                .join(" ");
            let run = eb_stack::site_commands::run_stack_batch(
                &eb_stack::site_commands::StackBatchRequest {
                    roots,
                    easyconfigs,
                    commit_trees,
                    installed,
                    installed_modulerc,
                    open_prs,
                    open_repo,
                    eessi_version,
                    easybuild_version,
                    generation,
                    filter_deps,
                    size,
                    easyconfigs_repo,
                    generated_at,
                    arches: arch,
                    unsupported,
                    command_line,
                    out_dir,
                },
            )?;
            println!("{}", run.summary());
            Ok(())
        }
        StackCommand::Arch {
            arch,
            easystack,
            easyconfigs,
            installed,
            additional_arch,
            unsupported,
            probe_sources,
            probe_retries,
            probe_timeout_s,
        } => {
            let report = eb_stack::arch_support::run_arch_check(
                &eb_stack::arch_support::ArchCheckRequest {
                    arches: arch,
                    easystacks: easystack,
                    easyconfigs,
                    installed,
                    additional: additional_arch,
                    unsupported,
                    source_probe: probe_sources.then(|| {
                        eb_stack::arch_support::SourceProbeRequest {
                            arches: eb_stack::source_probe::DEFAULT_TARGET_ARCHES
                                .iter()
                                .map(|arch| arch.to_string())
                                .collect(),
                            retries: probe_retries,
                            timeout: std::time::Duration::from_secs(probe_timeout_s),
                        }
                    }),
                },
            )?;
            print!("{}", report.stdout);
            if !report.parse_failed.is_empty() {
                bail!("{}", report.parse_failed.join("\n"));
            }
            if !report.unsupported.is_empty() {
                bail!("{}", report.unsupported.join("\n"));
            }
            if !report.flagged.is_empty() {
                bail!("{}", report.flagged.join("\n"));
            }
            if !report.source_findings.is_empty() {
                bail!("{}", report.source_findings.join("\n"));
            }
            Ok(())
        }
        StackCommand::Files {
            easystack,
            easyconfigs,
            commit_trees,
            easyconfigs_repo,
            sourcepath,
            path_map,
        } => {
            let report = eb_stack::file_presence::run_file_presence(
                &eb_stack::file_presence::FilePresenceRequest {
                    easystacks: easystack,
                    easyconfigs,
                    commit_trees,
                    easyconfigs_repo,
                    sourcepaths: sourcepath,
                    path_maps: path_map,
                },
            )?;
            print!("{}", report.stdout);
            if !report.missing.is_empty() {
                bail!(
                    "{}",
                    report
                        .missing
                        .iter()
                        .map(|missing| missing.message.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            }
            Ok(())
        }
        StackCommand::Easystack {
            lock,
            out,
            options,
            eessi_version,
            easybuild_version,
            generation,
            out_dir,
        } => {
            let report = match (eessi_version, easybuild_version, generation, out_dir) {
                (Some(eessi_version), Some(easybuild_version), Some(generation), Some(out_dir)) => {
                    eb_stack::workflows::stack_easystack_eessi(
                        &eb_stack::workflows::StackEasystackEessiRequest {
                            lock,
                            out_dir,
                            eessi_version,
                            easybuild_version,
                            generation,
                            options,
                        },
                    )?
                }
                _ => eb_stack::workflows::stack_easystack(
                    &eb_stack::workflows::StackEasystackRequest { lock, out, options },
                )?,
            };
            print!("{}", report.stdout);
            Ok(())
        }
        StackCommand::FromCommit {
            easystacks,
            easyconfigs_repo,
            easyblocks_repo,
            easybuild_version,
            json,
            rewrite,
        } => {
            let report = eb_stack::from_commit::check(&eb_stack::from_commit::FromCommitRequest {
                easystacks,
                easyconfigs_repo,
                easyblocks_repo,
                easybuild_version,
                json,
                rewrite,
            })?;
            print!("{}", report.stdout);
            if report.released > 0 && !rewrite {
                bail!(
                    "{} from-commit pin(s) already in the EasyBuild release",
                    report.released
                );
            }
            Ok(())
        }
        StackCommand::Sbom { lock, out } => {
            let loaded: StackLock = load_lock_file(&lock)?;
            let (sbom, stats) = eb_stack::lock_to_cyclonedx_from_recipes(&loaded, &lock);
            write_json_pretty(&out, &sbom)?;
            println!(
                "sbom={} components={} recipes_read={} recipes_unreadable={} with_artifact_facts={}",
                out.display(),
                loaded.packages.len(),
                stats.recipes_read,
                stats.recipes_unreadable,
                stats.with_artifact_facts
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
                .with_context(|| format!("--target {target} is not configured"))?;
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
                .with_context(|| format!("--target {target} is not configured"))?;
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
            "--source {}: not a file or a registry name",
            source.display()
        );
    }
    let format = format.ok_or_else(|| {
        usage("--format pypi, cran, or cargo is required when --source is a registry name")
    })?;
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            usage(format!(
                "--source {}: invalid registry name",
                source.display()
            ))
        })?;
    let ingest = materialize_registry_name(name, format, &UreqClient, &out_dir.join("ingest"))
        .with_context(|| format!("fetch {name} as {}", format.as_str()))?;
    Ok(ingest.dump)
}

fn parse_format(value: &str) -> Result<Option<ForeignFormat>> {
    eb_stack::cli_parse::parse_foreign_format(value, "--format").map_err(anyhow::Error::from)
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
        .with_context(|| format!("--stack-policy {}", path.display()))?;
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
    let listed = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    resolve_target_layers(&layers)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("--config {listed}"))
}

fn parse_dep_overrides(values: &[String]) -> Result<HashMap<String, String>> {
    let mut dependencies = HashMap::new();
    for value in values {
        let Some((name, version)) = value.split_once('=') else {
            return Err(usage(format!("--dep expects NAME=VERSION, got {value:?}")));
        };
        if name.trim().is_empty() || version.trim().is_empty() {
            return Err(usage(format!(
                "--dep expects non-empty NAME=VERSION, got {value:?}"
            )));
        }
        dependencies.insert(name.trim().to_string(), version.trim().to_string());
    }
    Ok(dependencies)
}

/// Read a stack lock. The path is named once, as `--lock`.
fn load_lock_file(path: &Path) -> Result<StackLock> {
    eb_stack::workflows::read_lock(path)
}

fn resolve_toolchain(
    name: Option<&str>,
    version: Option<&str>,
    spec: Option<&str>,
    default_name: bool,
) -> Result<Toolchain> {
    // Clap rejects combining --toolchain with the split flags. Inspect and
    // plan default a missing name to foss. Bump does not, the same as MCP.
    let (name, version) = if spec.is_some() {
        (None, None)
    } else {
        (name, version)
    };
    eb_stack::cli_parse::resolve_toolchain(name, version, spec, default_name, "--toolchain")
        .map_err(anyhow::Error::from)
}
