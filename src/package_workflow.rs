//! Canonical foreign-package planning bundle: manifest, SBOM, locks, recipes.

use crate::domain::Toolchain;
use crate::eb_emit::{emit_next_generation_from_path, EmitParams};
use crate::eb_parse::{
    easyconfig_letter_dir, parse_easyconfig_trees, resolve_easyconfig_file, ResolvedDep,
};
use crate::foreign::{parse_foreign_path, ForeignFormat};
use crate::hierarchy::{filter_candidates_in_hierarchy, hierarchy_for_with_tree};
use crate::manifest::package_plan_from_foreign;
use crate::package::{
    package_plan_to_cyclonedx, BuildSpec, ConditionExpr, DependencyIntent, DependencyRole,
    OutputRequest, OverlayExtension, PackageMetadata, PackageOrigin, PackagePlan, PatchArtifact,
    ProductProfile, ProfileLock, Provenance, Residual, ResidualSeverity, ResidualStage,
    SourceArtifact, StackPin, StackPinMode, StackPolicy, PACKAGE_SCHEMA_VERSION,
};
use crate::package_config::{apply_package_layers, PackageConfigLayer};
use crate::package_emit::{emit_profile_easyconfigs, EmittedEasyconfig};
use crate::package_solve::{
    solve_package_profile_with_hierarchy, unsatisfied_direct_dependencies_with_hierarchy,
};
use crate::package_sources::map_source_toolchain_to_target;
use crate::provides::{existing_language_provider, refuses_pip_overlay};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone)]
/// Everything needed to turn a foreign recipe into an installable bundle.
pub struct NewPackageRequest {
    /// Foreign recipe to ingest: a conda-forge meta.yaml or a spack package.py.
    pub source: PathBuf,
    /// Force a format instead of detecting it from the file.
    pub format: Option<ForeignFormat>,
    /// Toolchain generation the emitted recipes target.
    pub toolchain: Toolchain,
    /// Positional SHA-256 overrides, one for every canonical source artifact.
    pub source_checksums: Vec<String>,
    /// Configuration layers applied over the extracted plan, in order.
    pub package_layers: Vec<PackageConfigLayer>,
    /// Robot trees searched for dependency providers. At least one is
    /// required; an empty list is rejected rather than solved against nothing.
    pub easyconfig_roots: Vec<PathBuf>,
    /// Policy governing the dependency solve.
    pub stack_policy: StackPolicy,
}

#[derive(Debug, Clone)]
/// Everything needed to carry an existing easyconfig to a new generation.
pub struct BumpPackageRequest {
    /// The easyconfig being bumped.
    pub source: PathBuf,
    /// Toolchain generation to move to.
    pub toolchain: Toolchain,
    /// New package version. `None` keeps the source version and changes only
    /// the toolchain.
    pub version: Option<String>,
    /// SHA-256 of the new source tarball. Absent leaves the existing checksum
    /// in place and marks it stale, because a version change invalidates it.
    pub source_checksum: Option<String>,
    /// Robot trees searched for dependency providers.
    pub easyconfig_roots: Vec<PathBuf>,
    /// Toolchain hierarchy to resolve against, when not derived from the tree.
    pub hierarchy_fixture: Option<PathBuf>,
    /// Dependency name to version, pinning what the solve may pick.
    pub overrides: HashMap<String, String>,
    /// Policy governing the dependency solve.
    pub stack_policy: StackPolicy,
    /// Fail the bump when a patch's applicability to the new version cannot
    /// be decided from tree evidence, instead of carrying it with a flag.
    pub strict_patches: bool,
}

