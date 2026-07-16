//! Canonical foreign-package planning bundle: manifest, SBOM, locks, recipes.

use crate::domain::Toolchain;
use crate::eb_emit::{emit_next_generation_from_path, EmitParams};
use crate::eb_parse::{
    easyconfig_letter_dir, parse_easyconfig_trees, resolve_easyconfig_file, ResolvedDep,
};
use crate::foreign::{parse_foreign_path, ForeignFormat};
use crate::manifest::package_plan_from_foreign;
use crate::package::{
    package_plan_to_cyclonedx, BuildSpec, ConditionExpr, DependencyIntent, DependencyRole,
    OutputRequest, PackageMetadata, PackageOrigin, PackagePlan, PatchArtifact, ProductProfile,
    ProfileLock, Residual, ResidualSeverity, ResidualStage, SourceArtifact, StackPin, StackPinMode,
    StackPolicy, PACKAGE_SCHEMA_VERSION,
};
use crate::package_config::{apply_package_layers, PackageConfigLayer};
use crate::package_emit::{emit_profile_easyconfigs, EmittedEasyconfig};
use crate::package_solve::{solve_package_profile, solve_package_profile_with_hierarchy};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct NewPackageRequest {
    pub source: PathBuf,
    pub format: Option<ForeignFormat>,
    pub toolchain: Toolchain,
    /// Positional SHA-256 overrides, one for every canonical source artifact.
    pub source_checksums: Vec<String>,
    pub package_layers: Vec<PackageConfigLayer>,
    pub easyconfig_roots: Vec<PathBuf>,
    pub stack_policy: StackPolicy,
}

#[derive(Debug, Clone)]
pub struct BumpPackageRequest {
    pub source: PathBuf,
    pub toolchain: Toolchain,
    pub version: Option<String>,
    pub source_checksum: Option<String>,
    pub easyconfig_roots: Vec<PathBuf>,
    pub hierarchy_fixture: Option<PathBuf>,
    pub overrides: HashMap<String, String>,
    pub stack_policy: StackPolicy,
}

#[derive(Debug, Clone)]
pub struct PackageBundle {
    pub plan: PackagePlan,
    pub sbom: Value,
    pub locks: Vec<ProfileLock>,
    pub easyconfigs: Vec<EmittedEasyconfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPackageBundle {
    pub manifest: PathBuf,
    pub sbom: PathBuf,
    pub locks: Vec<PathBuf>,
    pub easyconfigs: Vec<PathBuf>,
    pub patches: Vec<PathBuf>,
}

pub fn inspect_new_package(
    source: &Path,
    format: Option<ForeignFormat>,
    toolchain: &Toolchain,
    package_layers: &[PackageConfigLayer],
) -> Result<(PackagePlan, Value), PackageWorkflowError> {
    let recipe = parse_foreign_path(source, format)
        .map_err(|error| PackageWorkflowError::Foreign(error.to_string()))?;
    let mut plan = package_plan_from_foreign(&recipe, toolchain);
    if !package_layers.is_empty() {
        apply_package_layers(&mut plan, package_layers)
            .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
    }
    refresh_checksum_residuals(&mut plan);
    let sbom = package_plan_to_cyclonedx(&plan)
        .map_err(|error| PackageWorkflowError::Sbom(error.to_string()))?;
    Ok((plan, sbom))
}

pub fn plan_new_package(
    request: &NewPackageRequest,
) -> Result<PackageBundle, PackageWorkflowError> {
    if request.easyconfig_roots.is_empty() {
        return Err(PackageWorkflowError::NoEasyconfigRoots);
    }
    let (mut plan, mut sbom) = inspect_new_package(
        &request.source,
        request.format,
        &request.toolchain,
        &request.package_layers,
    )?;
    if !request.source_checksums.is_empty() {
        apply_source_checksums(&mut plan, &request.source_checksums)?;
        sbom = package_plan_to_cyclonedx(&plan)
            .map_err(|error| PackageWorkflowError::Sbom(error.to_string()))?;
    }
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    let mut locks = Vec::new();
    for output in &plan.outputs {
        locks.push(
            solve_package_profile(
                &plan,
                &output.profile,
                &Default::default(),
                &tree.candidates,
                &request.stack_policy,
            )
            .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?,
        );
    }
    require_source_checksums(&plan)?;
    let easyconfigs = emit_profile_easyconfigs(&plan, &locks)
        .map_err(|error| PackageWorkflowError::Emit(error.to_string()))?;
    Ok(PackageBundle {
        plan,
        sbom,
        locks,
        easyconfigs,
    })
}

fn apply_source_checksums(
    plan: &mut PackagePlan,
    checksums: &[String],
) -> Result<(), PackageWorkflowError> {
    if checksums.len() != plan.sources.len() {
        return Err(PackageWorkflowError::SourceChecksumCount {
            expected: plan.sources.len(),
            actual: checksums.len(),
        });
    }
    for (index, (source, checksum)) in plan.sources.iter_mut().zip(checksums.iter()).enumerate() {
        validate_source_checksum(index, checksum)?;
        source.sha256 = Some(checksum.clone());
    }
    refresh_checksum_residuals(plan);
    Ok(())
}

fn require_source_checksums(plan: &PackagePlan) -> Result<(), PackageWorkflowError> {
    if plan.sources.is_empty() {
        return Err(PackageWorkflowError::NoSourceArtifacts);
    }
    let missing = plan
        .sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| source.sha256.is_none().then_some(index))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(PackageWorkflowError::MissingSourceChecksums(missing));
    }
    for (index, source) in plan.sources.iter().enumerate() {
        validate_source_checksum(index, source.sha256.as_deref().unwrap_or_default())?;
    }
    let missing_patches = plan
        .build
        .patches
        .iter()
        .filter(|patch| patch.sha256.is_none())
        .map(|patch| patch.filename.clone())
        .collect::<Vec<_>>();
    if !missing_patches.is_empty() {
        return Err(PackageWorkflowError::MissingPatchChecksums(missing_patches));
    }
    for patch in &plan.build.patches {
        validate_patch_checksum(patch)?;
        validate_patch_source(patch)?;
    }
    Ok(())
}

