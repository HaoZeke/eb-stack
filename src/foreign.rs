//! Ingest foreign package recipes (conda-forge, Spack) into EasyBuild scaffolds.
//!
//! # Conda-forge
//!
//! Supports classic `meta.yaml` (conda-build) and v1 `recipe.yaml` (rattler-build):
//! - expands a **restricted** Jinja subset: `{% set x = "..." %}`, `context:` scalars,
//!   `${{ var }}` / `{{ var }}` / `{{ var|lower }}` (no filters beyond lower, no
//!   `compiler()` evaluation — those lines are dropped as build-tool noise);
//! - parses multi-entry `source:` lists and single mapping sources;
//! - walks `requirements.{build,host,run}` list items, including selector-wrapped
//!   maps (`- if: ... then: ...`) by taking the first string leaf.
//!
//! # Spack
//!
//! Restricted static parse of `package.py` (no Python exec), following Spack's
//! package DSL as written in real packages:
//! - `class Name(Base)` and multi-base `class Name(Base1, Base2)`;
//! - `homepage` / `url` / `git` string attributes;
//! - `version("X", sha256=..., tag=..., commit=..., url=...)` kwargs;
//! - preferred version = first non-`develop`/`main`/`master`/`head` entry
//!   (Spack lists preferred versions first);
//! - `depends_on("spec", type=..., when=...)` including `type=("build", "run")`
//!   tuples and multi-type lists; language virtuals `c`/`cxx`/`fortran` skipped.
//!
//! Residuals (EB generation-native dep versions, easyblock/build logic) surface
//! as warnings — never invented as authoritative.
//!
//! # Robot resolve (when `--easyconfigs` is set)
//!
//! Same two-stage path as `bump --easyconfigs`:
//! 1. **Hierarchy consensus** ([`resolve_dep_versions_for_specs`]) for
//!    generation-native floors (GCCcore-level deps included).
//! 2. **Resolvo joint co-select** ([`resolvo_resolve_dep_versions`]) over
//!    hierarchy-eligible candidates; joint SAT wins when it succeeds.

use crate::domain::{Candidate, Toolchain};
use crate::eb_emit::easyconfig_filename;
use crate::eb_parse::{
    companion_easyconfig_basename, easyconfig_letter_dir, parse_easyconfig_trees,
};
use crate::hierarchy::{
    hierarchy_for_with_tree, resolve_dep_versions_for_specs, SourceDepSpec, ToolchainHierarchy,
};
use crate::select::resolvo_resolve_dep_versions;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Origin of a foreign recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForeignFormat {
    CondaForge,
    Spack,
}

impl ForeignFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CondaForge => "conda-forge",
            Self::Spack => "spack",
        }
    }
}

/// One dependency name (and optional pin) drawn from a foreign recipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignDep {
    pub name: String,
    /// Pin / constraint as written in the foreign recipe when present.
    pub pin: Option<String>,
    /// Role: `build`, `host`, `run`, or Spack type string when known.
    pub role: String,
}

/// One source artifact (URL/git + optional checksum).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ForeignSource {
    pub url: Option<String>,
    pub filename: Option<String>,
    pub sha256: Option<String>,
    pub git: Option<String>,
    pub tag: Option<String>,
    pub commit: Option<String>,
}

/// Intermediate fields shared by all foreign formats before EB emit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignRecipe {
    pub format: ForeignFormat,
    pub name: String,
    pub version: String,
    pub homepage: Option<String>,
    /// Preferred / primary source (first expanded URL or preferred Spack version).
    pub source_url: Option<String>,
    pub source_filename: Option<String>,
    pub sha256: Option<String>,
    /// All sources when the foreign recipe lists multiple (conda multi-source, etc.).
    #[serde(default)]
    pub sources: Vec<ForeignSource>,
    pub summary: Option<String>,
    pub dependencies: Vec<ForeignDep>,
    /// Build-system / base-class hints (e.g. Spack `MesonPackage`, `CMakePackage`).
    #[serde(default)]
    pub build_system_hints: Vec<String>,
    /// Mechanically extracted configure flags (e.g. Spack meson_args / cmake_args literals).
    #[serde(default)]
    pub configopts: Option<String>,
    /// Human notes from the parser.
    pub notes: Vec<String>,
}

/// Options for foreign → EB ingest, including optional robot hierarchy resolve.
#[derive(Debug, Clone, Default)]
pub struct IngestOpts {
    /// Robot easyconfig tree(s); later paths override earlier (site overlay).
    pub easyconfigs: Vec<PathBuf>,
    /// Keep foreign residual versions when the robot has no candidate.
    pub keep_old_deps: bool,
    /// Optional hierarchy fixture override.
    pub hierarchy_fixture: Option<PathBuf>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ForeignError {
    #[error("foreign recipe parse: {0}")]
    Parse(String),
    #[error("unsupported or undetected foreign recipe format for {0}")]
    Unsupported(String),
    #[error("IO: {0}")]
    Io(String),
}

/// Result of ingest → EB emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestResult {
    pub recipe: ForeignRecipe,
    pub filename: String,
    pub text: String,
    pub warnings: Vec<String>,
}

/// Detect format from path basename / extension.
pub fn detect_foreign_format(path: &Path) -> Option<ForeignFormat> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "meta.yaml" || name == "meta.yml" || name == "recipe.yaml" || name == "recipe.yml" {
        return Some(ForeignFormat::CondaForge);
    }
    if name == "package.py" {
        return Some(ForeignFormat::Spack);
    }
    None
}

/// Parse foreign recipe text for the given format.
pub fn parse_foreign_str(format: ForeignFormat, text: &str) -> Result<ForeignRecipe, ForeignError> {
    match format {
        ForeignFormat::CondaForge => parse_conda_forge(text),
        ForeignFormat::Spack => parse_spack_package(text),
    }
}

/// Parse from path; format auto-detected unless `format` is Some.
pub fn parse_foreign_path(
    path: &Path,
    format: Option<ForeignFormat>,
) -> Result<ForeignRecipe, ForeignError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ForeignError::Io(format!("read {}: {e}", path.display())))?;
    let fmt = format
        .or_else(|| detect_foreign_format(path))
        .ok_or_else(|| ForeignError::Unsupported(path.display().to_string()))?;
    parse_foreign_str(fmt, &text)
}

