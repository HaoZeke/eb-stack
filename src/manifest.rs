//! Intermediate package manifest: foreign (or EB) → planned SBOM + build config
//! → resolvo SAT pins → mechanical new recipe or version bump.
//!
//! # Pipeline
//!
//! ```text
//! foreign recipe  ──parse──►  PackageManifest
//!                                 │
//!                    ┌────────────┴────────────┐
//!                    ▼                         ▼
//!            BuildConfig               planned CycloneDX-like SBOM
//!                    │                         │
//!                    └────────────┬────────────┘
//!                                 ▼
//!                     IntermediatePlan (JSON)
//!                                 │
//!                     resolvo joint co-select
//!                     (robot easyconfigs universe)
//!                                 ▼
//!                     SolvedManifest (dep pins)
//!                                 │
//!              ┌──────────────────┴──────────────────┐
//!              ▼                                     ▼
//!     create new .eb scaffold              bump existing .eb
//! ```
//!
//! Parsers target **upstream-faithful extraction** of every field eb-stack can
//! use mechanically; residual fields stay explicit in `coverage` / residuals
//! rather than invented.

use crate::domain::{StackLock, Toolchain};
use crate::eb_emit::{easyconfig_filename, emit_next_generation_from_path, EmitParams};
use crate::eb_parse::{
    easyconfig_letter_dir, parse_easyconfig_trees,
};
use crate::foreign::{
    emit_easyconfig_from_foreign, map_dep_name_to_eb_pub, residual_queue_from_ingest,
    ForeignDep, ForeignError, ForeignFormat, ForeignRecipe, ForeignSource, IngestOpts,
    IngestResult, ResidualClaimLadder,
};
use crate::hierarchy::{
    hierarchy_for_with_tree, resolve_dep_versions_for_specs, SourceDepSpec,
};
use crate::select::resolvo_resolve_dep_versions;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Schema version for intermediate plan JSON on disk.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// How completely the foreign/upstream recipe was captured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParserCoverage {
    /// Field paths extracted into the manifest (e.g. `package.name`, `depends_on`).
    pub extracted: Vec<String>,
    /// Upstream constructs observed but not fully modeled (residuals).
    pub residual: Vec<String>,
}

impl ParserCoverage {
    pub fn extracted_count(&self) -> usize {
        self.extracted.len()
    }
    pub fn residual_count(&self) -> usize {
        self.residual.len()
    }
    /// Ratio extracted / (extracted + residual); 1.0 only when residual empty.
    pub fn ratio(&self) -> f64 {
        let e = self.extracted.len() as f64;
        let r = self.residual.len() as f64;
        if e + r == 0.0 {
            0.0
        } else {
            e / (e + r)
        }
    }
}

/// Origin of a package manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestOrigin {
    CondaForge,
    Spack,
    EasyBuild,
}

impl ManifestOrigin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CondaForge => "conda-forge",
            Self::Spack => "spack",
            Self::EasyBuild => "easybuild",
        }
    }
}

impl From<ForeignFormat> for ManifestOrigin {
    fn from(f: ForeignFormat) -> Self {
        match f {
            ForeignFormat::CondaForge => Self::CondaForge,
            ForeignFormat::Spack => Self::Spack,
        }
    }
}

/// One source artifact in the intermediate IR.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ManifestSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Conda `target_directory` / Spack resource destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<String>,
}

impl From<&ForeignSource> for ManifestSource {
    fn from(s: &ForeignSource) -> Self {
        Self {
            url: s.url.clone(),
            filename: s.filename.clone(),
            sha256: s.sha256.clone(),
            git: s.git.clone(),
            tag: s.tag.clone(),
            commit: s.commit.clone(),
            target_directory: s.target_directory.clone(),
        }
    }
}

/// Dependency edge in the intermediate IR (pre-SAT).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestDep {
    /// Name as written upstream.
    pub name: String,
    /// EasyBuild-mapped package name when known.
    pub eb_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// build | host | run | build+run | …
    pub role: String,
    /// After resolvo: co-selected version when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solved_version: Option<String>,
}