fn refresh_checksum_residuals(plan: &mut PackagePlan) {
    plan.residuals.retain(|residual| {
        !matches!(
            residual.id.as_str(),
            "source:missing-sha256" | "patch:missing-sha256" | "patch:missing-source"
        )
    });
    if plan.sources.iter().any(|source| source.sha256.is_none()) {
        plan.residuals.push(Residual {
            id: "source:missing-sha256".into(),
            stage: ResidualStage::Normalize,
            category: "checksum".into(),
            severity: ResidualSeverity::Blocking,
            summary: "one or more source artifacts have no sha256".into(),
            evidence: None,
            provenance: None,
        });
    }
    let missing_patches = plan
        .build
        .patches
        .iter()
        .filter(|patch| patch.sha256.is_none())
        .map(|patch| patch.filename.as_str())
        .collect::<Vec<_>>();
    if !missing_patches.is_empty() {
        plan.residuals.push(Residual {
            id: "patch:missing-sha256".into(),
            stage: ResidualStage::Normalize,
            category: "checksum".into(),
            severity: ResidualSeverity::Blocking,
            summary: "one or more patch artifacts have no sha256".into(),
            evidence: Some(missing_patches.join(", ")),
            provenance: None,
        });
    }
    let missing_patch_sources = plan
        .build
        .patches
        .iter()
        .filter(|patch| patch.resolved_source.is_none() && patch.source.is_none())
        .map(|patch| patch.filename.as_str())
        .collect::<Vec<_>>();
    if !missing_patch_sources.is_empty() {
        plan.residuals.push(Residual {
            id: "patch:missing-source".into(),
            stage: ResidualStage::Normalize,
            category: "patch-asset".into(),
            severity: ResidualSeverity::Blocking,
            summary: "one or more patch artifacts have no source file".into(),
            evidence: Some(missing_patch_sources.join(", ")),
            provenance: None,
        });
    }
}

fn validate_source_checksum(index: usize, checksum: &str) -> Result<(), PackageWorkflowError> {
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackageWorkflowError::InvalidSourceChecksum {
            index,
            checksum: checksum.to_string(),
        });
    }
    Ok(())
}