#[derive(Debug, Clone)]
/// A planned package, in memory and not yet written anywhere.
///
/// `locks` and `easyconfigs` are both empty for an inspection-only run, which
/// is how a caller tells the two apart.
pub struct PackageBundle {
    /// Canonical plan after ingest and every configuration layer.
    pub plan: PackagePlan,
    /// Planned CycloneDX SBOM. Records intent, not an installed filesystem.
    pub sbom: Value,
    /// One dependency lock per profile.
    pub locks: Vec<ProfileLock>,
    /// Emitted easyconfigs, one per profile.
    pub easyconfigs: Vec<EmittedEasyconfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Where a written bundle landed, so a caller can report or verify the paths.
pub struct WrittenPackageBundle {
    /// The package plan as written.
    pub manifest: PathBuf,
    /// The planned SBOM as written.
    pub sbom: PathBuf,
    /// One lock file per profile.
    pub locks: Vec<PathBuf>,
    /// Emitted easyconfigs, inside the robot-shaped overlay.
    pub easyconfigs: Vec<PathBuf>,
    /// Patch files copied next to the recipes that apply them.
    pub patches: Vec<PathBuf>,
}

/// Ingest a foreign recipe and apply configuration layers, without solving.
///
/// Returns the plan and its planned SBOM. Nothing is written and no robot tree
/// is consulted, so this is the cheap look before committing to a bundle.
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
/// [`inspect_new_package`] for a request, then apply the positional source
/// checksums it carries.
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
/// Solve every profile against `candidates` and emit its easyconfig.
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
    if matches!(
        plan.origin,
        PackageOrigin::Pypi | PackageOrigin::Cran | PackageOrigin::Cargo
    ) {
        if let Some(provider) = already_provided_language_root(&plan, candidates, hierarchy_fixture)
        {
            return Ok(already_provided_bundle(plan, sbom, provider));
        }
        if refuses_pip_overlay(&plan.package.name) {
            return Err(PackageWorkflowError::RefusePipOverlay {
                name: plan.package.name.clone(),
                version: plan.package.version.clone(),
            });
        }
    }
    let mut plan = plan;
    if plan.origin == PackageOrigin::Pypi {
        promote_pypi_overlay_extras(&mut plan, candidates, stack_policy, hierarchy_fixture)?;
        inject_overlay_build_tools(&mut plan, candidates);
        inject_overlay_backend_runtimes(&mut plan, candidates);
    }
    if plan.origin == PackageOrigin::Cargo {
        pin_binutils_to_gcccore(&mut plan, candidates, hierarchy_fixture);
    }
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

fn promote_pypi_overlay_extras(
    plan: &mut PackagePlan,
    candidates: &[crate::domain::Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<(), PackageWorkflowError> {
    let profile = plan
        .outputs
        .first()
        .map(|output| output.profile.as_str())
        .unwrap_or("default");
    let holes = unsatisfied_direct_dependencies_with_hierarchy(
        plan,
        profile,
        &Default::default(),
        candidates,
        stack_policy,
        hierarchy_fixture,
    )
    .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?;
    for hole in holes {
        if hole.name.eq_ignore_ascii_case("python") || hole.name.eq_ignore_ascii_case("r") {
            continue;
        }
        if refuses_pip_overlay(&hole.name) {
            continue;
        }
        let version = overlay_extension_version(plan, &hole.name).ok_or_else(|| {
            PackageWorkflowError::OverlayExtraNeedsVersion {
                name: hole.name.clone(),
                requirement: hole.version_req.clone(),
            }
        })?;
        for dependency in &mut plan.dependencies {
            let identity = dependency
                .eb_name
                .as_deref()
                .unwrap_or(dependency.name.as_str());
            if crate::provides::overlay_package_identity(identity)
                == crate::provides::overlay_package_identity(&hole.name)
            {
                dependency.solver_excluded = true;
            }
        }
        plan.overlay_extensions.push(OverlayExtension {
            name: hole.name.clone(),
            version: version.clone(),
        });
        plan.residuals.push(Residual {
            id: format!("pypi-overlay-ext:{}", hole.name),
            stage: ResidualStage::Resolve,
            category: "pypi-overlay-ext".into(),
            severity: ResidualSeverity::Judgment,
            summary: format!(
                "{} {} is not in the robot; emit it as a PythonBundle extension",
                hole.name, version
            ),
            evidence: Some(hole.version_req),
            provenance: None,
        });
    }
    Ok(())
}

fn pin_binutils_to_gcccore(
    plan: &mut PackagePlan,
    candidates: &[crate::domain::Candidate],
    hierarchy_fixture: Option<&Path>,
) {
    let Ok(hierarchy) =
        hierarchy_for_with_tree(&plan.build.toolchain, hierarchy_fixture, candidates)
    else {
        return;
    };
    let Some(gcccore) = hierarchy
        .members
        .iter()
        .find(|toolchain| toolchain.name.eq_ignore_ascii_case("GCCcore"))
        .cloned()
    else {
        return;
    };
    for dependency in &mut plan.dependencies {
        if dependency.name.eq_ignore_ascii_case("binutils") {
            dependency.toolchain = Some(gcccore.clone());
        }
    }
}

fn inject_overlay_build_tools(plan: &mut PackagePlan, candidates: &[crate::domain::Candidate]) {
    if plan.overlay_extensions.is_empty() {
        return;
    }
    for name in [
        "CMake",
        "Meson",
        "Ninja",
        "pkgconf",
        "Rust",
        "hatchling",
        "Python-bundle-PyPI",
    ] {
        let already = plan.dependencies.iter().any(|dependency| {
            crate::provides::overlay_package_identity(
                dependency
                    .eb_name
                    .as_deref()
                    .unwrap_or(dependency.name.as_str()),
            ) == crate::provides::overlay_package_identity(name)
        });
        let available = crate::provides::existing_language_provider(name, candidates).is_some()
            || candidates.iter().any(|candidate| {
                crate::provides::overlay_package_identity(&candidate.name)
                    == crate::provides::overlay_package_identity(name)
            });
        if already || !available {
            continue;
        }
        plan.dependencies.push(DependencyIntent {
            id: format!("dep:overlay-build:{name}"),
            name: name.to_string(),
            eb_name: None,
            constraint: None,
            toolchain: None,
            roles: vec![DependencyRole::Build],
            condition: ConditionExpr::Always,
            virtual_capability: None,
            solver_excluded: false,
            provenance: Vec::new(),
        });
    }
}

/// Install meson-python / hatchling import deps into the overlay prefix.
///
/// `pip --no-build-isolation` imports those backends from PYTHONPATH. EESSI
/// `Python-bundle-PyPI` may ship them as provides, but the hook does not see
/// the bundle unless the wheel is also in the overlay prefix.
fn inject_overlay_backend_runtimes(
    plan: &mut PackagePlan,
    candidates: &[crate::domain::Candidate],
) {
    if plan.overlay_extensions.is_empty() {
        return;
    }
    for name in ["packaging", "pyproject-metadata"] {
        if plan.overlay_extensions.iter().any(|ext| {
            crate::provides::overlay_package_identity(&ext.name)
                == crate::provides::overlay_package_identity(name)
        }) {
            continue;
        }
        let Some(version) = provided_ext_version(name, candidates) else {
            continue;
        };
        plan.overlay_extensions.insert(
            0,
            OverlayExtension {
                name: name.to_string(),
                version,
            },
        );
    }
}

fn provided_ext_version(name: &str, candidates: &[crate::domain::Candidate]) -> Option<String> {
    let identity = crate::provides::overlay_package_identity(name);
    let provider = crate::provides::existing_language_provider(name, candidates)?;
    provider
        .exts_list
        .iter()
        .find(|ext| crate::provides::overlay_package_identity(&ext.name) == identity)
        .filter(|ext| !ext.version.is_empty())
        .map(|ext| ext.version.clone())
}

fn overlay_extension_version(plan: &PackagePlan, name: &str) -> Option<String> {
    let mut exact = None;
    let mut fallback = None;
    for dependency in &plan.dependencies {
        let identity = dependency
            .eb_name
            .as_deref()
            .unwrap_or(dependency.name.as_str());
        if crate::provides::overlay_package_identity(identity)
            != crate::provides::overlay_package_identity(name)
        {
            continue;
        }
        let Some(version) = version_from_constraint(dependency.constraint.as_deref()) else {
            continue;
        };
        if dependency
            .constraint
            .as_deref()
            .is_some_and(|constraint| constraint.trim().starts_with("=="))
        {
            exact = Some(version);
        } else {
            fallback = Some(version);
        }
    }
    exact.or(fallback)
}

fn version_from_constraint(constraint: Option<&str>) -> Option<String> {
    let constraint = constraint
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if let Some(exact) = constraint.strip_prefix("==") {
        let exact = exact.trim();
        return (!exact.is_empty()).then(|| exact.to_string());
    }
    for prefix in [">=", "~=", ">"] {
        if let Some(rest) = constraint.strip_prefix(prefix) {
            let token = rest.split([',', ' ']).map(str::trim).find(|part| {
                !part.is_empty() && part.starts_with(|ch: char| ch.is_ascii_digit())
            })?;
            return Some(token.to_string());
        }
    }
    None
}

fn already_provided_language_root<'a>(
    plan: &PackagePlan,
    candidates: &'a [crate::domain::Candidate],
    hierarchy_fixture: Option<&Path>,
) -> Option<&'a crate::domain::Candidate> {
    let admitted =
        match hierarchy_for_with_tree(&plan.build.toolchain, hierarchy_fixture, candidates) {
            Ok(hierarchy) => filter_candidates_in_hierarchy(candidates, &hierarchy),
            Err(_) => return existing_language_provider(&plan.package.name, candidates),
        };
    let provider_path = existing_language_provider(&plan.package.name, &admitted)
        .map(|provider| provider.easyconfig_path.clone())?;
    candidates
        .iter()
        .find(|candidate| candidate.easyconfig_path == provider_path)
}

fn already_provided_bundle(
    mut plan: PackagePlan,
    sbom: Value,
    provider: &crate::domain::Candidate,
) -> PackageBundle {
    let provided_ext = provider.exts_list.iter().find(|ext| {
        crate::provides::overlay_package_identity(&ext.name)
            == crate::provides::overlay_package_identity(&plan.package.name)
    });
    let provided_version = provided_ext
        .map(|ext| ext.version.as_str())
        .unwrap_or(provider.version.as_str());
    plan.residuals.push(Residual {
        id: "already-provided".into(),
        stage: ResidualStage::Resolve,
        category: "already-provided".into(),
        severity: ResidualSeverity::Judgment,
        summary: format!(
            "{} {} is already provided by {} {} ({} {}); do not emit a pip overlay",
            plan.package.name,
            plan.package.version,
            provider.name,
            provider.version,
            plan.package.name,
            provided_version
        ),
        evidence: Some(provider.easyconfig_path.clone()),
        provenance: None,
    });
    PackageBundle {
        plan,
        sbom,
        locks: Vec::new(),
        easyconfigs: Vec::new(),
    }
}

/// The whole new-package path: ingest, layer, solve, emit.
///
/// Fails with [`PackageWorkflowError::NoEasyconfigRoots`] rather than solving
/// against an empty universe, which would silently produce a bundle whose
/// dependencies resolve to nothing.
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
/// Read the source easyconfig and build the plan for its new generation.
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
/// The policy with each override applied as an exact pin, replacing any pin
/// the policy already held for that package.
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
/// Solve the bumped plan and emit the new easyconfig.
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
    let mut result = emit_next_generation_from_path(
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

    // A version bump decides its patch set from tree evidence: a recipe for
    // the new version under another toolchain is what a maintainer already
    // ships, so its patch block is adopted verbatim and each patch carries a
    // recorded decision. Without a sibling, version-pinned patch names are
    // flagged rather than silently carried.
    let mut patch_calls: Vec<crate::patch_evolution::PatchCall> = Vec::new();
    if let Some(new_version) = request.version.as_deref() {
        let source_recipe = resolve_easyconfig_file(&request.source)
            .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
        if new_version != source_recipe.version {
            let sibling = crate::patch_evolution::sibling_paths(
                &source_recipe.name,
                new_version,
                candidates,
                &request.toolchain,
            )
            .into_iter()
            .find_map(|path| {
                resolve_easyconfig_file(Path::new(&path))
                    .ok()
                    .map(|recipe| crate::patch_evolution::SiblingRecipe {
                        easyconfig_path: path,
                        toolchain: recipe.toolchain,
                        patch_names: recipe.patch_names,
                    })
            });
            let patch_plan = crate::patch_evolution::plan_patch_evolution(
                new_version,
                &source_recipe.patch_names,
                sibling.as_ref(),
            );
            if request.strict_patches && !patch_plan.undecided().is_empty() {
                return Err(PackageWorkflowError::UndecidedPatches(
                    patch_plan.undecided().join(", "),
                ));
            }
            if let Some(sibling_path) = &patch_plan.sibling {
                result.text =
                    crate::patch_evolution::adopt_sibling_patch_block(&result.text, sibling_path)
                        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
                // Per-patch evidence supersedes the blanket review warning.
                result
                    .warnings
                    .retain(|w| !w.contains("patches were not modified"));
            }
            patch_calls = patch_plan.calls;
        }
    }

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
    for (index, call) in patch_calls.iter().enumerate() {
        plan.residuals.push(Residual {
            id: format!("patch-decision:{index}"),
            stage: ResidualStage::Emit,
            category: "patch-decision".into(),
            severity: match call.decision {
                crate::patch_evolution::PatchDecision::Undecided => ResidualSeverity::Judgment,
                _ => ResidualSeverity::Mechanical,
            },
            summary: format!(
                "{} {}: {}",
                call.decision.as_str(),
                call.patch,
                call.evidence
            ),
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

/// The whole bump path: read, re-target, solve, emit.
///
/// Fails with [`PackageWorkflowError::NoEasyconfigRoots`] rather than solving
/// against an empty universe.
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
    let retarget = is_generation_retarget(&recipe.toolchain, toolchain);
    let mut dependencies = Vec::new();
    dependencies.extend(
        recipe
            .dependencies
            .iter()
            .enumerate()
            .map(|(index, dependency)| {
                dependency_from_easyconfig(
                    dependency,
                    DependencyRole::Run,
                    index,
                    toolchain,
                    retarget,
                )
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
                    retarget,
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
        overlay_extensions: Vec::new(),
    }
}

/// Whether the plan moves the recipe onto a different toolchain generation.
///
/// This decides what a dependency version in the source recipe means. Within
/// one generation it is the version the recipe was built and tested against,
/// so a version bump should not regress below it. Across generations it is
/// only what that generation happened to ship: EasyBuild pins dependencies
/// exactly and has no notion of a minimum, so carrying the pin over as a floor
/// invents a requirement the package never stated, and a move onto an older
/// generation then fails on a floor no released version there can satisfy.
///
/// Unconstrained is the fallback, not the ideal. A real lower bound belongs in
/// the foreign manifest, where a Spack `depends_on` range or a conda-forge
/// version constraint states what the package actually needs; when the plan
/// carries one, that constraint is the one to keep.
fn is_generation_retarget(source: &Toolchain, target: &Toolchain) -> bool {
    !source.name.eq_ignore_ascii_case(&target.name) || source.version != target.version
}

fn dependency_from_easyconfig(
    dependency: &ResolvedDep,
    role: DependencyRole,
    index: usize,
    target_toolchain: &Toolchain,
    retarget: bool,
) -> DependencyIntent {
    let external = dependency
        .toolchain
        .as_ref()
        .is_some_and(|toolchain| toolchain.name.eq_ignore_ascii_case("system"));
    DependencyIntent {
        id: format!("easybuild:{index}:{}", dependency.name),
        name: dependency.name.clone(),
        eb_name: Some(dependency.name.clone()),
        // Unconstrained across a retarget: hierarchy filtering keeps the
        // candidate set to the target generation and prefer_newer picks within
        // it, which is the version that generation ships.
        constraint: (!retarget).then(|| format!(">={}", dependency.version)),
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

/// Write a bundle to one directory, artifacts and recipe overlay together.
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
    write_json(&manifest, &portable_package_plan(&bundle.plan))?;
    write_json(&sbom, &bundle.sbom)?;

    let mut locks = Vec::new();
    if !inspection_only {
        let lock_directory = artifact_directory.join("locks");
        std::fs::create_dir_all(&lock_directory)
            .map_err(|error| PackageWorkflowError::Io(lock_directory.clone(), error))?;
        for lock in &bundle.locks {
            let path = lock_directory.join(format!("{}.lock.json", lock.profile));
            write_json(&path, &portable_profile_lock(lock))?;
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

fn portable_package_plan(plan: &PackagePlan) -> PackagePlan {
    let mut portable = plan.clone();
    for source in &mut portable.sources {
        normalize_provenance_paths(&portable.origin, &mut source.provenance);
    }
    for dependency in &mut portable.dependencies {
        normalize_provenance_paths(&portable.origin, &mut dependency.provenance);
    }
    for rule in &mut portable.rules {
        normalize_provenance_path(&portable.origin, &mut rule.provenance);
    }
    for residual in &mut portable.residuals {
        if let Some(provenance) = &mut residual.provenance {
            normalize_provenance_path(&portable.origin, provenance);
        }
    }
    for patch in &mut portable.build.patches {
        if patch.resolved_source.is_some()
            || patch
                .source
                .as_deref()
                .is_some_and(|source| Path::new(source).is_absolute())
        {
            patch.source = Some(patch.filename.clone());
        }
    }
    portable
}

fn normalize_provenance_paths(origin: &PackageOrigin, provenance: &mut [Provenance]) {
    for item in provenance {
        normalize_provenance_path(origin, item);
    }
}

fn normalize_provenance_path(origin: &PackageOrigin, provenance: &mut Provenance) {
    let filename = Path::new(&provenance.span.path)
        .file_name()
        .and_then(|filename| filename.to_str())
        .unwrap_or(&provenance.span.path);
    let origin = match origin {
        PackageOrigin::CondaForge => "conda-forge",
        PackageOrigin::Spack => "spack",
        PackageOrigin::EasyBuild => "easybuild",
        PackageOrigin::Pypi => "pypi",
        PackageOrigin::Cran => "cran",
        PackageOrigin::Cargo => "cargo",
    };
    provenance.span.path = format!("{origin}/{filename}");
}

fn portable_profile_lock(lock: &ProfileLock) -> ProfileLock {
    let mut portable = lock.clone();
    for dependency in &mut portable.dependencies {
        dependency.easyconfig_path = portable_easyconfig_path(&dependency.easyconfig_path);
    }
    portable
}

fn portable_easyconfig_path(path: &str) -> String {
    let path = Path::new(path);
    if !path.is_absolute() {
        return relative_posix(path);
    }
    let components = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(index) = components
        .windows(2)
        .position(|parts| parts[0] == "easybuild" && parts[1] == "easyconfigs")
    {
        return components[index..].join("/");
    }
    path.file_name()
        .map(|filename| filename.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Reject path segments that would escape the bundle layout.
/// Reject a path segment that could escape the bundle root.
///
/// Package and profile names reach the filesystem, and a recipe is untrusted
/// input, so an empty segment, a separator, or a parent reference is an error
/// rather than something to sanitise and continue with.
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
/// A path as posix-separated text, keeping only normal components.
///
/// Used for overlay keys so collision detection compares the same string on
/// every platform.
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
/// Why a package could not be planned or written.
pub enum PackageWorkflowError {
    /// The foreign recipe could not be read or understood.
    #[error("foreign package parse: {0}")]
    Foreign(String),
    /// The source easyconfig could not be resolved into a plan.
    #[error("EasyBuild package adapter: {0}")]
    EasyBuild(String),
    /// A configuration layer was invalid or could not be applied.
    #[error("package config: {0}")]
    Config(String),
    /// The planned SBOM could not be produced.
    #[error("package SBOM: {0}")]
    Sbom(String),
    /// No robot tree was given. Solving against nothing would produce a
    /// bundle whose dependencies silently resolve to nothing.
    #[error("at least one EasyBuild robot root is required")]
    NoEasyconfigRoots,
    /// The recipe declares nothing to download, so there is nothing to build.
    #[error("foreign package plan has no source artifacts")]
    NoSourceArtifacts,
    /// Strict patch mode: a patch's applicability to the new version could
    /// not be decided from tree evidence.
    #[error("undecided patches after version bump: {0}")]
    UndecidedPatches(String),
    /// The number of positional checksums does not match the artifact count.
    #[error(
        "source checksum override count mismatch: expected {expected} positional SHA-256 values, got {actual}"
    )]
    SourceChecksumCount {
        /// Source artifacts the plan carries.
        expected: usize,
        /// Checksums supplied.
        actual: usize,
    },
    /// A source checksum is not a SHA-256 hex digest.
    #[error("source checksum {index} must be exactly 64 hexadecimal characters, got {checksum:?}")]
    InvalidSourceChecksum {
        /// Position of the offending value.
        index: usize,
        /// The value as given.
        checksum: String,
    },
    /// Some source artifacts have no checksum, so the recipe would download
    /// unverified bytes.
    #[error(
        "source checksum required for artifact positions {0:?}; repeat --source-checksum once per source artifact"
    )]
    MissingSourceChecksums(
        /// Artifact positions still without a checksum.
        Vec<usize>,
    ),
    /// Some patches have no checksum.
    #[error("patch checksum required for artifacts {0:?}")]
    MissingPatchChecksums(
        /// Patch filenames still without a checksum.
        Vec<String>,
    ),
    /// A patch checksum is not a SHA-256 hex digest.
    #[error(
        "patch checksum for {filename} must be exactly 64 hexadecimal characters, got {checksum:?}"
    )]
    InvalidPatchChecksum {
        /// Patch the value belongs to.
        filename: String,
        /// The value as given.
        checksum: String,
    },
    /// A patch is declared but names no file to copy.
    #[error("patch {0} has no source asset")]
    MissingPatchSource(
        /// Patch that names no file to read.
        String,
    ),
    /// A patch file could not be read.
    #[error("read patch source {0}: {1}")]
    PatchIo(
        /// Patch file that could not be read.
        PathBuf,
        /// Underlying error.
        std::io::Error,
    ),
    /// A patch on disk does not hash to what the plan declared.
    #[error("patch checksum mismatch for {filename}: expected {expected}, got {actual}")]
    PatchChecksumMismatch {
        /// Patch whose bytes disagree with the declared checksum.
        filename: String,
        /// Checksum the plan declared.
        expected: String,
        /// Checksum of the bytes on disk.
        actual: String,
    },
    /// A robot tree could not be parsed.
    #[error("EasyBuild robot parse: {0}")]
    Robot(String),
    /// No dependency selection satisfies a profile.
    #[error("package profile solve: {0}")]
    Solve(String),
    /// The foreign root is a compiled scientific package the robot does
    /// not ship. A `PythonBundle` pip overlay is the wrong install.
    #[error(
        "{name} {version} is a compiled scientific package; the robot does not provide it via SciPy-bundle or PyTorch. Do not pip-overlay it with PythonBundle"
    )]
    RefusePipOverlay {
        /// Package that must come from the scientific stack.
        name: String,
        /// Version the foreign source asked for.
        version: String,
    },
    /// A leftover PyPI dependency is not in the robot and has no exact
    /// version, so it cannot become an `exts_list` entry.
    #[error(
        "{name} is not in the robot and {requirement:?} is not an exact or lower-bounded version; pin it to emit a PythonBundle extension"
    )]
    OverlayExtraNeedsVersion {
        /// Missing leftover package.
        name: String,
        /// Constraint that could not be lowered to a version.
        requirement: String,
    },
    /// The easyconfig could not be rendered.
    #[error("EasyBuild recipe emission: {0}")]
    Emit(String),
    /// A bundle file could not be written.
    #[error("write {0}: {1}")]
    Io(
        /// Path being written.
        PathBuf,
        /// Underlying error.
        std::io::Error,
    ),
    /// A bundle file could not be serialized.
    #[error("serialize {0}: {1}")]
    Json(
        /// Path being serialized.
        PathBuf,
        /// Underlying error.
        serde_json::Error,
    ),
    /// A name that reaches the filesystem could escape the bundle root.
    #[error("unsafe {kind} path segment {value:?}: {reason}")]
    UnsafePathSegment {
        /// What the segment names, e.g. package or profile.
        kind: String,
        /// The segment as given.
        value: String,
        /// Why it was refused.
        reason: String,
    },
    /// Two packages claim the same overlay destination.
    #[error("easyconfig overlay collision at {path}: {reason}")]
    OverlayCollision {
        /// Overlay destination two packages both claim.
        path: String,
        /// What already claimed it.
        reason: String,
    },
    /// A resolved overlay path lies outside the bundle root.
    #[error("overlay path {path} is outside recipe bundle root {root}")]
    OverlayPathOutsideBundle {
        /// Destination that escaped the root.
        path: PathBuf,
        /// Root it had to stay within.
        root: PathBuf,
    },
}