/// Build-system / product config (not a full lock).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easyblock_hint: Option<String>,
    #[serde(default)]
    pub build_system_hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configopts: Option<String>,
    pub toolchain: Toolchain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moduleclass_hint: Option<String>,
    #[serde(default)]
    pub variants: Vec<ManifestVariant>,
    #[serde(default)]
    pub patches: Vec<String>,
}

/// Spack/conda variant or feature flag captured as residual or default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestVariant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Full intermediate package identity + edges (pre- or post-solve).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub schema_version: u32,
    pub origin: ManifestOrigin,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub sources: Vec<ManifestSource>,
    #[serde(default)]
    pub dependencies: Vec<ManifestDep>,
    pub build: BuildConfig,
    pub coverage: ParserCoverage,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Resolvo result attached to a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolvedManifest {
    /// name → version for co-selected deps (EB names).
    pub dep_versions: HashMap<String, String>,
    pub engine_note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<StackLock>,
}

/// On-disk intermediate plan: manifest + planned SBOM + optional solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntermediatePlan {
    pub schema_version: u32,
    pub package: PackageManifest,
    /// Planned CycloneDX-like document (pre-build lifecycle).
    pub planned_sbom: Value,
    /// Same as package.build, duplicated at top level for tool consumers.
    pub build_config: BuildConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solved: Option<SolvedManifest>,
    pub claim_ladder: ResidualClaimLadder,
    pub created_at: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("manifest: {0}")]
    Msg(String),
    #[error(transparent)]
    Foreign(#[from] ForeignError),
    #[error("IO: {0}")]
    Io(String),
}

/// Build a [`PackageManifest`] from a parsed foreign recipe (no robot yet).
pub fn package_manifest_from_foreign(
    recipe: &ForeignRecipe,
    toolchain: &Toolchain,
) -> PackageManifest {
    let mut coverage = ParserCoverage::default();
    coverage.extracted.push("name".into());
    coverage.extracted.push("version".into());
    if recipe.homepage.is_some() {
        coverage.extracted.push("homepage".into());
    } else {
        coverage.residual.push("homepage".into());
    }
    if recipe.summary.is_some() {
        coverage.extracted.push("summary/description".into());
    } else {
        coverage.residual.push("summary/description".into());
    }
    if recipe.license.is_some() {
        coverage.extracted.push("license".into());
    } else {
        coverage.residual.push("license".into());
    }
    if !recipe.sources.is_empty() || recipe.source_url.is_some() {
        coverage.extracted.push("sources".into());
    } else {
        coverage.residual.push("sources".into());
    }
    if !recipe.dependencies.is_empty() {
        coverage.extracted.push("dependencies".into());
    }
    if recipe.configopts.is_some() {
        coverage.extracted.push("configopts".into());
    } else {
        coverage.residual.push("product_configopts".into());
    }
    if !recipe.build_system_hints.is_empty() {
        coverage.extracted.push("build_system_hints".into());
    }
    if !recipe.variants.is_empty() {
        coverage.extracted.push("variants".into());
    }
    if !recipe.patches.is_empty() {
        coverage.extracted.push("patches".into());
    } else {
        coverage.residual.push("patches".into());
    }
    for n in &recipe.notes {
        if n.contains("when=") || n.contains("residual") || n.contains("resource()") {
            coverage.residual.push(n.clone());
        }
    }
    // Upstream constructs we never claim fully without Python/EB exec:
    coverage.residual.push("control_flow_selectors".into());
    coverage.residual.push("dynamic_version_compute".into());

    let mut sources: Vec<ManifestSource> = recipe.sources.iter().map(ManifestSource::from).collect();
    if sources.is_empty() {
        sources.push(ManifestSource {
            url: recipe.source_url.clone(),
            filename: recipe.source_filename.clone(),
            sha256: recipe.sha256.clone(),
            ..Default::default()
        });
    }

    let dependencies: Vec<ManifestDep> = recipe
        .dependencies
        .iter()
        .map(|d| ManifestDep {
            name: d.name.clone(),
            eb_name: map_dep_name_to_eb_pub(&d.name),
            pin: d.pin.clone(),
            role: d.role.clone(),
            solved_version: None,
        })
        .collect();

    let easyblock_hint = guess_easyblock_hint(recipe);
    let variants: Vec<ManifestVariant> = recipe
        .variants
        .iter()
        .map(|v| ManifestVariant {
            name: v.name.clone(),
            default: v.default.clone(),
            description: v.description.clone(),
        })
        .collect();

    let build = BuildConfig {
        easyblock_hint: Some(easyblock_hint),
        build_system_hints: recipe.build_system_hints.clone(),
        configopts: recipe.configopts.clone(),
        toolchain: toolchain.clone(),
        moduleclass_hint: Some("lib".into()),
        variants,
        patches: recipe.patches.clone(),
    };

    PackageManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        origin: ManifestOrigin::from(recipe.format),
        name: recipe.name.clone(),
        version: recipe.version.clone(),
        homepage: recipe.homepage.clone(),
        description: recipe
            .description
            .clone()
            .or_else(|| recipe.summary.clone()),
        license: recipe.license.clone(),
        sources,
        dependencies,
        build,
        coverage,
        notes: recipe.notes.clone(),
    }
}