/// Emit a parseable EasyBuild scaffold from intermediate fields.
pub fn emit_easyconfig_from_foreign(
    recipe: &ForeignRecipe,
    toolchain: &Toolchain,
) -> IngestResult {
    let mut warnings = recipe.notes.clone();
    warnings.push(format!(
        "ingested from {}; toolchain and full build logic are residual — review before `eb` install",
        recipe.format.as_str()
    ));

    let homepage = recipe.homepage.clone().unwrap_or_else(|| {
        format!("https://example.invalid/TODO-{}", recipe.name)
    });
    let summary = recipe.summary.clone().unwrap_or_else(|| {
        format!(
            "Scaffold from {} for {}-{}",
            recipe.format.as_str(),
            recipe.name,
            recipe.version
        )
    });

    let easyblock = guess_easyblock(recipe, &mut warnings);
    let (source_urls_line, sources_line, checksums_line) = source_block(recipe, &mut warnings);
    let configopts_line = match &recipe.configopts {
        Some(c) if !c.is_empty() => {
            warnings.push(format!(
                "configopts mechanically extracted from foreign recipe ({c})"
            ));
            format!("configopts = '{}'\n", escape_py_single(c))
        }
        _ => String::new(),
    };

    let mut dep_comment_lines: Vec<String> = Vec::new();
    let mut builddep_tuples: Vec<String> = Vec::new();
    let mut dep_tuples: Vec<String> = Vec::new();
    let mut seen_dep: std::collections::HashSet<String> = std::collections::HashSet::new();
    for d in &recipe.dependencies {
        let eb_name = map_dep_name_to_eb(&d.name);
        let pin_note = d.pin.as_deref().unwrap_or("");
        dep_comment_lines.push(format!(
            "#   - {} → {} [{}]{}",
            d.name,
            eb_name,
            d.role,
            if pin_note.is_empty() {
                String::new()
            } else {
                format!(" pin={pin_note}")
            }
        ));
        if !seen_dep.insert(eb_name.clone()) {
            continue;
        }
        let is_build = d.role.contains("build") && !d.role.contains("run");
        if let Some(ver) = residual_version_token(d.pin.as_deref()) {
            let tuple = format!("    ('{eb_name}', '{ver}'),");
            if is_build {
                builddep_tuples.push(tuple);
            } else {
                dep_tuples.push(tuple);
            }
            warnings.push(format!(
                "dependency {eb_name} uses residual foreign version {ver} (from {:?}) — not EB generation consensus; re-resolve with bump --easyconfigs",
                d.pin
            ));
        } else {
            // Still emit a residual pin so the scaffold is a real work queue, not empty.
            let tuple = format!("    ('{eb_name}', '0.0.0'),  # residual: no foreign pin");
            if is_build {
                builddep_tuples.push(tuple);
            } else {
                dep_tuples.push(tuple);
            }
            warnings.push(format!(
                "dependency {eb_name} ({}) has no foreign pin — emitted residual 0.0.0 for review",
                d.role
            ));
        }
    }

    let builddeps_block = if builddep_tuples.is_empty() {
        "builddependencies = []\n".to_string()
    } else {
        format!(
            "builddependencies = [\n{}\n]\n",
            builddep_tuples.join("\n")
        )
    };
    let deps_block = if dep_tuples.is_empty() {
        "dependencies = []\n".to_string()
    } else {
        format!("dependencies = [\n{}\n]\n", dep_tuples.join("\n"))
    };

    let tc_line = format!(
        "toolchain = {{'name': '{}', 'version': '{}'}}",
        toolchain.name, toolchain.version
    );

    let warn_header: String = warnings
        .iter()
        .map(|w| format!("# WARNING: {w}"))
        .collect::<Vec<_>>()
        .join("\n");

    let foreign_deps_header = if dep_comment_lines.is_empty() {
        "# Foreign dependencies: (none extracted)\n".to_string()
    } else {
        format!(
            "# Foreign-origin dependencies (names from {}; versions not EB-resolved):\n{}\n",
            recipe.format.as_str(),
            dep_comment_lines.join("\n")
        )
    };

    let text = format!(
        r#"# EasyBuild scaffold generated by eb-stack ingest ({origin}).
# Residuals: full product configopts, patches, and EB generation-native dep versions.
{warn_header}
{foreign_deps_header}easyblock = '{easyblock}'

name = '{name}'
version = '{version}'
homepage = '{homepage}'
description = """{summary}

Origin: {origin}. Re-resolve dependency versions with `eb-stack bump --easyconfigs`
against a robot tree before production install."""

{tc_line}

{source_urls_line}
{sources_line}
{checksums_line}
{configopts_line}
{builddeps_block}
{deps_block}
sanity_check_paths = {{
    'files': [],
    'dirs': ['lib', 'include'],
}}

moduleclass = 'lib'
"#,
        origin = recipe.format.as_str(),
        warn_header = warn_header,
        foreign_deps_header = foreign_deps_header,
        easyblock = easyblock,
        name = recipe.name,
        version = recipe.version,
        homepage = escape_py_single(&homepage),
        summary = escape_py_triple(&summary),
        tc_line = tc_line,
        source_urls_line = source_urls_line,
        sources_line = sources_line,
        checksums_line = checksums_line,
        configopts_line = configopts_line,
        builddeps_block = builddeps_block,
        deps_block = deps_block,
    );

    let filename = easyconfig_filename(&recipe.name, &recipe.version, toolchain);
    IngestResult {
        recipe: recipe.clone(),
        filename,
        text,
        warnings,
    }
}

/// Parse path, emit scaffold (no robot resolve).
pub fn ingest_foreign_to_easyconfig(
    path: &Path,
    format: Option<ForeignFormat>,
    toolchain: &Toolchain,
) -> Result<IngestResult, ForeignError> {
    ingest_foreign_to_easyconfig_with_opts(path, format, toolchain, &IngestOpts::default())
}

/// Parse path, emit scaffold, and **mechanically** fill dependency versions
/// (and preferred package name casing) from robot easyconfig tree(s) when
/// `opts.easyconfigs` is non-empty — same hierarchy resolve path as `bump`.
pub fn ingest_foreign_to_easyconfig_with_opts(
    path: &Path,
    format: Option<ForeignFormat>,
    toolchain: &Toolchain,
    opts: &IngestOpts,
) -> Result<IngestResult, ForeignError> {
    let mut recipe = parse_foreign_path(path, format)?;
    // Spack: pull string-literal -D flags from meson_args / cmake_args bodies.
    if recipe.format == ForeignFormat::Spack {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Some(flags) = extract_spack_config_flags(&text) {
                recipe.configopts = Some(flags);
            }
        }
    }
    let mut result = emit_easyconfig_from_foreign(&recipe, toolchain);
    if !opts.easyconfigs.is_empty() {
        result = apply_robot_resolution(result, toolchain, opts)?;
    }
    Ok(result)
}