fn validate_patch_checksum(patch: &PatchArtifact) -> Result<(), PackageWorkflowError> {
    let checksum = patch.sha256.as_deref().unwrap_or_default();
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(PackageWorkflowError::InvalidPatchChecksum {
            filename: patch.filename.clone(),
            checksum: checksum.to_string(),
        });
    }
    Ok(())
}

fn validate_patch_source(patch: &PatchArtifact) -> Result<PathBuf, PackageWorkflowError> {
    let source = patch
        .resolved_source
        .clone()
        .or_else(|| patch.source.as_deref().map(PathBuf::from))
        .ok_or_else(|| PackageWorkflowError::MissingPatchSource(patch.filename.clone()))?;
    let bytes = std::fs::read(&source)
        .map_err(|error| PackageWorkflowError::PatchIo(source.clone(), error))?;
    let digest = Sha256::digest(&bytes);
    let mut actual = String::with_capacity(64);
    for byte in digest {
        write!(&mut actual, "{byte:02x}").expect("writing a SHA-256 digest to String cannot fail");
    }
    let expected = patch.sha256.as_deref().unwrap_or_default();
    if actual != expected {
        return Err(PackageWorkflowError::PatchChecksumMismatch {
            filename: patch.filename.clone(),
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(source)
}

pub fn plan_package_bump(
    request: &BumpPackageRequest,
) -> Result<PackageBundle, PackageWorkflowError> {
    if request.easyconfig_roots.is_empty() {
        return Err(PackageWorkflowError::NoEasyconfigRoots);
    }
    let resolved = resolve_easyconfig_file(&request.source)
        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    let mut plan = package_plan_from_easyconfig(
        &resolved,
        &request.toolchain,
        request.version.as_deref(),
        request.source_checksum.as_deref(),
    );
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    let mut stack_policy = request.stack_policy.clone();
    for (name, version) in &request.overrides {
        stack_policy.pins.retain(|pin| pin.name != *name);
        stack_policy.pins.push(StackPin {
            name: name.clone(),
            version_requirement: format!("=={version}"),
            toolchain: None,
            versionsuffix: None,
            mode: StackPinMode::Locked,
            source: Some("package bump override".into()),
        });
    }
    let lock = solve_package_profile_with_hierarchy(
        &plan,
        "default",
        &Default::default(),
        &tree.candidates,
        &stack_policy,
        request.hierarchy_fixture.as_deref(),
    )
    .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?;
    let dependency_versions = lock
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.clone(), dependency.version.clone()))
        .collect::<HashMap<_, _>>();
    let result = emit_next_generation_from_path(
        &request.source,
        &EmitParams {
            toolchain: request.toolchain.clone(),
            version: request.version.clone(),
            dep_versions: dependency_versions,
            source_checksum: request.source_checksum.clone(),
        },
    )
    .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    for (index, warning) in result.warnings.iter().enumerate() {
        plan.residuals.push(Residual {
            id: format!("bump-warning:{index}"),
            stage: ResidualStage::Emit,
            category: "bump-warning".into(),
            severity: ResidualSeverity::Judgment,
            summary: warning.clone(),
            evidence: None,
            provenance: None,
        });
    }
    let sbom = package_plan_to_cyclonedx(&plan)
        .map_err(|error| PackageWorkflowError::Sbom(error.to_string()))?;
    Ok(PackageBundle {
        plan,
        sbom,
        locks: vec![lock],
        easyconfigs: vec![EmittedEasyconfig {
            profile: "default".into(),
            filename: result.filename,
            text: result.text,
        }],
    })
}

fn package_plan_from_easyconfig(
    recipe: &crate::eb_parse::ResolvedEasyconfig,
    toolchain: &Toolchain,
    version: Option<&str>,
    source_checksum: Option<&str>,
) -> PackagePlan {
    let version = version.unwrap_or(&recipe.version).to_string();
    let source_count = if recipe.sources_count > 0 {
        recipe.sources_count
    } else {
        recipe
            .checksums
            .len()
            .saturating_sub(recipe.patch_names.len())
    };
    let mut sources = recipe
        .checksums
        .iter()
        .take(source_count)
        .map(|checksum| SourceArtifact {
            sha256: Some(checksum.clone()),
            ..SourceArtifact::default()
        })
        .collect::<Vec<_>>();
    if let Some(checksum) = source_checksum {
        if let Some(source) = sources.first_mut() {
            source.sha256 = Some(checksum.to_string());
        } else {
            sources.push(SourceArtifact {
                sha256: Some(checksum.to_string()),
                ..SourceArtifact::default()
            });
        }
    }
    let mut dependencies = Vec::new();
    dependencies.extend(
        recipe
            .dependencies
            .iter()
            .enumerate()
            .map(|(index, dependency)| {
                dependency_from_easyconfig(dependency, DependencyRole::Run, index)
            }),
    );
    let runtime_count = dependencies.len();
    dependencies.extend(
        recipe
            .builddependencies
            .iter()
            .enumerate()
            .map(|(index, dependency)| {
                dependency_from_easyconfig(dependency, DependencyRole::Build, runtime_count + index)
            }),
    );
    let versionsuffix = recipe.versionsuffix.iter().cloned().collect::<Vec<_>>();
    let patches = recipe
        .patch_names
        .iter()
        .enumerate()
        .map(|(index, filename)| {
            let resolved_source = Path::new(&recipe.easyconfig_path)
                .parent()
                .map(|directory| directory.join(filename));
            PatchArtifact {
                filename: filename.clone(),
                sha256: recipe.checksums.get(source_count + index).cloned(),
                source: resolved_source
                    .as_deref()
                    .map(|source| source.display().to_string()),
                resolved_source,
            }
        })
        .collect();
    let profile = ProductProfile {
        name: "default".into(),
        default: true,
        versionsuffix,
        features: BTreeMap::new(),
        parameters: BTreeMap::new(),
        toolchain_options: BTreeMap::new(),
        config_options: Vec::new(),
        easyconfig_parameters: BTreeMap::new(),
        verification_commands: Vec::new(),
    };
    PackagePlan {
        schema_version: PACKAGE_SCHEMA_VERSION,
        origin: PackageOrigin::EasyBuild,
        package: PackageMetadata {
            name: recipe.name.clone(),
            version,
            upstream_version: None,
            homepage: recipe.homepage.clone(),
            description: None,
            license: None,
        },
        sources,
        dependencies,
        rules: Vec::new(),
        build: BuildSpec {
            toolchain: toolchain.clone(),
            easyblock: recipe.easyblock.clone(),
            build_systems: Vec::new(),
            config_options: recipe.configopts.iter().cloned().collect(),
            moduleclass: recipe.moduleclass.clone(),
            patches,
            easyconfig_parameters: BTreeMap::new(),
        },
        profiles: vec![profile],
        outputs: vec![OutputRequest {
            profile: "default".into(),
            stack: toolchain.label(),
        }],
        residuals: Vec::new(),
    }
}

fn dependency_from_easyconfig(
    dependency: &ResolvedDep,
    role: DependencyRole,
    index: usize,
) -> DependencyIntent {
    let external = dependency
        .toolchain
        .as_ref()
        .is_some_and(|toolchain| toolchain.name.eq_ignore_ascii_case("system"));
    DependencyIntent {
        id: format!("easybuild:{index}:{}", dependency.name),
        name: dependency.name.clone(),
        eb_name: Some(dependency.name.clone()),
        constraint: Some(format!(">={}", dependency.version)),
        roles: vec![role],
        condition: ConditionExpr::Always,
        virtual_capability: external.then(|| format!("external:system:{}", dependency.name)),
        solver_excluded: false,
        provenance: Vec::new(),
    }
}

pub fn write_package_bundle(
    bundle: &PackageBundle,
    output_directory: &Path,
) -> Result<WrittenPackageBundle, PackageWorkflowError> {
    let inspection_only = bundle.locks.is_empty() && bundle.easyconfigs.is_empty();
    if !inspection_only {
        require_source_checksums(&bundle.plan)?;
    }
    std::fs::create_dir_all(output_directory)
        .map_err(|error| PackageWorkflowError::Io(output_directory.to_path_buf(), error))?;
    let manifest = output_directory.join("package.plan.json");
    let sbom = output_directory.join("package.sbom.cdx.json");
    write_json(&manifest, &bundle.plan)?;
    write_json(&sbom, &bundle.sbom)?;

    let mut locks = Vec::new();
    if !inspection_only {
        let lock_directory = output_directory.join("locks");
        std::fs::create_dir_all(&lock_directory)
            .map_err(|error| PackageWorkflowError::Io(lock_directory.clone(), error))?;
        for lock in &bundle.locks {
            let path = lock_directory.join(format!("{}.lock.json", lock.profile));
            write_json(&path, lock)?;
            locks.push(path);
        }
    }

    let mut easyconfigs = Vec::new();
    let mut patches = Vec::new();
    if !inspection_only {
        let recipe_directory = output_directory
            .join("easyconfigs")
            .join(easyconfig_letter_dir(&bundle.plan.package.name))
            .join(&bundle.plan.package.name);
        std::fs::create_dir_all(&recipe_directory)
            .map_err(|error| PackageWorkflowError::Io(recipe_directory.clone(), error))?;
        for recipe in &bundle.easyconfigs {
            let path = recipe_directory.join(&recipe.filename);
            std::fs::write(&path, &recipe.text)
                .map_err(|error| PackageWorkflowError::Io(path.clone(), error))?;
            easyconfigs.push(path);
        }
        for patch in &bundle.plan.build.patches {
            let source = validate_patch_source(patch)?;
            let path = recipe_directory.join(&patch.filename);
            std::fs::copy(&source, &path)
                .map_err(|error| PackageWorkflowError::Io(path.clone(), error))?;
            patches.push(path);
        }
    }

    Ok(WrittenPackageBundle {
        manifest,
        sbom,
        locks,
        easyconfigs,
        patches,
    })
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), PackageWorkflowError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| PackageWorkflowError::Json(path.to_path_buf(), error))?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| PackageWorkflowError::Io(path.to_path_buf(), error))
}