fn guess_easyblock_hint(recipe: &ForeignRecipe) -> String {
    let hints = recipe
        .build_system_hints
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if hints.contains("meson") {
        "MesonNinja".into()
    } else if hints.contains("cmake") {
        "CMakeMake".into()
    } else if hints.contains("python") || hints.contains("pypi") {
        "PythonPackage".into()
    } else if recipe
        .dependencies
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case("meson") || d.name.eq_ignore_ascii_case("ninja"))
    {
        "MesonNinja".into()
    } else if recipe
        .dependencies
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case("cmake"))
    {
        "CMakeMake".into()
    } else {
        "ConfigureMake".into()
    }
}

/// Planned SBOM (CycloneDX-shaped JSON) from a package manifest — pre-install.
pub fn planned_sbom_from_manifest(pkg: &PackageManifest) -> Value {
    let mut components = Vec::new();
    let root_ref = format!("pkg:eb-stack/{}@{}", pkg.name, pkg.version);
    components.push(json!({
        "type": "library",
        "bom-ref": root_ref,
        "name": pkg.name,
        "version": pkg.version,
        "description": pkg.description,
        "licenses": pkg.license.as_ref().map(|l| json!([{"license": {"name": l}}])),
        "externalReferences": pkg.homepage.as_ref().map(|h| json!([{
            "type": "website",
            "url": h
        }])),
        "properties": [
            {"name": "eb_stack:origin", "value": pkg.origin.as_str()},
            {"name": "eb_stack:easyblock_hint", "value": pkg.build.easyblock_hint.clone().unwrap_or_default()},
            {"name": "eb_stack:toolchain", "value": pkg.build.toolchain.label()},
            {"name": "eb_stack:lifecycle", "value": "pre-build"},
        ]
    }));
    for d in &pkg.dependencies {
        let ver = d
            .solved_version
            .clone()
            .or_else(|| d.pin.clone())
            .unwrap_or_else(|| "*".into());
        components.push(json!({
            "type": "library",
            "bom-ref": format!("pkg:eb-stack/{}@{}", d.eb_name, ver),
            "name": d.eb_name,
            "version": ver,
            "properties": [
                {"name": "eb_stack:role", "value": d.role},
                {"name": "eb_stack:foreign_name", "value": d.name},
                {"name": "eb_stack:foreign_pin", "value": d.pin.clone().unwrap_or_default()},
            ]
        }));
    }
    let depends_on: Vec<String> = pkg
        .dependencies
        .iter()
        .map(|d| {
            let ver = d
                .solved_version
                .clone()
                .or_else(|| d.pin.clone())
                .unwrap_or_else(|| "*".into());
            format!("pkg:eb-stack/{}@{}", d.eb_name, ver)
        })
        .collect();
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "timestamp": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "tools": [{
                "vendor": "eb-stack",
                "name": "eb-stack",
                "version": env!("CARGO_PKG_VERSION")
            }],
            "component": {
                "type": "application",
                "name": pkg.name,
                "version": pkg.version,
                "bom-ref": root_ref,
            },
            "properties": [
                {"name": "eb_stack:plan", "value": "intermediate"},
                {"name": "eb_stack:parser_coverage_ratio", "value": format!("{:.3}", pkg.coverage.ratio())},
            ]
        },
        "components": components,
        "dependencies": [{
            "ref": root_ref,
            "dependsOn": depends_on
        }]
    })
}