/// Resolve dep/builddep versions in an already-emitted scaffold against robot trees.
///
/// Prefer joint **resolvo** selection over hierarchy-eligible candidates when
/// the SAT solve is feasible; fall back to generation hierarchy consensus
/// (same path as `bump`) for pins resolvo cannot co-select.
fn apply_robot_resolution(
    mut result: IngestResult,
    toolchain: &Toolchain,
    opts: &IngestOpts,
) -> Result<IngestResult, ForeignError> {
    let roots: Vec<&Path> = opts.easyconfigs.iter().map(|p| p.as_path()).collect();
    let tree = parse_easyconfig_trees(&roots).map_err(|e| {
        ForeignError::Parse(format!("robot parse: {e}"))
    })?;
    result.warnings.push(format!(
        "robot resolve: {} candidate(s), {:.1}% parse coverage",
        tree.candidates.len(),
        100.0 * tree.coverage()
    ));

    // Prefer EasyBuild package name casing when the robot already has this software.
    if let Some(canon) = find_canonical_package_name(&result.recipe.name, &tree.candidates) {
        if canon != result.recipe.name {
            result.warnings.push(format!(
                "package name casing from robot: {} → {}",
                result.recipe.name, canon
            ));
            result.text = result.text.replacen(
                &format!("name = '{}'", result.recipe.name),
                &format!("name = '{canon}'"),
                1,
            );
            result.filename = easyconfig_filename(&canon, &result.recipe.version, toolchain);
            result.recipe.name = canon;
        }
    }

    // Build specs from mapped foreign deps (skip toolchain virtuals).
    let mut specs = Vec::new();
    for d in &result.recipe.dependencies {
        let eb_name = map_dep_name_to_eb(&d.name);
        if is_toolchain_virtual(&eb_name) {
            result.warnings.push(format!(
                "robot resolve: skip toolchain virtual '{eb_name}' (provided by foss/GCCcore stack)"
            ));
            continue;
        }
        let floor = residual_version_token(d.pin.as_deref()).unwrap_or_else(|| "0.0.0".into());
        specs.push(SourceDepSpec::plain(eb_name, floor));
    }
    if specs.is_empty() {
        result
            .warnings
            .push("robot resolve: no non-virtual foreign deps to resolve".into());
        return Ok(result);
    }

    let h: ToolchainHierarchy = hierarchy_for_with_tree(
        toolchain,
        opts.hierarchy_fixture.as_deref(),
        &tree.candidates,
    )
    .map_err(|e| ForeignError::Parse(format!("hierarchy: {e}")))?;
    result.warnings.push(format!(
        "robot resolve: hierarchy {}-{} members: {}",
        toolchain.name,
        toolchain.version,
        h.member_labels().join(" < ")
    ));

    // Hierarchy consensus first (generation-native floors; GCCcore-aware).
    let (mut map, kept) =
        resolve_dep_versions_for_specs(&specs, &tree.candidates, &h, opts.keep_old_deps).map_err(
            |e| ForeignError::Parse(format!("robot dep resolve: {e}")),
        )?;
    for k in kept {
        result.warnings.push(format!("robot resolve: {k}"));
    }
    for (name, ver) in &map {
        result.warnings.push(format!(
            "robot resolve: {name} → {ver} (hierarchy consensus)"
        ));
    }

    // Joint resolvo over hierarchy-eligible candidates (shared with bump).
    match resolvo_resolve_dep_versions(
        &specs,
        &tree.candidates,
        &h,
        toolchain,
        &result.recipe.name,
        &result.recipe.version,
        Some(&map),
    ) {
        Ok((resolvo_map, engine_note)) => {
            result
                .warnings
                .push(format!("robot resolve: {engine_note}"));
            for (name, ver) in resolvo_map {
                match map.get(&name) {
                    Some(hver) if hver == &ver => {
                        result.warnings.push(format!(
                            "robot resolve: {name} → {ver} (resolvo joint agrees with hierarchy)"
                        ));
                    }
                    Some(hver) => {
                        result.warnings.push(format!(
                            "robot resolve: {name} resolvo={ver} hierarchy={hver}; keeping hierarchy"
                        ));
                    }
                    None => {
                        result.warnings.push(format!(
                            "robot resolve: {name} → {ver} (resolvo joint; hierarchy had no pin)"
                        ));
                        map.insert(name, ver);
                    }
                }
            }
        }
        Err(why) => {
            result.warnings.push(format!(
                "robot resolve: resolvo joint skipped ({why}); using hierarchy consensus only"
            ));
        }
    }

    // Rewrite residual version tokens in the emitted text.
    for (name, ver) in &map {
        let re = Regex::new(&format!(
            r"(\('{re_name}',\s*')([^']*)(')",
            re_name = regex::escape(name)
        ))
        .map_err(|e| ForeignError::Parse(e.to_string()))?;
        let ver = ver.clone();
        result.text = re
            .replace_all(&result.text, move |caps: &regex::Captures| {
                format!("{}{}{}", &caps[1], ver, &caps[3])
            })
            .to_string();
    }

    // Strip residual 0.0.0 lines for deps that never resolved (keep comments).
    let zero_re = Regex::new(r#"(?m)^\s*\('[^']+',\s*'0\.0\.0'\),?\s*(?:#.*)?\n"#)
        .map_err(|e| ForeignError::Parse(e.to_string()))?;
    if !opts.keep_old_deps {
        let stripped = zero_re.replace_all(&result.text, "");
        if stripped != result.text {
            result.warnings.push(
                "robot resolve: dropped residual 0.0.0 dep lines that did not resolve (pass --keep-old-deps to retain)"
                    .into(),
            );
            result.text = stripped.to_string();
        }
    }

    Ok(result)
}

fn find_canonical_package_name(name: &str, cands: &[Candidate]) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    // Prefer exact case-insensitive match with most common casing among candidates
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in cands {
        if c.name.to_ascii_lowercase() == lower {
            *counts.entry(c.name.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(n, _)| n)
}

fn is_toolchain_virtual(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "blas"
            | "lapack"
            | "scalapack"
            | "mpi"
            | "fftw"
            | "fftw-api"
            | "mkl"
            | "openblas"
            | "c"
            | "cxx"
            | "fortran"
            | "libgcc-ng"
            | "libstdcxx-ng"
            | "llvm-openmp"
    )
}

/// Extract static `-D…` string literals from Spack packages.
///
/// Only plain string list items (not f-strings): `"-Dwith_xtb=false"`.
fn extract_spack_config_flags(text: &str) -> Option<String> {
    // Line must start with whitespace then a quote (not f") so f-strings are skipped.
    let lit = Regex::new(r#"(?m)^[ \t]+[\"'](-D[A-Za-z0-9_./+=-]+)[\"']"#).ok()?;
    let mut flags: Vec<String> = lit
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    if flags.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    flags.retain(|f| seen.insert(f.clone()));
    Some(flags.join(" "))
}

fn guess_easyblock(recipe: &ForeignRecipe, warnings: &mut Vec<String>) -> String {
    for h in &recipe.build_system_hints {
        let hl = h.to_ascii_lowercase();
        if hl.contains("meson") {
            warnings.push(format!("build-system hint {h} → easyblock MesonNinja"));
            return "MesonNinja".into();
        }
        if hl.contains("cmake") {
            warnings.push(format!("build-system hint {h} → easyblock CMakeNinja"));
            return "CMakeNinja".into();
        }
        if hl.contains("python") || hl.contains("pip") {
            warnings.push(format!("build-system hint {h} → easyblock PythonPackage"));
            return "PythonPackage".into();
        }
        if hl.contains("autotools") || hl.contains("autoreconf") {
            return "ConfigureMake".into();
        }
    }
    // Dep names as weak signal
    let dep_names: Vec<&str> = recipe.dependencies.iter().map(|d| d.name.as_str()).collect();
    if dep_names.iter().any(|n| *n == "meson" || n.ends_with("-meson")) {
        warnings.push("meson in foreign deps → easyblock MesonNinja".into());
        return "MesonNinja".into();
    }
    if dep_names.iter().any(|n| *n == "cmake" || *n == "ninja") {
        warnings.push("cmake/ninja in foreign deps → easyblock CMakeNinja".into());
        return "CMakeNinja".into();
    }
    "ConfigureMake".into()
}

fn source_block(recipe: &ForeignRecipe, warnings: &mut Vec<String>) -> (String, String, String) {
    let mut resolved: Vec<ForeignSource> = if recipe.sources.is_empty() {
        vec![ForeignSource {
            url: recipe.source_url.clone(),
            filename: recipe.source_filename.clone(),
            sha256: recipe.sha256.clone(),
            ..Default::default()
        }]
    } else {
        recipe.sources.clone()
    };

    // Materialize git+tag into a GitHub archive URL when possible.
    for s in &mut resolved {
        if s.url.is_none() {
            if let (Some(git), Some(tag)) = (s.git.as_ref(), s.tag.as_ref()) {
                if let Some(archive) = github_archive_url(git, tag) {
                    warnings.push(format!(
                        "materialized git+tag into archive URL for tag {tag}"
                    ));
                    s.url = Some(archive);
                }
            }
        }
    }

    let usable: Vec<&ForeignSource> = resolved
        .iter()
        .filter(|s| s.url.as_ref().or(s.git.as_ref()).is_some())
        .collect();

    if usable.is_empty() {
        warnings.push("no source URL in foreign recipe; placeholder source used".into());
        return (
            "source_urls = ['https://example.invalid/TODO/']".into(),
            "sources = [SOURCE_TAR_GZ]".into(),
            format!("checksums = ['{}']", "0".repeat(64)),
        );
    }

    if usable.len() == 1 {
        let s = usable[0];
        let url = s.url.as_ref().or(s.git.as_ref()).unwrap();
        let fname = s
            .filename
            .clone()
            .unwrap_or_else(|| filename_from_url(url));
        let (base, file) = split_url_base_file(url, &fname);
        let checksums = match &s.sha256 {
            Some(sum) => format!("checksums = ['{sum}']"),
            None => {
                warnings.push(
                    "no sha256 on primary source; checksums left as 64-zero placeholder".into(),
                );
                format!("checksums = ['{}']", "0".repeat(64))
            }
        };
        return (
            format!("source_urls = ['{base}']"),
            format!("sources = ['{file}']"),
            checksums,
        );
    }

    // Multi-source: emit EasyBuild list-of-dicts form so every foreign URL lands.
    warnings.push(format!(
        "emitting {} foreign sources as EasyBuild multi-source list",
        usable.len()
    ));
    let mut source_entries = Vec::new();
    let mut checksum_entries = Vec::new();
    for s in &usable {
        let url = s.url.as_ref().or(s.git.as_ref()).unwrap();
        let fname = s
            .filename
            .clone()
            .unwrap_or_else(|| filename_from_url(url));
        let (base, file) = split_url_base_file(url, &fname);
        source_entries.push(format!(
            "    {{\n        'source_urls': ['{base}'],\n        'filename': '{file}',\n    }},"
        ));
        let sum = s
            .sha256
            .clone()
            .unwrap_or_else(|| "0".repeat(64));
        if s.sha256.is_none() {
            warnings.push(format!(
                "source {file} has no sha256; using zero placeholder"
            ));
        }
        checksum_entries.push(format!("    '{sum}',"));
    }
    (
        "source_urls = []  # per-source source_urls inside sources entries".into(),
        format!("sources = [\n{}\n]", source_entries.join("\n")),
        format!("checksums = [\n{}\n]", checksum_entries.join("\n")),
    )
}

/// Map foreign dependency package names toward EasyBuild-style names.
fn map_dep_name_to_eb(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "python" => "Python".into(),
        "cmake" => "CMake".into(),
        "ninja" => "Ninja".into(),
        "meson" => "Meson".into(),
        "pkg-config" | "pkgconf" => "pkgconf".into(),
        "boost" => "Boost".into(),
        "hdf5" => "HDF5".into(),
        "eigen" => "Eigen".into(),
        "highway" => "Highway".into(),
        "libinih" | "inih" => "inih".into(),
        "gmake" | "make" => "make".into(),
        "capnproto" => "CapnProto".into(),
        "py-pyyaml" | "pyyaml" => "PyYAML".into(),
        "py-numpy" | "numpy" => "numpy".into(),
        "libgcc-ng" | "libstdcxx-ng" => name.to_string(), // residual CF runtime
        "libmetatomic-torch" => "metatomic-torch".into(),
        "libmetatensor-torch" => "metatensor-torch".into(),
        "libmetatensor" => "metatensor".into(),
        "libtorch" | "pytorch" => "PyTorch".into(),
        "readcon-core" => "readcon-core".into(),
        other => other.to_string(),
    }
}

/// Residual version for emit: exact pin, else open-range floor (`1.8.0:`, `>=1.2`).
fn residual_version_token(pin: Option<&str>) -> Option<String> {
    let p = pin?;
    if let Some(exact) = exact_version_token(p) {
        return Some(exact);
    }
    let p = p.trim();
    // Spack open range "1.8.0:" or "3.11:"
    if let Some(floor) = p.strip_suffix(':') {
        let floor = floor.trim();
        if !floor.is_empty()
            && floor
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Some(floor.to_string());
        }
    }
    // conda ">=0.1.15,<0.2" or ">=1.2"
    if let Some(rest) = p.strip_prefix(">=") {
        let floor = rest.split(&[',', ' '][..]).next()?.trim();
        if !floor.is_empty() {
            return Some(floor.to_string());
        }
    }
    if let Some(rest) = p.strip_prefix('>') {
        if !rest.starts_with('=') {
            let floor = rest.split(&[',', ' '][..]).next()?.trim();
            if !floor.is_empty() {
                return Some(floor.to_string());
            }
        }
    }
    None
}

