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
use crate::package_solve::solve_package_profile_with_hierarchy;
use crate::package_sources::map_source_toolchain_to_target;
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
    materialize_foreign_local_patches(&mut plan, source)?;
    if !package_layers.is_empty() {
        apply_package_layers(&mut plan, package_layers)
            .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
    }
    refresh_checksum_residuals(&mut plan);
    let sbom = package_plan_to_cyclonedx(&plan)
        .map_err(|error| PackageWorkflowError::Sbom(error.to_string()))?;
    Ok((plan, sbom))
}

fn materialize_foreign_local_patches(
    plan: &mut PackagePlan,
    recipe_source: &Path,
) -> Result<(), PackageWorkflowError> {
    let Some(recipe_directory) = recipe_source.parent() else {
        return Ok(());
    };
    for patch in &mut plan.build.patches {
        if patch.url.is_some() || patch.resolved_source.is_some() {
            continue;
        }
        let declared_source = PathBuf::from(patch.source.as_deref().unwrap_or(&patch.filename));
        let resolved_source = if declared_source.is_absolute() {
            declared_source.clone()
        } else {
            recipe_directory.join(&declared_source)
        };
        if !resolved_source.is_file() {
            continue;
        }
        let bytes = std::fs::read(&resolved_source)
            .map_err(|error| PackageWorkflowError::PatchIo(resolved_source.clone(), error))?;
        let filename = resolved_source
            .file_name()
            .and_then(|filename| filename.to_str())
            .ok_or_else(|| PackageWorkflowError::MissingPatchSource(patch.filename.clone()))?
            .to_string();
        patch.filename = filename;
        if patch.sha256.is_none() {
            patch.sha256 = Some(sha256_hex(&bytes));
        }
        patch.source = Some(declared_source.display().to_string());
        patch.resolved_source = Some(resolved_source);
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut checksum = String::with_capacity(64);
    for byte in digest {
        write!(&mut checksum, "{byte:02x}")
            .expect("writing a SHA-256 digest to String cannot fail");
    }
    checksum
}

/// Parse foreign source, apply package layers and optional checksum overrides.
///
/// Does not solve profiles or emit recipes. Used by both single-package planning
/// and recursive package-closure expansion against a shared robot universe.
pub fn prepare_new_package_plan(
    request: &NewPackageRequest,
) -> Result<(PackagePlan, Value), PackageWorkflowError> {
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
    Ok((plan, sbom))
}

/// Solve every plan output profile and emit easyconfigs against a candidate universe.
pub fn complete_package_bundle(
    plan: PackagePlan,
    sbom: Value,
    candidates: &[crate::domain::Candidate],
    stack_policy: &StackPolicy,
) -> Result<PackageBundle, PackageWorkflowError> {
    complete_package_bundle_with_hierarchy(plan, sbom, candidates, stack_policy, None)
}

/// Like [`complete_package_bundle`], with an optional hierarchy fixture path.
pub fn complete_package_bundle_with_hierarchy(
    plan: PackagePlan,
    sbom: Value,
    candidates: &[crate::domain::Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<PackageBundle, PackageWorkflowError> {
    let mut locks = Vec::new();
    for output in &plan.outputs {
        locks.push(
            solve_package_profile_with_hierarchy(
                &plan,
                &output.profile,
                &Default::default(),
                candidates,
                stack_policy,
                hierarchy_fixture,
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

pub fn plan_new_package(
    request: &NewPackageRequest,
) -> Result<PackageBundle, PackageWorkflowError> {
    if request.easyconfig_roots.is_empty() {
        return Err(PackageWorkflowError::NoEasyconfigRoots);
    }
    let (plan, sbom) = prepare_new_package_plan(request)?;
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    complete_package_bundle(plan, sbom, &tree.candidates, &request.stack_policy)
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
    if plan.sources.is_empty() && plan.origin != PackageOrigin::EasyBuild {
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
        if patch.url.is_none()
            && (plan.origin != PackageOrigin::EasyBuild
                || patch.resolved_source.is_some()
                || patch.source.is_some())
        {
            validate_patch_source(patch)?;
        }
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
        .filter(|patch| {
            patch.url.is_none() && patch.resolved_source.is_none() && patch.source.is_none()
        })
        .map(|patch| patch.filename.as_str())
        .collect::<Vec<_>>();
    if !missing_patch_sources.is_empty() {
        plan.residuals.push(Residual {
            id: "patch:missing-source".into(),
            stage: ResidualStage::Normalize,
            category: "patch-asset".into(),
            severity: if plan.origin == PackageOrigin::EasyBuild {
                ResidualSeverity::Judgment
            } else {
                ResidualSeverity::Blocking
            },
            summary: if plan.origin == PackageOrigin::EasyBuild {
                "one or more imported patch artifacts are not available beside the easyconfig"
                    .into()
            } else {
                "one or more patch artifacts have no source file".into()
            },
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
    let actual = sha256_hex(&bytes);
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

/// Parse an existing EasyBuild recipe into a retargeted EasyBuild-origin plan.
///
/// Does not solve dependencies or emit the next-generation recipe. Used by both
/// standalone `package bump` and catalog-backed closure planning for
/// `easybuild-bump` providers.
pub fn prepare_package_bump(
    request: &BumpPackageRequest,
) -> Result<(PackagePlan, Value), PackageWorkflowError> {
    let resolved = resolve_easyconfig_file(&request.source)
        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    let mut plan = package_plan_from_easyconfig(
        &resolved,
        &request.toolchain,
        request.version.as_deref(),
        request.source_checksum.as_deref(),
    );
    refresh_checksum_residuals(&mut plan);
    let sbom = package_plan_to_cyclonedx(&plan)
        .map_err(|error| PackageWorkflowError::Sbom(error.to_string()))?;
    Ok((plan, sbom))
}

/// Fold package-specific `--dep` overrides into locked stack pins.
pub fn stack_policy_with_bump_overrides(
    stack_policy: &StackPolicy,
    overrides: &HashMap<String, String>,
) -> StackPolicy {
    let mut policy = stack_policy.clone();
    for (name, version) in overrides {
        policy.pins.retain(|pin| pin.name != *name);
        policy.pins.push(StackPin {
            name: name.clone(),
            version_requirement: format!("=={version}"),
            toolchain: None,
            versionsuffix: None,
            mode: StackPinMode::Locked,
            source: Some("package bump override".into()),
        });
    }
    policy
}

/// Solve the bump plan against a candidate universe and emit the retargeted `.eb`.
///
/// Preserves source recipe build mechanics, source/patch identity, and checksum
/// order via the annual-bump emitter. Stack-policy preferred pins remain a
/// Resolvo input; lock evidence records selection and fallback outcomes.
pub fn complete_package_bump(
    request: &BumpPackageRequest,
    mut plan: PackagePlan,
    candidates: &[crate::domain::Candidate],
    stack_policy: &StackPolicy,
) -> Result<PackageBundle, PackageWorkflowError> {
    let lock = solve_package_profile_with_hierarchy(
        &plan,
        "default",
        &Default::default(),
        candidates,
        stack_policy,
        request.hierarchy_fixture.as_deref(),
    )
    .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?;
    let dependency_versions = lock
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.clone(), dependency.version.clone()))
        .collect::<HashMap<_, _>>();
    let dependency_toolchains = lock
        .dependencies
        .iter()
        .map(|dependency| (dependency.name.clone(), dependency.toolchain.clone()))
        .collect::<HashMap<_, _>>();
    let result = emit_next_generation_from_path(
        &request.source,
        &EmitParams {
            toolchain: request.toolchain.clone(),
            version: request.version.clone(),
            dep_versions: dependency_versions,
            dep_toolchains: dependency_toolchains,
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

pub fn plan_package_bump(
    request: &BumpPackageRequest,
) -> Result<PackageBundle, PackageWorkflowError> {
    if request.easyconfig_roots.is_empty() {
        return Err(PackageWorkflowError::NoEasyconfigRoots);
    }
    let (plan, _sbom) = prepare_package_bump(request)?;
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    let stack_policy = stack_policy_with_bump_overrides(&request.stack_policy, &request.overrides);
    complete_package_bump(request, plan, &tree.candidates, &stack_policy)
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
                dependency_from_easyconfig(dependency, DependencyRole::Run, index, toolchain)
            }),
    );
    let runtime_count = dependencies.len();
    dependencies.extend(
        recipe
            .builddependencies
            .iter()
            .enumerate()
            .map(|(index, dependency)| {
                dependency_from_easyconfig(
                    dependency,
                    DependencyRole::Build,
                    runtime_count + index,
                    toolchain,
                )
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
                .map(|directory| directory.join(filename))
                .filter(|source| source.is_file());
            PatchArtifact {
                filename: filename.clone(),
                sha256: recipe.checksums.get(source_count + index).cloned(),
                url: None,
                source: resolved_source
                    .as_deref()
                    .map(|source| source.display().to_string()),
                condition: ConditionExpr::Always,
                resolved_source,
            }
        })
        .collect();
    let profile = ProductProfile {
        name: "default".into(),
        default: true,
        versionsuffix,
        platform: None,
        architecture: None,
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
            source_root: None,
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
    target_toolchain: &Toolchain,
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
        toolchain: dependency.toolchain.as_ref().map(|source_toolchain| {
            map_source_toolchain_to_target(Some(source_toolchain), target_toolchain, None)
        }),
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
    let mut claimed = BTreeMap::new();
    write_package_bundle_into(bundle, output_directory, output_directory, &mut claimed)
}

/// Write package artifacts under `artifact_directory` and recipes/patches under the
/// shared `recipe_bundle_root/easyconfigs/<letter>/<name>/` overlay.
///
/// `claimed_paths` tracks every overlay destination (posix-relative to
/// `recipe_bundle_root`) so multi-package writers can reject collisions.
pub fn write_package_bundle_into(
    bundle: &PackageBundle,
    artifact_directory: &Path,
    recipe_bundle_root: &Path,
    claimed_paths: &mut BTreeMap<String, String>,
) -> Result<WrittenPackageBundle, PackageWorkflowError> {
    let inspection_only = bundle.locks.is_empty() && bundle.easyconfigs.is_empty();
    if !inspection_only {
        require_source_checksums(&bundle.plan)?;
    }
    std::fs::create_dir_all(artifact_directory)
        .map_err(|error| PackageWorkflowError::Io(artifact_directory.to_path_buf(), error))?;
    let manifest = artifact_directory.join("package.plan.json");
    let sbom = artifact_directory.join("package.sbom.cdx.json");
    write_json(&manifest, &bundle.plan)?;
    write_json(&sbom, &bundle.sbom)?;

    let mut locks = Vec::new();
    if !inspection_only {
        let lock_directory = artifact_directory.join("locks");
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
        let package_name = &bundle.plan.package.name;
        validate_path_segment(package_name, "package name")?;
        let recipe_directory = recipe_bundle_root
            .join("easyconfigs")
            .join(easyconfig_letter_dir(package_name))
            .join(package_name);
        std::fs::create_dir_all(&recipe_directory)
            .map_err(|error| PackageWorkflowError::Io(recipe_directory.clone(), error))?;
        for recipe in &bundle.easyconfigs {
            validate_path_segment(&recipe.filename, "easyconfig filename")?;
            let path = recipe_directory.join(&recipe.filename);
            claim_overlay_path(recipe_bundle_root, &path, &recipe.text, claimed_paths)?;
            std::fs::write(&path, &recipe.text)
                .map_err(|error| PackageWorkflowError::Io(path.clone(), error))?;
            easyconfigs.push(path);
        }
        for patch in &bundle.plan.build.patches {
            if patch.url.is_some() {
                continue;
            }
            if bundle.plan.origin == PackageOrigin::EasyBuild
                && patch.resolved_source.is_none()
                && patch.source.is_none()
            {
                continue;
            }
            validate_path_segment(&patch.filename, "patch filename")?;
            let source = validate_patch_source(patch)?;
            let path = recipe_directory.join(&patch.filename);
            let content = std::fs::read_to_string(&source)
                .map_err(|error| PackageWorkflowError::PatchIo(source.clone(), error))?;
            claim_overlay_path(recipe_bundle_root, &path, &content, claimed_paths)?;
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

/// Reject path segments that would escape the bundle layout.
pub fn validate_path_segment(segment: &str, kind: &str) -> Result<(), PackageWorkflowError> {
    if segment.is_empty() {
        return Err(PackageWorkflowError::UnsafePathSegment {
            kind: kind.into(),
            value: segment.into(),
            reason: "empty".into(),
        });
    }
    if segment == "." || segment == ".." {
        return Err(PackageWorkflowError::UnsafePathSegment {
            kind: kind.into(),
            value: segment.into(),
            reason: "reserved relative segment".into(),
        });
    }
    if segment.contains('/') || segment.contains('\\') || segment.contains('\0') {
        return Err(PackageWorkflowError::UnsafePathSegment {
            kind: kind.into(),
            value: segment.into(),
            reason: "contains path separator or NUL".into(),
        });
    }
    if Path::new(segment).components().count() != 1 {
        return Err(PackageWorkflowError::UnsafePathSegment {
            kind: kind.into(),
            value: segment.into(),
            reason: "must be a single path component".into(),
        });
    }
    Ok(())
}

fn claim_overlay_path(
    recipe_bundle_root: &Path,
    absolute: &Path,
    content: &str,
    claimed_paths: &mut BTreeMap<String, String>,
) -> Result<(), PackageWorkflowError> {
    let relative = absolute.strip_prefix(recipe_bundle_root).map_err(|_| {
        PackageWorkflowError::OverlayPathOutsideBundle {
            path: absolute.to_path_buf(),
            root: recipe_bundle_root.to_path_buf(),
        }
    })?;
    let key = relative_posix(relative);
    if let Some(previous) = claimed_paths.get(&key) {
        if previous != content {
            return Err(PackageWorkflowError::OverlayCollision {
                path: key,
                reason: "destination already claimed with different content".into(),
            });
        }
        return Err(PackageWorkflowError::OverlayCollision {
            path: key,
            reason: "destination already claimed".into(),
        });
    }
    claimed_paths.insert(key, content.to_string());
    Ok(())
}

/// Join path components with `/` regardless of host separator.
pub fn relative_posix(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn write_json(
    path: &Path,
    value: &impl serde::Serialize,
) -> Result<(), PackageWorkflowError> {
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
    #[error("unsafe {kind} path segment {value:?}: {reason}")]
    UnsafePathSegment {
        kind: String,
        value: String,
        reason: String,
    },
    #[error("easyconfig overlay collision at {path}: {reason}")]
    OverlayCollision { path: String, reason: String },
    #[error("overlay path {path} is outside recipe bundle root {root}")]
    OverlayPathOutsideBundle { path: PathBuf, root: PathBuf },
}