/// Build an intermediate plan (no robot solve yet).
pub fn plan_from_foreign(recipe: &ForeignRecipe, toolchain: &Toolchain) -> IntermediatePlan {
    let package = package_manifest_from_foreign(recipe, toolchain);
    let planned_sbom = planned_sbom_from_manifest(&package);
    let build_config = package.build.clone();
    IntermediatePlan {
        schema_version: MANIFEST_SCHEMA_VERSION,
        package,
        planned_sbom,
        build_config,
        solved: None,
        claim_ladder: ResidualClaimLadder::ingest_default(),
        created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }
}

/// Run hierarchy + resolvo joint co-select; update plan.solved and dep pins.
pub fn solve_plan_with_robot(
    plan: &mut IntermediatePlan,
    opts: &IngestOpts,
) -> Result<(), ManifestError> {
    if opts.easyconfigs.is_empty() {
        return Err(ManifestError::Msg(
            "solve_plan_with_robot requires IngestOpts.easyconfigs".into(),
        ));
    }
    let roots: Vec<&Path> = opts.easyconfigs.iter().map(|p| p.as_path()).collect();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|e| ManifestError::Msg(format!("robot parse: {e}")))?;
    let hierarchy = hierarchy_for_with_tree(
        &plan.build_config.toolchain,
        opts.hierarchy_fixture.as_deref(),
        &tree.candidates,
    )
    .map_err(|e| ManifestError::Msg(format!("hierarchy: {e}")))?;

    let mut specs: Vec<SourceDepSpec> = Vec::new();
    for d in &plan.package.dependencies {
        // Skip toolchain virtuals / noise handled later in emit
        if d.eb_name.is_empty() {
            continue;
        }
        specs.push(SourceDepSpec {
            name: d.eb_name.clone(),
            version: d.pin.clone().unwrap_or_else(|| "0.0.0".into()),
            versionsuffix: None,
            system_toolchain: false,
            optional: false,
        });
    }

    let (hierarchy_pins, _hnote) =
        resolve_dep_versions_for_specs(&specs, &tree.candidates, &hierarchy, opts.keep_old_deps)
            .map_err(|e| ManifestError::Msg(format!("hierarchy resolve: {e}")))?;

    let (dep_versions, engine_note) = resolvo_resolve_dep_versions(
        &specs,
        &tree.candidates,
        &hierarchy,
        &plan.build_config.toolchain,
        &plan.package.name,
        &plan.package.version,
        Some(&hierarchy_pins),
    )
    .unwrap_or_else(|e| {
        (
            hierarchy_pins.clone(),
            format!("resolvo failed ({e}); hierarchy consensus only"),
        )
    });

    for d in &mut plan.package.dependencies {
        if let Some(v) = dep_versions.get(&d.eb_name) {
            d.solved_version = Some(v.clone());
        }
    }
    plan.planned_sbom = planned_sbom_from_manifest(&plan.package);
    plan.solved = Some(SolvedManifest {
        dep_versions,
        engine_note,
        lock: None,
    });
    plan.claim_ladder.resolves =
        "partial — joint resolvo/hierarchy pins when robot had candidates; re-verify with check-recipe"
            .into();
    Ok(())
}

/// Emit a new EasyBuild scaffold from a (optionally solved) plan.
pub fn emit_new_recipe_from_plan(plan: &IntermediatePlan) -> IngestResult {
    let mut recipe = foreign_recipe_from_plan(plan);
    // Apply solved versions into foreign pins so emit writes real tuples.
    if let Some(solved) = &plan.solved {
        for d in &mut recipe.dependencies {
            let eb = map_dep_name_to_eb_pub(&d.name);
            if let Some(v) = solved.dep_versions.get(&eb) {
                d.pin = Some(v.clone());
            }
        }
    }
    let mut result = emit_easyconfig_from_foreign(&recipe, &plan.build_config.toolchain);
    result.warnings.push(format!(
        "emitted from intermediate plan (parser coverage ratio {:.1}%, {} residual field notes)",
        100.0 * plan.package.coverage.ratio(),
        plan.package.coverage.residual_count()
    ));
    if let Some(s) = &plan.solved {
        result.warnings.push(format!("solve: {}", s.engine_note));
    }
    result
}