/// Turn `https://github.com/org/repo.git` + `v4.3.0` into an archive tarball URL.
fn github_archive_url(git: &str, tag: &str) -> Option<String> {
    let g = git.trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = g.strip_prefix("https://github.com/") {
        return Some(format!(
            "https://github.com/{rest}/archive/refs/tags/{tag}.tar.gz"
        ));
    }
    if let Some(rest) = g.strip_prefix("http://github.com/") {
        return Some(format!(
            "https://github.com/{rest}/archive/refs/tags/{tag}.tar.gz"
        ));
    }
    if let Some(rest) = g.strip_prefix("git@github.com:") {
        let rest = rest.trim_end_matches(".git");
        return Some(format!(
            "https://github.com/{rest}/archive/refs/tags/{tag}.tar.gz"
        ));
    }
    None
}

fn filename_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("source.tar.gz")
        .to_string()
}

fn split_url_base_file(url: &str, fname: &str) -> (String, String) {
    if let Some(pos) = url.rfind('/') {
        let base = url[..=pos].to_string();
        let file = url[pos + 1..].to_string();
        if file.is_empty() {
            (base, fname.to_string())
        } else {
            (base, file)
        }
    } else {
        (url.to_string(), fname.to_string())
    }
}

/// Exact version token from a pin (`1.2.3`, `==1.2.3`) — not ranges.
///
/// Spack open ranges (`1.8.0:`) and conda range ops are rejected so we do not
/// pretend a lower bound is an EB-generation pin.
fn exact_version_token(pin: &str) -> Option<String> {
    let p = pin.trim();
    if p.is_empty() || p == "*" {
        return None;
    }
    // Spack-style foo@1.2: strip leading @
    let p = p.strip_prefix('@').unwrap_or(p);
    let p = p.strip_prefix("==").unwrap_or(p).trim();
    if p.starts_with('>')
        || p.starts_with('<')
        || p.starts_with('!')
        || p.contains(',')
        || p.ends_with(':')
        || p.contains(':')
    {
        // open range (`1.8.0:`) or other non-exact constraint
        return None;
    }
    if p.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && p.chars().any(|c| c.is_ascii_digit())
    {
        Some(p.to_string())
    } else {
        None
    }
}

fn escape_py_single(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn escape_py_triple(s: &str) -> String {
    s.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"")
}

// ===========================================================================
// Conda-forge / rattler-build
// ===========================================================================