#[derive(Debug, Error)]
pub enum PackageWorkflowError {
    #[error("foreign package parse: {0}")]
    Foreign(String),
    #[error("EasyBuild package adapter: {0}")]
    EasyBuild(String),
    #[error("package config: {0}")]
    Config(String),
    #[error("package SBOM: {0}")]
    Sbom(String),
    #[error("at least one EasyBuild robot root is required")]
    NoEasyconfigRoots,
    #[error("foreign package plan has no source artifacts")]
    NoSourceArtifacts,
    #[error(
        "source checksum override count mismatch: expected {expected} positional SHA-256 values, got {actual}"
    )]
    SourceChecksumCount { expected: usize, actual: usize },
    #[error("source checksum {index} must be exactly 64 hexadecimal characters, got {checksum:?}")]
    InvalidSourceChecksum { index: usize, checksum: String },
    #[error(
        "source checksum required for artifact positions {0:?}; repeat --source-checksum once per source artifact"
    )]
    MissingSourceChecksums(Vec<usize>),
    #[error("patch checksum required for artifacts {0:?}")]
    MissingPatchChecksums(Vec<String>),
    #[error(
        "patch checksum for {filename} must be exactly 64 hexadecimal characters, got {checksum:?}"
    )]
    InvalidPatchChecksum { filename: String, checksum: String },
    #[error("patch {0} has no source asset")]
    MissingPatchSource(String),
    #[error("read patch source {0}: {1}")]
    PatchIo(PathBuf, std::io::Error),
    #[error("patch checksum mismatch for {filename}: expected {expected}, got {actual}")]
    PatchChecksumMismatch {
        filename: String,
        expected: String,
        actual: String,
    },
    #[error("EasyBuild robot parse: {0}")]
    Robot(String),
    #[error("package profile solve: {0}")]
    Solve(String),
    #[error("EasyBuild recipe emission: {0}")]
    Emit(String),
    #[error("write {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("serialize {0}: {1}")]
    Json(PathBuf, serde_json::Error),
}