fn foreign_recipe_from_plan(plan: &IntermediatePlan) -> ForeignRecipe {
    let pkg = &plan.package;
    let format = match pkg.origin {
        ManifestOrigin::CondaForge => ForeignFormat::CondaForge,
        ManifestOrigin::Spack => ForeignFormat::Spack,
        ManifestOrigin::EasyBuild => ForeignFormat::CondaForge, // emit path still works
    };
    let primary = pkg.sources.first();
    ForeignRecipe {
        format,
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        homepage: pkg.homepage.clone(),
        source_url: primary.and_then(|s| s.url.clone()),
        source_filename: primary.and_then(|s| s.filename.clone()),
        sha256: primary.and_then(|s| s.sha256.clone()),
        sources: pkg
            .sources
            .iter()
            .map(|s| ForeignSource {
                url: s.url.clone(),
                filename: s.filename.clone(),
                sha256: s.sha256.clone(),
                git: s.git.clone(),
                tag: s.tag.clone(),
                commit: s.commit.clone(),
                target_directory: s.target_directory.clone(),
            })
            .collect(),
        summary: pkg.description.clone(),
        description: pkg.description.clone(),
        license: pkg.license.clone(),
        dependencies: pkg
            .dependencies
            .iter()
            .map(|d| ForeignDep {
                name: d.name.clone(),
                pin: d.solved_version.clone().or_else(|| d.pin.clone()),
                role: d.role.clone(),
            })
            .collect(),
        build_system_hints: pkg.build.build_system_hints.clone(),
        configopts: pkg.build.configopts.clone(),
        patches: pkg.build.patches.clone(),
        variants: pkg
            .build
            .variants
            .iter()
            .map(|v| crate::foreign::ForeignVariant {
                name: v.name.clone(),
                default: v.default.clone(),
                description: v.description.clone(),
            })
            .collect(),
        notes: pkg.notes.clone(),
    }
}

/// Bump an existing easyconfig using solved dep pins from the plan (mechanical).
pub fn bump_recipe_from_plan(
    plan: &IntermediatePlan,
    source_eb: &Path,
    out_eb: &Path,
) -> Result<String, ManifestError> {
    let solved = plan
        .solved
        .as_ref()
        .ok_or_else(|| ManifestError::Msg("bump_recipe_from_plan requires plan.solved".into()))?;
    let params = EmitParams {
        toolchain: plan.build_config.toolchain.clone(),
        version: Some(plan.package.version.clone()),
        dep_versions: solved.dep_versions.clone(),
        source_checksum: None,
    };
    let result = emit_next_generation_from_path(source_eb, &params)
        .map_err(|e| ManifestError::Msg(format!("bump emit: {e}")))?;
    if let Some(parent) = out_eb.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ManifestError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    std::fs::write(out_eb, &result.text)
        .map_err(|e| ManifestError::Io(format!("write {}: {e}", out_eb.display())))?;
    Ok(result.text)
}