fn parse_conda_forge(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut notes = Vec::new();
    let (expanded, ctx_notes) = expand_conda_templates(text);
    notes.extend(ctx_notes);

    let yaml: YamlValue = serde_yaml::from_str(&expanded).map_err(|e| {
        ForeignError::Parse(format!(
            "conda YAML parse after template expand: {e}; first 200 chars: {:?}",
            expanded.chars().take(200).collect::<String>()
        ))
    })?;

    let map = yaml
        .as_mapping()
        .ok_or_else(|| ForeignError::Parse("conda: top-level must be a mapping".into()))?;

    let package = map
        .get(YamlValue::from("package"))
        .and_then(|v| v.as_mapping());
    let name = package
        .and_then(|p| p.get(YamlValue::from("name")))
        .and_then(yaml_as_string)
        .ok_or_else(|| ForeignError::Parse("conda: missing package.name".into()))?;
    let version = package
        .and_then(|p| p.get(YamlValue::from("version")))
        .and_then(yaml_as_string)
        .ok_or_else(|| ForeignError::Parse("conda: missing package.version".into()))?;

    if version.contains("{{") || version.contains("${{") {
        return Err(ForeignError::Parse(format!(
            "conda: package.version still has unexpanded template: {version}"
        )));
    }

    let sources = parse_conda_sources(map.get(YamlValue::from("source")));
    if sources.is_empty() {
        notes.push("conda: no source entries extracted".into());
    } else if sources.len() > 1 {
        notes.push(format!(
            "conda: {} source entries (multi-source recipe)",
            sources.len()
        ));
    }

    let about = map
        .get(YamlValue::from("about"))
        .and_then(|v| v.as_mapping());
    let homepage = about
        .and_then(|a| {
            a.get(YamlValue::from("homepage"))
                .or_else(|| a.get(YamlValue::from("home")))
        })
        .and_then(yaml_as_string);
    let summary = about
        .and_then(|a| a.get(YamlValue::from("summary")))
        .and_then(yaml_as_string);

    let mut dependencies = Vec::new();
    if let Some(req) = map
        .get(YamlValue::from("requirements"))
        .and_then(|v| v.as_mapping())
    {
        for (section, role) in [("build", "build"), ("host", "host"), ("run", "run")] {
            if let Some(list) = req.get(YamlValue::from(section)).and_then(|v| v.as_sequence()) {
                for item in list {
                    for raw in flatten_conda_req_item(item) {
                        if let Some(dep) = parse_conda_dep_line(&raw, role) {
                            if is_conda_compiler_macro(&dep.name) {
                                notes.push(format!(
                                    "skipped conda compiler/stdlib macro: {}",
                                    dep.name
                                ));
                                continue;
                            }
                            if dep.name.starts_with("if:") || dep.name == "then" || dep.name == "else"
                            {
                                continue;
                            }
                            dependencies.push(dep);
                        }
                    }
                }
            }
        }
    }

    let primary = sources.first().cloned().unwrap_or_default();
    Ok(ForeignRecipe {
        format: ForeignFormat::CondaForge,
        name: sanitize_pkg_name(&name),
        version: version.trim().to_string(),
        homepage,
        source_url: primary.url.clone(),
        source_filename: primary.filename.clone(),
        sha256: primary.sha256.clone(),
        sources,
        summary,
        dependencies,
        build_system_hints: Vec::new(),
        configopts: None,
        notes,
    })
}

/// Expand `{% set %}`, `context:` scalars, and simple `${{ x }}` / `{{ x }}` / `|lower`.
fn expand_conda_templates(text: &str) -> (String, Vec<String>) {
    let mut notes = Vec::new();
    let mut vars: HashMap<String, String> = HashMap::new();

    // Classic: {% set name = "zlib" %}  or {% set version = '1.2' %}
    let set_re =
        Regex::new(r#"\{%\s*set\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*[\"']([^\"']*)[\"']\s*%\}"#)
            .expect("set re");
    for c in set_re.captures_iter(text) {
        vars.insert(c[1].to_string(), c[2].to_string());
    }

    // rattler context: block — simple scalar keys only
    if let Some(ctx_start) = Regex::new(r"(?m)^context\s*:\s*$")
        .ok()
        .and_then(|r| r.find(text))
    {
        let rest = &text[ctx_start.end()..];
        for line in rest.lines() {
            if Regex::new(r"^[A-Za-z_]").ok().is_some_and(|r| r.is_match(line))
                && line.contains(':')
                && !line.starts_with(' ')
                && !line.starts_with('\t')
            {
                break;
            }
            if let Some(c) =
                Regex::new(r#"^[ \t]+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[\"']?([^\"'#\n]+?)[\"']?\s*(?:#.*)?$"#)
                    .ok()
                    .and_then(|r| r.captures(line))
            {
                let k = c[1].to_string();
                let v = c[2].trim().to_string();
                // skip nested structures
                if !v.is_empty() && v != "|" && v != ">" {
                    vars.insert(k, v);
                }
            }
        }
    }

    if !vars.is_empty() {
        notes.push(format!(
            "expanded {} template variable(s): {}",
            vars.len(),
            vars.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    let mut out = text.to_string();
    // Remove {% set ... %} lines after capture
    out = set_re.replace_all(&out, "").to_string();

    // Replace longer keys first
    let mut keys: Vec<_> = vars.keys().cloned().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in keys {
        let v = vars.get(&k).unwrap();
        let v_lower = v.to_ascii_lowercase();
        // ${{ var }}  ${{ var|lower }}
        for (pat, rep) in [
            (format!("${{{{ {k} }}}}"), v.as_str()),
            (format!("${{{{{k}}}}}"), v.as_str()),
            (format!("${{{{ {k}|lower }}}}"), v_lower.as_str()),
            (format!("${{{{{k}|lower}}}}"), v_lower.as_str()),
            (format!("{{{{ {k} }}}}"), v.as_str()),
            (format!("{{{{{k}}}}}"), v.as_str()),
            (format!("{{{{ {k}|lower }}}}"), v_lower.as_str()),
            (format!("{{{{{k}|lower}}}}"), v_lower.as_str()),
        ] {
            out = out.replace(&pat, rep);
        }
    }

    if out.contains("{{") || out.contains("${{") {
        notes.push(
            "residual Jinja/template constructs remain after expand (compiler macros, selectors)"
                .into(),
        );
    }

    (out, notes)
}

fn parse_conda_sources(source_val: Option<&YamlValue>) -> Vec<ForeignSource> {
    let Some(v) = source_val else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match v {
        YamlValue::Sequence(seq) => {
            for item in seq {
                if let Some(s) = foreign_source_from_yaml_map(item) {
                    out.push(s);
                }
            }
        }
        YamlValue::Mapping(_) => {
            if let Some(s) = foreign_source_from_yaml_map(v) {
                out.push(s);
            }
        }
        _ => {}
    }
    out
}

fn foreign_source_from_yaml_map(v: &YamlValue) -> Option<ForeignSource> {
    let m = v.as_mapping()?;
    let url = m.get(YamlValue::from("url")).and_then(yaml_as_string);
    let git = m
        .get(YamlValue::from("git_url"))
        .or_else(|| m.get(YamlValue::from("git")))
        .and_then(yaml_as_string);
    let filename = m.get(YamlValue::from("fn")).and_then(yaml_as_string);
    let sha256 = m.get(YamlValue::from("sha256")).and_then(yaml_as_string);
    let tag = m.get(YamlValue::from("tag")).and_then(yaml_as_string);
    if url.is_none() && git.is_none() && sha256.is_none() {
        return None;
    }
    Some(ForeignSource {
        url,
        filename,
        sha256,
        git,
        tag,
        commit: m.get(YamlValue::from("git_rev")).and_then(yaml_as_string),
    })
}

/// Flatten a requirements list item into package match strings.
fn flatten_conda_req_item(item: &YamlValue) -> Vec<String> {
    match item {
        YamlValue::String(s) => vec![s.clone()],
        YamlValue::Mapping(m) => {
            // Selector form: { if: ..., then: "pkg" | [..], else: ... }
            let mut out = Vec::new();
            for key in ["then", "else"] {
                if let Some(v) = m.get(YamlValue::from(key)) {
                    out.extend(flatten_conda_req_item(v));
                }
            }
            // bare string values that look like package matches
            if out.is_empty() {
                for (k, v) in m {
                    if let Some(ks) = k.as_str() {
                        if ks == "if" {
                            continue;
                        }
                    }
                    out.extend(flatten_conda_req_item(v));
                }
            }
            out
        }
        YamlValue::Sequence(seq) => seq.iter().flat_map(flatten_conda_req_item).collect(),
        _ => Vec::new(),
    }
}

fn yaml_as_string(v: &YamlValue) -> Option<String> {
    match v {
        YamlValue::String(s) => Some(s.clone()),
        YamlValue::Number(n) => Some(n.to_string()),
        YamlValue::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse_conda_dep_line(raw: &str, role: &str) -> Option<ForeignDep> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // "name", "name version", "name >=1.2", "libtorch *cpu*"
    let mut parts = line.split_whitespace();
    let name = parts.next()?.to_string();
    if name.contains("{{") || name.contains("${{") {
        return None;
    }
    let pin = {
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            None
        } else {
            Some(rest.join(" "))
        }
    };
    Some(ForeignDep {
        name: sanitize_pkg_name(&name),
        pin,
        role: role.into(),
    })
}

fn is_conda_compiler_macro(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("compiler(")
        || n.contains("stdlib(")
        || n.starts_with("cross-python")
        || n == "sccache"
}

fn sanitize_pkg_name(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        return "unknown".into();
    }
    n.to_string()
}

// ===========================================================================
// Spack package.py (restricted static parse)
// ===========================================================================

fn parse_spack_package(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut notes = Vec::new();
    notes.push("Spack package.py: restricted static parse (no Python execution)".into());

    // class Name(Base) or class Name(Base1, Base2, ...)
    let class_re = Regex::new(
        r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)\s*:",
    )
    .map_err(|e| ForeignError::Parse(e.to_string()))?;
    let class_cap = class_re
        .captures(text)
        .ok_or_else(|| ForeignError::Parse("spack: no class Name(...): found".into()))?;
    let class_name = class_cap.get(1).unwrap().as_str();
    let bases_raw = class_cap.get(2).unwrap().as_str();
    let bases: Vec<String> = bases_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let name = spack_class_to_pkg_name(class_name);
    let mut build_system_hints = bases.clone();
    notes.push(format!(
        "class {class_name} bases: {}",
        bases.join(", ")
    ));

    let homepage = spack_string_attr(text, "homepage");
    let url = spack_string_attr(text, "url");
    let git = spack_string_attr(text, "git");

    // version("X", sha256="...", tag="...", commit="...", url="...")
    let versions = parse_spack_versions(text)?;
    if versions.is_empty() {
        return Err(ForeignError::Parse(
            "spack: no version(\"...\") directives found".into(),
        ));
    }

    let preferred = pick_preferred_spack_version(&versions);
    notes.push(format!(
        "preferred version {} (from {} version() directives)",
        preferred.version,
        versions.len()
    ));
    if versions.len() > 1 {
        notes.push(format!(
            "additional versions not emitted: {}",
            versions
                .iter()
                .filter(|v| v.version != preferred.version)
                .take(8)
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let dependencies = parse_spack_depends_on(text, &mut notes);

    // resource() urls as extra notes (not primary package source)
    let res_re = Regex::new(
        r#"(?m)resource\s*\(\s*[^)]*url\s*=\s*[\"']([^\"']+)[\"']"#,
    )
    .ok();
    if let Some(re) = res_re {
        let n = re.find_iter(text).count();
        if n > 0 {
            notes.push(format!(
                "{n} resource() fetch(es) present — not folded into primary sources"
            ));
        }
    }

    // Keep http(s) `url` separate from `git` so tag→archive materialization can run.
    let mut sources = vec![ForeignSource {
        url: preferred.url.clone().or_else(|| url.clone()),
        filename: None,
        sha256: preferred.sha256.clone(),
        git: git.clone(),
        tag: preferred.tag.clone(),
        commit: preferred.commit.clone(),
    }];
    if sources[0].url.is_none() && sources[0].git.is_none() {
        sources[0].git = git.clone();
    }

    let summary = spack_docstring(text);

    // Meson/CMake from bases
    if build_system_hints.is_empty() {
        build_system_hints = bases;
    }

    let configopts = extract_spack_config_flags(text);
    if let Some(ref c) = configopts {
        notes.push(format!(
            "extracted {} configure flag token(s) from meson_args/cmake_args",
            c.split_whitespace().count()
        ));
    }

    Ok(ForeignRecipe {
        format: ForeignFormat::Spack,
        name,
        version: preferred.version.clone(),
        homepage,
        source_url: sources[0].url.clone(),
        source_filename: None,
        sha256: preferred.sha256.clone(),
        sources,
        summary,
        dependencies,
        build_system_hints,
        configopts,
        notes,
    })
}

#[derive(Debug, Clone)]
struct SpackVersion {
    version: String,
    sha256: Option<String>,
    tag: Option<String>,
    commit: Option<String>,
    url: Option<String>,
}

fn parse_spack_versions(text: &str) -> Result<Vec<SpackVersion>, ForeignError> {
    // Match version( ... ) possibly multi-line up to closing paren at depth 0
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, _)) = chars.peek().copied() {
        if text[i..].starts_with("version(") || text[i..].starts_with("version (") {
            let start = if text[i..].starts_with("version (") {
                i + "version (".len()
            } else {
                i + "version(".len()
            };
            // find matching close paren
            let mut depth = 1;
            let mut j = start;
            let bytes = text.as_bytes();
            while j < text.len() && depth > 0 {
                match bytes[j] as char {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    '"' | '\'' => {
                        let q = bytes[j];
                        j += 1;
                        while j < text.len() && bytes[j] != q {
                            if bytes[j] == b'\\' {
                                j += 1;
                            }
                            j += 1;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let inner = &text[start..j.saturating_sub(1)];
            if let Some(v) = parse_spack_version_call(inner) {
                out.push(v);
            }
            // advance
            while chars.peek().map(|(k, _)| *k < j).unwrap_or(false) {
                chars.next();
            }
        } else {
            chars.next();
        }
    }
    Ok(out)
}

fn parse_spack_version_call(inner: &str) -> Option<SpackVersion> {
    // First positional string is the version id
    let ver_re = Regex::new(r#"^\s*[\"']([^\"']+)[\"']"#).ok()?;
    let version = ver_re.captures(inner)?.get(1)?.as_str().to_string();
    let kw = |key: &str| -> Option<String> {
        let re = Regex::new(&format!(
            r#"{key}\s*=\s*[\"']([^\"']*)[\"']"#
        ))
        .ok()?;
        re.captures(inner)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
    };
    Some(SpackVersion {
        version,
        sha256: kw("sha256"),
        tag: kw("tag"),
        commit: kw("commit"),
        url: kw("url"),
    })
}

fn pick_preferred_spack_version(versions: &[SpackVersion]) -> SpackVersion {
    let is_floating = |v: &str| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "develop" | "main" | "master" | "head" | "stable" | "latest"
        )
    };
    versions
        .iter()
        .find(|v| !is_floating(&v.version))
        .or_else(|| versions.first())
        .cloned()
        .expect("versions non-empty")
}

fn parse_spack_depends_on(text: &str, notes: &mut Vec<String>) -> Vec<ForeignDep> {
    let mut out = Vec::new();
    // Scan depends_on( ... ) with paren matching
    let mut i = 0;
    let b = text.as_bytes();
    while i < text.len() {
        if text[i..].starts_with("depends_on(") || text[i..].starts_with("depends_on (") {
            let start = if text[i..].starts_with("depends_on (") {
                i + "depends_on (".len()
            } else {
                i + "depends_on(".len()
            };
            let mut depth = 1;
            let mut j = start;
            while j < text.len() && depth > 0 {
                match b[j] as char {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    '"' | '\'' => {
                        let q = b[j];
                        j += 1;
                        while j < text.len() && b[j] != q {
                            if b[j] == b'\\' {
                                j += 1;
                            }
                            j += 1;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let inner = &text[start..j.saturating_sub(1)];
            if let Some(dep) = parse_spack_depends_on_call(inner, notes) {
                out.push(dep);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // Dedupe by name: keep the entry with the richest residual pin (last wins when
    // Spack lists version-gated depends_on from oldest to newest).
    let mut by_name: std::collections::HashMap<String, ForeignDep> =
        std::collections::HashMap::new();
    for d in out {
        by_name
            .entry(d.name.clone())
            .and_modify(|prev| {
                // Prefer a pin when the previous had none; otherwise keep the later
                // (usually tighter / newer when= branch listed later in package.py).
                if d.pin.is_some() {
                    *prev = d.clone();
                }
            })
            .or_insert(d);
    }
    by_name.into_values().collect()
}

fn parse_spack_depends_on_call(inner: &str, notes: &mut Vec<String>) -> Option<ForeignDep> {
    let spec_re = Regex::new(r#"^\s*[\"']([^\"']+)[\"']"#).ok()?;
    let spec = spec_re.captures(inner)?.get(1)?.as_str();
    let (dep_name, pin) = split_spack_spec(spec);

    if matches!(dep_name.as_str(), "c" | "cxx" | "fortran") {
        notes.push(format!("skipped language virtual depends_on({spec})"));
        return None;
    }

    // type="build" or type=("build", "run") or type=["build","run"]
    let role = {
        if let Some(c) = Regex::new(r#"type\s*=\s*[\"']([^\"']+)[\"']"#)
            .ok()
            .and_then(|r| r.captures(inner))
        {
            c.get(1).unwrap().as_str().to_string()
        } else if let Some(c) = Regex::new(r#"type\s*=\s*[\(\[]([^\)\]]+)[\)\]]"#)
            .ok()
            .and_then(|r| r.captures(inner))
        {
            // join multiple types
            let inner_types = c.get(1).unwrap().as_str();
            let types: Vec<String> = Regex::new(r#"[\"']([^\"']+)[\"']"#)
                .ok()
                .map(|r| {
                    r.captures_iter(inner_types)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if types.is_empty() {
                "run".into()
            } else {
                types.join("+")
            }
        } else {
            "run".into()
        }
    };

    // when= is residual note only
    if inner.contains("when=") {
        notes.push(format!(
            "depends_on({spec}) has when= clause — dep recorded unconditionally"
        ));
    }

    Some(ForeignDep {
        name: spack_dep_to_eb_name(&dep_name),
        pin,
        role,
    })
}

fn spack_class_to_pkg_name(class_name: &str) -> String {
    // Spack: directory name is usually hyphenated; class PyFoo -> py-foo
    if let Some(rest) = class_name.strip_prefix("Py") {
        if !rest.is_empty() && rest.starts_with(|c: char| c.is_uppercase()) {
            return format!("py-{}", camel_to_kebab(rest));
        }
    }
    // R packages: RFoo -> r-foo is less common; keep camel_to_kebab
    camel_to_kebab(class_name)
}

fn camel_to_kebab(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                if prev.is_lowercase() || next_lower {
                    out.push('-');
                }
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn spack_dep_to_eb_name(name: &str) -> String {
    name.to_string()
}

fn split_spack_spec(spec: &str) -> (String, Option<String>) {
    // foo@1.2.3, foo@1.2: +bar, foo+bar
    if let Some((n, rest)) = spec.split_once('@') {
        let ver = rest.split(['+', '~', '%']).next().unwrap_or(rest);
        (n.to_string(), Some(ver.to_string()))
    } else {
        let n = spec.split(['+', '~', '%']).next().unwrap_or(spec);
        (n.to_string(), None)
    }
}

fn spack_string_attr(text: &str, attr: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?m)^\s*{attr}\s*=\s*[\"']([^\"']+)[\"']"#
    ))
    .ok()?;
    re.captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

fn spack_docstring(text: &str) -> Option<String> {
    let re =
        Regex::new(r#"(?s)class\s+\w+\s*\([^)]*\)\s*:\s*(?:r)?\"\"\"(.+?)\"\"\""#).ok()?;
    re.captures(text).and_then(|c| {
        c.get(1)
            .map(|m| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))
    })
}

/// Write ingest result using letter/name layout when `out` is a directory.
pub fn write_ingest_result(
    result: &IngestResult,
    toolchain: &Toolchain,
    out: Option<&Path>,
    out_dir: Option<&Path>,
) -> Result<std::path::PathBuf, ForeignError> {
    let path = if let Some(p) = out {
        p.to_path_buf()
    } else if let Some(dir) = out_dir {
        let letter = easyconfig_letter_dir(&result.recipe.name);
        let sub = dir.join(&letter).join(&result.recipe.name);
        std::fs::create_dir_all(&sub)
            .map_err(|e| ForeignError::Io(format!("mkdir {}: {e}", sub.display())))?;
        let base = companion_easyconfig_basename(
            &result.recipe.name,
            &result.recipe.version,
            toolchain,
            None,
        );
        sub.join(base)
    } else {
        std::path::PathBuf::from(&result.filename)
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ForeignError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(&path, &result.text)
        .map_err(|e| ForeignError::Io(format!("write {}: {e}", path.display())))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONDA_ZLIB: &str = r#"
package:
  name: zlib
  version: 1.3.1
source:
  url: https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz
  sha256: 9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23
requirements:
  build:
    - make
  run:
    - libgcc-ng
about:
  home: https://zlib.net/
  summary: zlib compression library
"#;

    const SPACK_ZLIB: &str = r#"
class Zlib(Package):
    """A free compression library."""

    homepage = "https://zlib.net"
    url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"

    version("1.3.1", sha256="9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23")
    version("1.2.13", sha256="b3a24de97a8fdbc835b9833169501030b8977031bcb54b3b3ac13740f846ab30")

    depends_on("c", type="build")
    depends_on("gmake", type="build")
"#;

    const SPACK_MULTI_BASE: &str = r#"
class Qmcpack(CMakePackage, CudaPackage):
    """QMCPACK."""

    homepage = "https://www.qmcpack.org/"
    git = "https://github.com/QMCPACK/qmcpack.git"

    version("develop")
    version("4.3.0", tag="v4.3.0", commit="bb7eede051f98ec03296664b304982e655f960c4")
    version("4.2.0", tag="v4.2.0", commit="44a7f7e99a5770ea368b8ea35b181329606bc343")

    depends_on("c", type="build")
    depends_on("cxx", type="build")
    depends_on("cmake@3.17.0:", when="@3.16.0:", type="build")
    depends_on("boost@1.61.0:+exception+serialization+random", when="@3.6.0:", type="build")
    depends_on("libxml2")
    depends_on("mpi", when="+mpi")
    depends_on("python@3.10:", when="@4.3:", type=("build", "run", "test"))
    depends_on("hdf5~mpi", when="~phdf5")
    depends_on("blas")
    depends_on("lapack")
"#;

    const CONDA_RATTLE_EON_MIN: &str = r#"
context:
  version: "2.16.0"

package:
  name: eon
  version: ${{ version }}

source:
  - url: https://github.com/TheochemUI/eOn/releases/download/v${{ version }}/eon-v${{ version }}.tar.xz
    sha256: 3d4da89a393c8821bf370cb97c9d2403718d83f9cbb5e8b918cd90af14ed52dc
  - url: https://github.com/OmniPotentRPC/rgpot/archive/refs/tags/v2.2.1.tar.gz
    sha256: d4687bc719e19174e89288dd16dd45d7a8645d7205c7f8d8fc4d677266055918
    target_directory: subprojects/rgpot

requirements:
  build:
    - ${{ compiler('c') }}
    - cmake
    - ninja
    - meson
  host:
    - python
    - numpy
    - xtb
    - libmetatomic-torch >=0.1.15,<0.2
  run:
    - python
    - xtb
    - quill

about:
  homepage: https://eondocs.org/
  summary: "Algorithms for long time scales"
"#;

    fn foss() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        }
    }

    #[test]
    fn parse_conda_zlib_fields() {
        let r = parse_conda_forge(CONDA_ZLIB).expect("conda");
        assert_eq!(r.name, "zlib");
        assert_eq!(r.version, "1.3.1");
        assert!(r.source_url.as_ref().unwrap().contains("zlib-1.3.1.tar.gz"));
        assert_eq!(
            r.sha256.as_deref(),
            Some("9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23")
        );
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"make"), "{names:?}");
        assert!(names.contains(&"libgcc-ng"), "{names:?}");
    }

    #[test]
    fn parse_conda_rattler_context_and_multi_source() {
        let r = parse_conda_forge(CONDA_RATTLE_EON_MIN).expect("rattler eon");
        assert_eq!(r.name, "eon");
        assert_eq!(r.version, "2.16.0");
        assert_eq!(r.sources.len(), 2, "multi-source: {:?}", r.sources);
        assert!(
            r.source_url
                .as_ref()
                .unwrap()
                .contains("eon-v2.16.0.tar.xz"),
            "{:?}",
            r.source_url
        );
        assert!(
            r.sha256.as_ref().unwrap().starts_with("3d4da89a"),
            "{:?}",
            r.sha256
        );
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"meson"), "{names:?}");
        assert!(names.contains(&"xtb"), "{names:?}");
        assert!(names.contains(&"libmetatomic-torch"), "{names:?}");
        assert!(
            !names.iter().any(|n| n.contains("compiler")),
            "compiler macros skipped: {names:?}"
        );
        assert_eq!(r.homepage.as_deref(), Some("https://eondocs.org/"));
    }

    #[test]
    fn parse_spack_zlib_fields() {
        let r = parse_spack_package(SPACK_ZLIB).expect("spack");
        assert_eq!(r.name, "zlib");
        assert_eq!(r.version, "1.3.1");
        assert!(r.source_url.as_ref().unwrap().contains("zlib-1.3.1"));
        assert!(r.sha256.is_some());
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"gmake"), "{names:?}");
        assert!(!names.iter().any(|n| *n == "c"), "language virtual skipped");
    }

    #[test]
    fn parse_spack_multi_base_and_prefer_non_develop() {
        let r = parse_spack_package(SPACK_MULTI_BASE).expect("qmcpack-like");
        assert_eq!(r.name, "qmcpack");
        assert_eq!(r.version, "4.3.0", "skip develop");
        assert!(
            r.build_system_hints.iter().any(|h| h.contains("CMake")),
            "{:?}",
            r.build_system_hints
        );
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"boost"), "{names:?}");
        assert!(names.contains(&"hdf5"), "{names:?}");
        assert!(names.contains(&"python"), "{names:?}");
        assert!(names.contains(&"libxml2"), "{names:?}");
        // multi-type
        let py = r.dependencies.iter().find(|d| d.name == "python").unwrap();
        assert!(
            py.role.contains("build") && py.role.contains("run"),
            "role={}",
            py.role
        );
    }

    #[test]
    fn emit_conda_tracks_fixture_and_reparses() {
        let r = parse_conda_forge(CONDA_ZLIB).unwrap();
        let out = emit_easyconfig_from_foreign(&r, &foss());
        assert!(out.text.contains("name = 'zlib'"));
        assert!(out.text.contains("version = '1.3.1'"));
        assert!(out.text.contains("zlib-1.3.1.tar.gz"));
        let resolved = crate::eb_parse::resolve_easyconfig_str(&out.text)
            .expect("emitted eb must re-parse");
        assert_eq!(resolved.name, "zlib");
        assert_eq!(resolved.version, "1.3.1");
    }

    #[test]
    fn emit_spack_meson_hint_sets_easyblock() {
        let text = r#"
class Eon(MesonPackage):
    homepage = "https://eondocs.org/"
    url = "https://example.invalid/eon-2.16.0.tar.xz"
    version("2.16.0", sha256="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    depends_on("meson@1.8.0:", type="build")
    depends_on("eigen@3.4:")
"#;
        let r = parse_spack_package(text).unwrap();
        let out = emit_easyconfig_from_foreign(&r, &foss());
        assert!(
            out.text.contains("easyblock = 'MesonNinja'"),
            "{}",
            out.text
        );
        // Floor pins become residual dep versions + EB name mapping.
        assert!(out.text.contains("('Meson', '1.8.0')"), "{}", out.text);
        assert!(out.text.contains("('Eigen', '3.4')"), "{}", out.text);
        let resolved = crate::eb_parse::resolve_easyconfig_str(&out.text).unwrap();
        assert_eq!(resolved.name, "eon");
        assert_eq!(resolved.version, "2.16.0");
    }

    #[test]
    fn emit_multi_source_lists_all_urls() {
        let r = parse_conda_forge(CONDA_RATTLE_EON_MIN).unwrap();
        let out = emit_easyconfig_from_foreign(&r, &foss());
        assert!(out.text.contains("eon-v2.16.0.tar.xz"), "{}", out.text);
        assert!(out.text.contains("rgpot") || out.text.contains("v2.2.1"), "{}", out.text);
        assert!(
            out.text.contains("sources = ["),
            "multi-source list: {}",
            out.text
        );
        let resolved = crate::eb_parse::resolve_easyconfig_str(&out.text).unwrap();
        assert_eq!(resolved.name, "eon");
        assert!(
            resolved.checksums.len() >= 2,
            "checksums: {}",
            resolved.checksums.len()
        );
    }

    #[test]
    fn github_tag_materializes_archive_url() {
        let text = r#"
class Qmcpack(CMakePackage, CudaPackage):
    homepage = "https://www.qmcpack.org/"
    git = "https://github.com/QMCPACK/qmcpack.git"
    version("4.3.0", tag="v4.3.0", commit="bb7eede051f98ec03296664b304982e655f960c4")
    depends_on("hdf5")
"#;
        let r = parse_spack_package(text).unwrap();
        let out = emit_easyconfig_from_foreign(&r, &foss());
        assert!(
            out.text.contains("archive/refs/tags/v4.3.0.tar.gz")
                || out.text.contains("qmcpack"),
            "{}",
            out.text
        );
        assert!(!out.text.contains("${{"), "no jinja");
        let resolved = crate::eb_parse::resolve_easyconfig_str(&out.text).unwrap();
        assert_eq!(resolved.version, "4.3.0");
    }

    #[test]
    fn detect_format_by_filename() {
        assert_eq!(
            detect_foreign_format(Path::new("/x/meta.yaml")),
            Some(ForeignFormat::CondaForge)
        );
        assert_eq!(
            detect_foreign_format(Path::new("recipe.yaml")),
            Some(ForeignFormat::CondaForge)
        );
        assert_eq!(
            detect_foreign_format(Path::new("package.py")),
            Some(ForeignFormat::Spack)
        );
        assert_eq!(detect_foreign_format(Path::new("foo.eb")), None);
    }
}