/// End-to-end: foreign path → plan → optional solve → new recipe + JSON artifacts.
pub fn plan_and_emit(
    source: &Path,
    format: Option<ForeignFormat>,
    toolchain: &Toolchain,
    opts: &IngestOpts,
    manifest_out: Option<&Path>,
    sbom_out: Option<&Path>,
    out_eb: Option<&Path>,
    out_dir: Option<&Path>,
    bump_from: Option<&Path>,
) -> Result<(IntermediatePlan, Option<PathBuf>), ManifestError> {
    let mut recipe = crate::foreign::parse_foreign_path(source, format)?;
    if recipe.format == ForeignFormat::Spack {
        if let Ok(text) = std::fs::read_to_string(source) {
            if let Some(flags) = crate::foreign::extract_spack_config_flags_pub(&text) {
                recipe.configopts = Some(flags);
            }
        }
    }
    let mut plan = plan_from_foreign(&recipe, toolchain);
    if !opts.easyconfigs.is_empty() {
        solve_plan_with_robot(&mut plan, opts)?;
    }

    if let Some(p) = manifest_out {
        write_plan_json(p, &plan)?;
    }
    if let Some(p) = sbom_out {
        write_json(p, &plan.planned_sbom)?;
    }

    let eb_path = if let Some(src) = bump_from {
        let dest = out_eb
            .map(PathBuf::from)
            .or_else(|| {
                out_dir.map(|d| {
                    d.join(easyconfig_letter_dir(&plan.package.name))
                        .join(&plan.package.name)
                        .join(easyconfig_filename(
                            &plan.package.name,
                            &plan.package.version,
                            toolchain,
                        ))
                })
            })
            .unwrap_or_else(|| {
                PathBuf::from(easyconfig_filename(
                    &plan.package.name,
                    &plan.package.version,
                    toolchain,
                ))
            });
        bump_recipe_from_plan(&plan, src, &dest)?;
        Some(dest)
    } else if out_eb.is_some() || out_dir.is_some() {
        let result = emit_new_recipe_from_plan(&plan);
        let dest = if let Some(p) = out_eb {
            p.to_path_buf()
        } else {
            let d = out_dir.unwrap();
            d.join(easyconfig_letter_dir(&result.recipe.name))
                .join(&result.recipe.name)
                .join(&result.filename)
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ManifestError::Io(format!("mkdir {}: {e}", parent.display())))?;
        }
        std::fs::write(&dest, &result.text)
            .map_err(|e| ManifestError::Io(format!("write {}: {e}", dest.display())))?;
        // residual queue beside recipe
        let queue_path = dest.with_extension("residuals.json");
        let queue = residual_queue_from_ingest(&result, toolchain);
        crate::foreign::write_residual_queue(&queue_path, &queue)
            .map_err(|e| ManifestError::Msg(e.to_string()))?;
        // also write plan residual coverage into queue notes via re-read not needed
        Some(dest)
    } else {
        None
    };

    Ok((plan, eb_path))
}

fn write_plan_json(path: &Path, plan: &IntermediatePlan) -> Result<(), ManifestError> {
    write_json(path, plan)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), ManifestError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ManifestError::Io(format!("mkdir {}: {e}", parent.display())))?;
    }
    let s = serde_json::to_string_pretty(value)
        .map_err(|e| ManifestError::Msg(format!("serialize: {e}")))?;
    std::fs::write(path, s + "\n")
        .map_err(|e| ManifestError::Io(format!("write {}: {e}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreign::parse_foreign_path;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/foreign_ingest")
    }

    fn foss() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        }
    }

    #[test]
    fn plan_from_conda_eon_has_sources_and_sbom() {
        let path = root().join("conda_eon/recipe.yaml");
        let recipe = parse_foreign_path(&path, None).expect("parse");
        let plan = plan_from_foreign(&recipe, &foss());
        assert_eq!(plan.package.version, "2.16.0");
        assert!(plan.package.sources.len() >= 3);
        assert!(plan.package.coverage.extracted_count() > 0);
        assert_eq!(plan.planned_sbom["bomFormat"], "CycloneDX");
        assert_eq!(plan.planned_sbom["specVersion"], "1.5");
        assert!(plan.planned_sbom["components"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn plan_from_spack_qmcpack_variants() {
        let path = root().join("spack_qmcpack/package.py");
        let recipe = parse_foreign_path(&path, None).expect("parse");
        let plan = plan_from_foreign(&recipe, &foss());
        assert_eq!(plan.package.name, "qmcpack");
        assert_eq!(plan.package.version, "4.3.0");
        // variants extracted after parser expansion
        assert!(
            !plan.build_config.variants.is_empty() || !plan.package.build.variants.is_empty(),
            "expected variants from QMCPACK package.py"
        );
    }

    #[test]
    fn emit_from_plan_reparses() {
        let path = root().join("conda_zlib/meta.yaml");
        let recipe = parse_foreign_path(&path, None).unwrap();
        let plan = plan_from_foreign(&recipe, &foss());
        let out = emit_new_recipe_from_plan(&plan);
        assert!(out.text.contains("name = 'zlib'"));
        assert!(out.text.contains("version = '1.3.1'"));
    }
}
