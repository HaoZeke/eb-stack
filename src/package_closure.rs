//! Recursive package-closure planner and aggregate bundle writer.
//!
//! Closes a root foreign package plan over robot candidates, optional catalog
//! overrides, and ordered local source roots (EasyBuild / conda-forge / Spack).
//! Compatible robot candidates win. Holes resolve robot-first, then catalog
//! overrides, then EasyBuild cross-generation bump, then an unambiguous foreign
//! recipe. Package names never appear as control flow.
//!
//! The aggregate writer places root artifacts at the bundle root, companion
//! artifacts under `packages/<name>-<version>-<toolchain>/`, and every recipe
//! and verified patch into one shared `easyconfigs/` overlay.

use crate::domain::{Candidate, Toolchain};
use crate::eb_parse::{resolve_easyconfig_file, resolve_easyconfig_str};
use crate::hierarchy::{hierarchy_for_with_tree, ToolchainHierarchy};
use crate::package::{OutputRequest, PackagePlan, ProfileEnvironment, StackPolicy};
use crate::package_catalog::{
    CatalogProviderKind, PackageCatalogError, PackageSourceCatalog, PackageSourceProvider,
};
use crate::package_config::PackageConfigLayer;
use crate::package_solve::{
    solve_package_profile_with_hierarchy, unsatisfied_direct_dependencies, ProfileSolveError,
    UnsatisfiedDirectDependency,
};
use crate::package_sources::{
    discover_provider_candidates_for_hole, map_source_toolchain_to_target, DiscoveredCandidate,
    PackageSourceError, PackageSourceIndex, PackageSourceRoots, ProviderDiscoveryError,
};
use crate::package_workflow::{
    complete_package_bump, complete_package_bundle, prepare_new_package_plan, prepare_package_bump,
    relative_posix, stack_policy_with_bump_overrides, validate_path_segment, write_json,
    write_package_bundle_into, BumpPackageRequest, NewPackageRequest, PackageBundle,
    PackageWorkflowError, WrittenPackageBundle,
};
use crate::version::matches_req;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Schema version for `build-order.json` and `closure.plan.json`.
pub const CLOSURE_BUNDLE_SCHEMA_VERSION: u32 = 1;

/// Root package bundle plus generated companions in topological build order.
#[derive(Debug, Clone)]
pub struct PackageClosure {
    pub root: PackageBundle,
    /// Generated companion bundles, dependencies before dependents.
    pub companions: Vec<PackageBundle>,
}

/// Paths written for a closed multi-package bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPackageClosure {
    pub root: WrittenPackageBundle,
    pub companions: Vec<WrittenPackageBundle>,
    pub build_order: PathBuf,
    pub closure_plan: PathBuf,
    pub closure_sbom: PathBuf,
}

/// Declared EasyBuild build order for a closed package bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureBuildOrder {
    pub schema_version: u32,
    /// Bundle-relative recipe paths using `/` separators.
    pub recipes: Vec<String>,
}

/// Aggregate description of a written package closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosurePlanDocument {
    pub schema_version: u32,
    pub root: ClosurePackageRef,
    pub companions: Vec<ClosurePackageRef>,
    pub build_order: String,
}

/// One package identity inside a written closure layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosurePackageRef {
    pub name: String,
    pub version: String,
    pub toolchain: crate::domain::Toolchain,
    /// Bundle-relative directory (`.` for the root package).
    pub directory: String,
    pub manifest: String,
    pub sbom: String,
    pub recipes: Vec<String>,
}

#[derive(Debug, Error)]
pub enum PackageClosureError {
    #[error(transparent)]
    Workflow(#[from] PackageWorkflowError),
    #[error(transparent)]
    Catalog(#[from] PackageCatalogError),
    #[error(transparent)]
    SourceRoots(#[from] PackageSourceError),
    #[error("package profile solve: {0}")]
    Solve(#[from] ProfileSolveError),
    #[error("dependency cycle: {}", path_display(.path))]
    Cycle { path: Vec<String> },
    #[error("no package-source catalog entry for {name}{version}")]
    MissingProvider { name: String, version: String },
    #[error(
        "ambiguous package-source providers for {name}{version}: {count} catalog entries match"
    )]
    AmbiguousProvider {
        name: String,
        version: String,
        count: usize,
    },
    #[error("no package source for {name}{version_req}")]
    MissingSource {
        name: String,
        version_req: String,
        candidates: Vec<DiscoveredCandidate>,
    },
    #[error("ambiguous package sources for {name}{version_req}: {count} compatible candidates")]
    AmbiguousSource {
        name: String,
        version_req: String,
        count: usize,
        candidates: Vec<DiscoveredCandidate>,
    },
    #[error(
        "catalog provider {name} version {provided} does not satisfy dependency requirement {required}"
    )]
    IncompatibleProviderVersion {
        name: String,
        provided: String,
        required: String,
    },
    #[error(
        "generated candidate for {name} ({required}) is not admitted for profile {profile} by the target hierarchy or stack policy"
    )]
    GeneratedCandidateNotAdmitted {
        name: String,
        required: String,
        profile: String,
    },
    #[error("package {package} has no product profile {profile}")]
    MissingProfile { package: String, profile: String },
    #[error("package {package} profile selection is ambiguous: {reason}")]
    ProfileAmbiguity { package: String, reason: String },
    #[error("generated companion candidate: {0}")]
    GeneratedCandidate(String),
    #[error("package-closure bundle layout: {0}")]
    Layout(String),
    #[error("aggregate package-closure SBOM: {0}")]
    AggregateSbom(String),
}

fn path_display(path: &[String]) -> String {
    path.join(" -> ")
}

/// Plan a root package and recursively close robot holes (catalog only).
///
/// Prefer [`plan_package_closure_with_sources`] when ordered EasyBuild /
/// conda-forge / Spack source roots are configured.
pub fn plan_package_closure(
    request: &NewPackageRequest,
    catalog: &PackageSourceCatalog,
) -> Result<PackageClosure, PackageClosureError> {
    plan_package_closure_with_sources(request, catalog, &PackageSourceRoots::default())
}

/// Plan a root package and close robot holes via catalog overrides and source roots.
///
/// The EasyBuild robot is parsed once. Each requested root profile is solved
/// against robot candidates plus generated companions. Holes are discovered by
/// inspecting the admitted candidate universe, never by parsing solver text.
/// Explicit catalog entries override discovery for the same identity.
pub fn plan_package_closure_with_sources(
    request: &NewPackageRequest,
    catalog: &PackageSourceCatalog,
    source_roots: &PackageSourceRoots,
) -> Result<PackageClosure, PackageClosureError> {
    if request.easyconfig_roots.is_empty() {
        return Err(PackageWorkflowError::NoEasyconfigRoots.into());
    }
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = crate::eb_parse::parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;

    let source_index = PackageSourceIndex::build_with_easybuild_candidates(
        source_roots,
        &tree.candidates,
        &request.easyconfig_roots,
    )?;
    let target_hierarchy = hierarchy_for_with_tree(&request.toolchain, None, &tree.candidates).ok();

    let (plan, sbom) = prepare_new_package_plan(request)?;
    let mut state = ClosureState {
        robot: tree.candidates,
        generated: HashMap::new(),
        topo: Vec::new(),
        catalog,
        source_index,
        target_toolchain: request.toolchain.clone(),
        target_hierarchy,
        easyconfig_roots: request.easyconfig_roots.clone(),
        default_stack_policy: request.stack_policy.clone(),
    };

    let root_path = vec![plan.package.name.clone()];
    let root = state.close_package(
        PreparedCompanion::Foreign { plan, sbom },
        &request.stack_policy,
        &root_path,
    )?;

    let companions = state
        .topo
        .iter()
        .filter_map(|key| state.generated.get(key).map(|entry| entry.bundle.clone()))
        .collect();

    Ok(PackageClosure { root, companions })
}

/// Write a planned package closure into a single campaign-ready bundle layout.
///
/// Root manifest/SBOM/locks stay at the bundle root. Each companion is written
/// under `packages/<name>-<version>-<toolchain-name>-<toolchain-version>/`.
/// Every recipe and verified patch lands in one shared `easyconfigs/` overlay;
/// colliding destinations are rejected. Profile order within each package is
/// preserved; companions precede the root in `build-order.json`.
pub fn write_package_closure(
    closure: &PackageClosure,
    output_directory: &Path,
) -> Result<WrittenPackageClosure, PackageClosureError> {
    std::fs::create_dir_all(output_directory)
        .map_err(|error| PackageWorkflowError::Io(output_directory.to_path_buf(), error))?;

    let mut claimed = BTreeMap::new();
    let root = write_package_bundle_into(
        &closure.root,
        output_directory,
        output_directory,
        &mut claimed,
    )?;

    let mut written_companions = Vec::with_capacity(closure.companions.len());
    let mut companion_refs = Vec::with_capacity(closure.companions.len());
    for companion in &closure.companions {
        let segment = package_layout_segment(companion)?;
        let artifact_directory = output_directory.join("packages").join(&segment);
        let written = write_package_bundle_into(
            companion,
            &artifact_directory,
            output_directory,
            &mut claimed,
        )?;
        companion_refs.push(package_ref_from_written(
            companion,
            &written,
            output_directory,
            &format!("packages/{segment}"),
        )?);
        written_companions.push(written);
    }

    let root_ref = package_ref_from_written(&closure.root, &root, output_directory, ".")?;

    let mut recipes = Vec::new();
    for companion in &written_companions {
        for path in &companion.easyconfigs {
            recipes.push(bundle_relative(output_directory, path)?);
        }
    }
    for path in &root.easyconfigs {
        recipes.push(bundle_relative(output_directory, path)?);
    }

    let build_order_doc = ClosureBuildOrder {
        schema_version: CLOSURE_BUNDLE_SCHEMA_VERSION,
        recipes: recipes.clone(),
    };
    let build_order = output_directory.join("build-order.json");
    write_json(&build_order, &build_order_doc)?;

    let closure_plan_doc = ClosurePlanDocument {
        schema_version: CLOSURE_BUNDLE_SCHEMA_VERSION,
        root: root_ref,
        companions: companion_refs,
        build_order: "build-order.json".into(),
    };
    let closure_plan = output_directory.join("closure.plan.json");
    write_json(&closure_plan, &closure_plan_doc)?;

    let aggregate = merge_closure_sboms(
        std::iter::once(&closure.root.sbom)
            .chain(closure.companions.iter().map(|bundle| &bundle.sbom)),
    )?;
    let closure_sbom = output_directory.join("closure.sbom.cdx.json");
    write_json(&closure_sbom, &aggregate)?;

    Ok(WrittenPackageClosure {
        root,
        companions: written_companions,
        build_order,
        closure_plan,
        closure_sbom,
    })
}

/// Deterministic companion directory segment under `packages/`.
///
/// Format: `<name>-<version>-<toolchain-name>-<toolchain-version>`. Each field
/// is validated as a single safe path component so no package-specific branches
/// are required to keep the layout inside the bundle.
pub fn package_layout_segment(bundle: &PackageBundle) -> Result<String, PackageClosureError> {
    let name = &bundle.plan.package.name;
    let version = &bundle.plan.package.version;
    let toolchain = &bundle.plan.build.toolchain;
    validate_path_segment(name, "package name")?;
    validate_path_segment(version, "package version")?;
    validate_path_segment(&toolchain.name, "toolchain name")?;
    validate_path_segment(&toolchain.version, "toolchain version")?;
    Ok(format!(
        "{}-{}-{}-{}",
        name, version, toolchain.name, toolchain.version
    ))
}

fn package_ref_from_written(
    bundle: &PackageBundle,
    written: &WrittenPackageBundle,
    output_directory: &Path,
    directory: &str,
) -> Result<ClosurePackageRef, PackageClosureError> {
    let mut recipes = Vec::with_capacity(written.easyconfigs.len());
    for path in &written.easyconfigs {
        recipes.push(bundle_relative(output_directory, path)?);
    }
    Ok(ClosurePackageRef {
        name: bundle.plan.package.name.clone(),
        version: bundle.plan.package.version.clone(),
        toolchain: bundle.plan.build.toolchain.clone(),
        directory: directory.into(),
        manifest: bundle_relative(output_directory, &written.manifest)?,
        sbom: bundle_relative(output_directory, &written.sbom)?,
        recipes,
    })
}

fn bundle_relative(root: &Path, path: &Path) -> Result<String, PackageClosureError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        PackageClosureError::Layout(format!("path {} is outside bundle", path.display()))
    })?;
    Ok(relative_posix(relative))
}

/// Merge root and companion CycloneDX documents into one aggregate BOM.
///
/// Components and dependency graph edges are deduplicated by `bom-ref` when
/// present, otherwise by a stable `type|name|version` identity. The result is a
/// real CycloneDX JSON document, not a custom wrapper.
pub fn merge_closure_sboms<'a, I>(sboms: I) -> Result<Value, PackageClosureError>
where
    I: IntoIterator<Item = &'a Value>,
{
    let mut components: Vec<Value> = Vec::new();
    let mut seen_components: BTreeMap<String, usize> = BTreeMap::new();
    let mut dependencies: Vec<Value> = Vec::new();
    let mut seen_dep_refs: BTreeMap<String, usize> = BTreeMap::new();
    let mut metadata_component: Option<Value> = None;
    let mut bom_format = "CycloneDX".to_string();
    let mut spec_version = "1.5".to_string();
    let mut version = 1u64;

    for sbom in sboms {
        if let Some(format) = sbom.get("bomFormat").and_then(Value::as_str) {
            bom_format = format.to_string();
        }
        if let Some(spec) = sbom.get("specVersion").and_then(Value::as_str) {
            spec_version = spec.to_string();
        }
        if let Some(ver) = sbom.get("version").and_then(Value::as_u64) {
            version = ver;
        }
        if metadata_component.is_none() {
            if let Some(component) = sbom.pointer("/metadata/component").cloned() {
                metadata_component = Some(component);
            }
        }
        if let Some(list) = sbom.get("components").and_then(Value::as_array) {
            for component in list {
                let key = component_identity(component);
                if seen_components.contains_key(&key) {
                    continue;
                }
                seen_components.insert(key, components.len());
                components.push(component.clone());
            }
        }
        if let Some(list) = sbom.get("dependencies").and_then(Value::as_array) {
            for edge in list {
                let Some(reference) = edge.get("ref").and_then(Value::as_str).map(str::to_string)
                else {
                    continue;
                };
                if let Some(&index) = seen_dep_refs.get(&reference) {
                    // Merge dependsOn lists for the same ref.
                    let existing = &mut dependencies[index];
                    let mut depends = existing
                        .get("dependsOn")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(extra) = edge.get("dependsOn").and_then(Value::as_array) {
                        for item in extra {
                            if !depends.contains(item) {
                                depends.push(item.clone());
                            }
                        }
                    }
                    if let Some(object) = existing.as_object_mut() {
                        object.insert("dependsOn".into(), Value::Array(depends));
                    }
                    continue;
                }
                seen_dep_refs.insert(reference, dependencies.len());
                dependencies.push(edge.clone());
            }
        }
    }

    let mut metadata = Map::new();
    if let Some(component) = metadata_component {
        metadata.insert("component".into(), component);
    }
    metadata.insert(
        "properties".into(),
        json!([{
            "name": "eb-stack:document-kind",
            "value": "package-closure"
        }]),
    );

    let mut aggregate = Map::new();
    aggregate.insert("bomFormat".into(), Value::String(bom_format));
    aggregate.insert("specVersion".into(), Value::String(spec_version));
    aggregate.insert("version".into(), json!(version));
    aggregate.insert("metadata".into(), Value::Object(metadata));
    aggregate.insert("components".into(), Value::Array(components));
    aggregate.insert("dependencies".into(), Value::Array(dependencies));
    Ok(Value::Object(aggregate))
}

fn component_identity(component: &Value) -> String {
    if let Some(bom_ref) = component
        .get("bom-ref")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return format!("ref:{bom_ref}");
    }
    let kind = component
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let name = component.get("name").and_then(Value::as_str).unwrap_or("");
    let version = component
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("");
    format!("id:{kind}|{name}|{version}")
}

struct GeneratedEntry {
    bundle: PackageBundle,
    candidates: Vec<Candidate>,
}

/// Prepared companion ready for hole-filling and kind-specific completion.
enum PreparedCompanion {
    Foreign {
        plan: PackagePlan,
        sbom: Value,
    },
    Bump {
        plan: PackagePlan,
        request: BumpPackageRequest,
        stack_policy: StackPolicy,
    },
}

impl PreparedCompanion {
    fn plan(&self) -> &PackagePlan {
        match self {
            Self::Foreign { plan, .. } | Self::Bump { plan, .. } => plan,
        }
    }
}

struct ClosureState<'a> {
    robot: Vec<Candidate>,
    generated: HashMap<String, GeneratedEntry>,
    topo: Vec<String>,
    catalog: &'a PackageSourceCatalog,
    source_index: PackageSourceIndex,
    target_toolchain: Toolchain,
    target_hierarchy: Option<ToolchainHierarchy>,
    easyconfig_roots: Vec<PathBuf>,
    default_stack_policy: StackPolicy,
}

impl ClosureState<'_> {
    fn universe(&self) -> Vec<Candidate> {
        let mut candidates = self.robot.clone();
        for key in &self.topo {
            if let Some(entry) = self.generated.get(key) {
                candidates.extend(entry.candidates.iter().cloned());
            }
        }
        candidates
    }

    fn close_package(
        &mut self,
        prepared: PreparedCompanion,
        stack_policy: &StackPolicy,
        path: &[String],
    ) -> Result<PackageBundle, PackageClosureError> {
        // Fill holes for every requested profile before the final multi-profile solve.
        let profiles: Vec<String> = prepared
            .plan()
            .outputs
            .iter()
            .map(|output| output.profile.clone())
            .collect();
        for profile in &profiles {
            self.fill_holes_for_profile(prepared.plan(), profile, stack_policy, path)?;
        }

        let candidates = self.universe();
        match prepared {
            PreparedCompanion::Foreign { plan, sbom } => {
                complete_package_bundle(plan, sbom, &candidates, stack_policy).map_err(Into::into)
            }
            PreparedCompanion::Bump {
                plan,
                request,
                stack_policy: bump_policy,
            } => {
                complete_package_bump(&request, plan, &candidates, &bump_policy).map_err(Into::into)
            }
        }
    }

    fn fill_holes_for_profile(
        &mut self,
        plan: &PackagePlan,
        profile: &str,
        stack_policy: &StackPolicy,
        path: &[String],
    ) -> Result<(), PackageClosureError> {
        loop {
            let candidates = self.universe();
            let holes = unsatisfied_direct_dependencies(
                plan,
                profile,
                &ProfileEnvironment::default(),
                &candidates,
                stack_policy,
            )?;
            if holes.is_empty() {
                return Ok(());
            }
            let generated_before = self.generated.len();
            for hole in &holes {
                self.ensure_companion_for_hole(plan, profile, stack_policy, hole, path)?;
            }
            if self.generated.len() == generated_before {
                let hole = &holes[0];
                return Err(PackageClosureError::GeneratedCandidateNotAdmitted {
                    name: hole.name.clone(),
                    required: hole.version_req.clone(),
                    profile: profile.to_string(),
                });
            }
        }
    }

    fn ensure_companion_for_hole(
        &mut self,
        plan: &PackagePlan,
        profile: &str,
        stack_policy: &StackPolicy,
        hole: &UnsatisfiedDirectDependency,
        path: &[String],
    ) -> Result<(), PackageClosureError> {
        if path
            .iter()
            .any(|step| package_identity(step) == package_identity(&hole.name))
        {
            let mut cycle = path.to_vec();
            cycle.push(hole.name.clone());
            return Err(PackageClosureError::Cycle { path: cycle });
        }

        let providers = resolve_provider_candidates_for_hole(
            self.catalog,
            &self.source_index,
            hole,
            &self.target_toolchain,
            self.target_hierarchy.as_ref(),
        )?;
        let mut provider = match providers.as_slice() {
            [provider] => provider.clone(),
            many => self.select_source_provider(plan, profile, stack_policy, hole, many)?,
        };
        refine_bump_toolchain(
            &mut provider,
            &self.target_toolchain,
            self.target_hierarchy.as_ref(),
        )?;

        if let Some(provided_version) = provider.version.as_deref() {
            if !matches_req(provided_version, &hole.version_req) {
                return Err(PackageClosureError::IncompatibleProviderVersion {
                    name: provider.name.clone(),
                    provided: provided_version.to_string(),
                    required: hole.version_req.clone(),
                });
            }
        }

        let key = companion_key(&provider);
        if self.generated.contains_key(&key) {
            // Already closed; verify the generated candidate still satisfies the hole.
            let entry = self.generated.get(&key).expect("just checked");
            let ok = entry.candidates.iter().any(|candidate| {
                package_identity(&candidate.name) == package_identity(&hole.name)
                    && matches_req(&candidate.version, &hole.version_req)
            });
            if ok {
                return Ok(());
            }
            return Err(PackageClosureError::IncompatibleProviderVersion {
                name: provider.name.clone(),
                provided: entry.bundle.plan.package.version.clone(),
                required: hole.version_req.clone(),
            });
        }

        let (prepared, companion_policy) = prepare_companion_from_provider(
            &provider,
            &self.easyconfig_roots,
            &self.default_stack_policy,
        )?;

        let mut child_path = path.to_vec();
        child_path.push(prepared.plan().package.name.clone());

        let companion_bundle = self.close_package(prepared, &companion_policy, &child_path)?;

        let candidates = candidates_from_bundle(&companion_bundle)?;
        // Final constraint check on emitted identity.
        let ok = candidates.iter().any(|candidate| {
            package_identity(&candidate.name) == package_identity(&hole.name)
                && matches_req(&candidate.version, &hole.version_req)
        });
        if !ok {
            let version = companion_bundle.plan.package.version.clone();
            return Err(PackageClosureError::IncompatibleProviderVersion {
                name: hole.name.clone(),
                provided: version,
                required: hole.version_req.clone(),
            });
        }

        self.topo.push(key.clone());
        self.generated.insert(
            key,
            GeneratedEntry {
                bundle: companion_bundle,
                candidates,
            },
        );
        Ok(())
    }

    fn select_source_provider(
        &self,
        plan: &PackagePlan,
        profile: &str,
        stack_policy: &StackPolicy,
        hole: &UnsatisfiedDirectDependency,
        providers: &[PackageSourceProvider],
    ) -> Result<PackageSourceProvider, PackageClosureError> {
        let identity = package_identity(&hole.name);
        let mut selection_plan = plan.clone();
        selection_plan.dependencies.retain(|dependency| {
            let name = dependency
                .eb_name
                .as_deref()
                .unwrap_or(dependency.name.as_str());
            package_identity(name) == identity
        });
        if selection_plan.dependencies.is_empty() {
            return Err(PackageClosureError::GeneratedCandidate(format!(
                "cannot isolate dependency {} for source-version selection",
                hole.name
            )));
        }

        let mut candidates = self.universe();
        let mut prospective = self.source_index.retargeted_candidates_for_providers(
            providers,
            &self.target_toolchain,
            self.target_hierarchy.as_ref(),
        );
        // Recursive closure validates the selected recipe's dependencies. The
        // selection solve is scoped to the direct package identity so metadata
        // version choice does not depend on which companion is generated first.
        for candidate in &mut prospective {
            candidate.dependencies.clear();
            candidate.builddependencies.clear();
        }
        for candidate in prospective {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }

        let lock = solve_package_profile_with_hierarchy(
            &selection_plan,
            profile,
            &ProfileEnvironment::default(),
            &candidates,
            stack_policy,
            None,
        )?;
        let selected = lock
            .dependencies
            .iter()
            .find(|dependency| package_identity(&dependency.name) == identity)
            .ok_or_else(|| {
                PackageClosureError::GeneratedCandidate(format!(
                    "Resolvo did not select a source version for {}",
                    hole.name
                ))
            })?;
        providers
            .iter()
            .find(|provider| {
                provider.source == PathBuf::from(&selected.easyconfig_path)
                    && provider.version.as_deref() == Some(selected.version.as_str())
            })
            .cloned()
            .ok_or_else(|| {
                PackageClosureError::GeneratedCandidate(format!(
                    "Resolvo selected {} {} from {}, which is not an admissible source provider",
                    selected.name, selected.version, selected.easyconfig_path
                ))
            })
    }
}

fn prepare_companion_from_provider(
    provider: &PackageSourceProvider,
    easyconfig_roots: &[PathBuf],
    default_stack_policy: &StackPolicy,
) -> Result<(PreparedCompanion, StackPolicy), PackageClosureError> {
    match provider.provider {
        CatalogProviderKind::Foreign => {
            let companion_request =
                foreign_request_from_provider(provider, easyconfig_roots, default_stack_policy)?;
            let (mut companion_plan, companion_sbom) =
                prepare_new_package_plan(&companion_request)?;
            companion_plan.package.name.clone_from(&provider.name);
            select_provider_profile(&mut companion_plan, &provider.profile)?;
            let policy = companion_request.stack_policy.clone();
            Ok((
                PreparedCompanion::Foreign {
                    plan: companion_plan,
                    sbom: companion_sbom,
                },
                policy,
            ))
        }
        CatalogProviderKind::EasyBuildBump => {
            let bump_request =
                bump_request_from_provider(provider, easyconfig_roots, default_stack_policy)?;
            let (plan, _sbom) = prepare_package_bump(&bump_request)?;
            let policy = stack_policy_with_bump_overrides(
                &bump_request.stack_policy,
                &bump_request.overrides,
            );
            Ok((
                PreparedCompanion::Bump {
                    plan,
                    request: bump_request,
                    stack_policy: policy.clone(),
                },
                policy,
            ))
        }
    }
}

/// Catalog overrides win when present; otherwise discover from source roots.
fn resolve_provider_candidates_for_hole(
    catalog: &PackageSourceCatalog,
    source_index: &PackageSourceIndex,
    hole: &UnsatisfiedDirectDependency,
    target_toolchain: &Toolchain,
    hierarchy: Option<&ToolchainHierarchy>,
) -> Result<Vec<PackageSourceProvider>, PackageClosureError> {
    let named = catalog
        .providers()
        .iter()
        .filter(|provider| package_identity(&provider.name) == package_identity(&hole.name))
        .collect::<Vec<_>>();
    if !named.is_empty() {
        let compatible = named
            .iter()
            .copied()
            .filter(|provider| {
                provider
                    .version
                    .as_deref()
                    .is_none_or(|version| matches_req(version, &hole.version_req))
            })
            .collect::<Vec<_>>();
        return match compatible.as_slice() {
            [provider] => Ok(vec![(*provider).clone()]),
            [] => Err(PackageClosureError::IncompatibleProviderVersion {
                name: hole.name.clone(),
                provided: named
                    .iter()
                    .filter_map(|provider| provider.version.as_deref())
                    .collect::<Vec<_>>()
                    .join(", "),
                required: hole.version_req.clone(),
            }),
            many => Err(PackageClosureError::AmbiguousProvider {
                name: hole.name.clone(),
                version: format!(" ({})", hole.version_req),
                count: many.len(),
            }),
        };
    }

    discover_provider_candidates_for_hole(source_index, hole, target_toolchain, hierarchy).map_err(
        |error| match error {
            ProviderDiscoveryError::Missing {
                name,
                version_req,
                candidates,
            } => {
                if candidates.is_empty()
                    && source_index.easybuild_candidates().is_empty()
                    && source_index.foreign_candidates().is_empty()
                {
                    PackageClosureError::MissingProvider {
                        name,
                        version: version_req,
                    }
                } else {
                    PackageClosureError::MissingSource {
                        name,
                        version_req,
                        candidates,
                    }
                }
            }
            ProviderDiscoveryError::Ambiguous {
                name,
                version_req,
                count,
                candidates,
            } => PackageClosureError::AmbiguousSource {
                name,
                version_req,
                count,
                candidates,
            },
            ProviderDiscoveryError::Incompatible {
                name,
                provided,
                required,
                ..
            } => PackageClosureError::IncompatibleProviderVersion {
                name,
                provided,
                required,
            },
        },
    )
}

/// For EasyBuild-bump providers, remap the source recipe's toolchain family
/// onto the target hierarchy (GCCcore → GCCcore member, not composite parent).
fn refine_bump_toolchain(
    provider: &mut PackageSourceProvider,
    target_toolchain: &Toolchain,
    hierarchy: Option<&ToolchainHierarchy>,
) -> Result<(), PackageClosureError> {
    if provider.provider != CatalogProviderKind::EasyBuildBump {
        return Ok(());
    }
    let resolved = resolve_easyconfig_file(&provider.source)
        .map_err(|error| PackageClosureError::GeneratedCandidate(error.to_string()))?;
    provider.toolchain =
        map_source_toolchain_to_target(Some(&resolved.toolchain), target_toolchain, hierarchy);
    if provider.version.is_none() {
        provider.version = Some(resolved.version);
    }
    Ok(())
}

fn provider_stack_policy(
    provider: &PackageSourceProvider,
    default_stack_policy: &StackPolicy,
) -> Result<StackPolicy, PackageClosureError> {
    if let Some(path) = &provider.stack_policy {
        load_stack_policy(path)
    } else {
        let mut policy = default_stack_policy.clone();
        policy.toolchain = provider.toolchain.clone();
        Ok(policy)
    }
}

fn foreign_request_from_provider(
    provider: &PackageSourceProvider,
    easyconfig_roots: &[PathBuf],
    default_stack_policy: &StackPolicy,
) -> Result<NewPackageRequest, PackageClosureError> {
    let mut package_layers = Vec::new();
    for path in &provider.package_config {
        package_layers.push(
            PackageConfigLayer::from_path(path)
                .map_err(|error| PackageWorkflowError::Config(error.to_string()))?,
        );
    }
    Ok(NewPackageRequest {
        source: provider.source.clone(),
        format: provider.format,
        toolchain: provider.toolchain.clone(),
        source_checksums: provider.source_checksums.clone(),
        package_layers,
        easyconfig_roots: easyconfig_roots.to_vec(),
        stack_policy: provider_stack_policy(provider, default_stack_policy)?,
    })
}

fn bump_request_from_provider(
    provider: &PackageSourceProvider,
    easyconfig_roots: &[PathBuf],
    default_stack_policy: &StackPolicy,
) -> Result<BumpPackageRequest, PackageClosureError> {
    if provider.source_checksums.len() > 1 {
        return Err(PackageCatalogError::MultipleBumpChecksums {
            name: provider.name.clone(),
        }
        .into());
    }
    Ok(BumpPackageRequest {
        source: provider.source.clone(),
        toolchain: provider.toolchain.clone(),
        version: provider.version.clone(),
        source_checksum: provider.source_checksums.first().cloned(),
        easyconfig_roots: easyconfig_roots.to_vec(),
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: provider_stack_policy(provider, default_stack_policy)?,
    })
}

fn load_stack_policy(path: &Path) -> Result<StackPolicy, PackageClosureError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| PackageWorkflowError::Io(path.to_path_buf(), error))?;
    if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
        serde_json::from_str(&text)
            .map_err(|error| PackageWorkflowError::Json(path.to_path_buf(), error))
            .map_err(Into::into)
    } else {
        toml::from_str(&text)
            .map_err(|error| PackageWorkflowError::Config(format!("stack policy TOML: {error}")))
            .map_err(Into::into)
    }
}

fn select_provider_profile(
    plan: &mut PackagePlan,
    profile: &str,
) -> Result<(), PackageClosureError> {
    if profile.trim().is_empty() {
        return Err(PackageClosureError::ProfileAmbiguity {
            package: plan.package.name.clone(),
            reason: "provider profile name is empty".into(),
        });
    }
    if !plan.profiles.iter().any(|item| item.name == profile) {
        return Err(PackageClosureError::MissingProfile {
            package: plan.package.name.clone(),
            profile: profile.to_string(),
        });
    }
    let matching: Vec<_> = plan
        .profiles
        .iter()
        .filter(|item| item.name == profile)
        .collect();
    if matching.len() > 1 {
        return Err(PackageClosureError::ProfileAmbiguity {
            package: plan.package.name.clone(),
            reason: format!("multiple profiles named {profile}"),
        });
    }
    plan.outputs = vec![OutputRequest {
        profile: profile.to_string(),
        stack: plan.build.toolchain.label(),
    }];
    Ok(())
}

fn candidates_from_bundle(bundle: &PackageBundle) -> Result<Vec<Candidate>, PackageClosureError> {
    let mut candidates = Vec::new();
    for emitted in &bundle.easyconfigs {
        let mut resolved = resolve_easyconfig_str(&emitted.text)
            .map_err(|error| PackageClosureError::GeneratedCandidate(error.to_string()))?;
        resolved.easyconfig_path = format!("__package_closure__/{}", emitted.filename);
        // Prefer lock identity when emission and plan agree.
        if let Some(lock) = bundle
            .locks
            .iter()
            .find(|lock| lock.profile == emitted.profile)
        {
            resolved.name = lock.package.clone();
            resolved.version = lock.version.clone();
            resolved.toolchain = lock.toolchain.clone();
            if !lock.versionsuffix.is_empty() {
                resolved.versionsuffix = Some(lock.versionsuffix.clone());
            }
        }
        candidates.push(resolved.to_candidate());
    }
    if candidates.is_empty() {
        return Err(PackageClosureError::GeneratedCandidate(
            "emitted companion produced no candidates".into(),
        ));
    }
    Ok(candidates)
}

fn companion_key(provider: &PackageSourceProvider) -> String {
    format!(
        "{}@{}@{}",
        package_identity(&provider.name),
        provider.version.as_deref().unwrap_or(""),
        provider.toolchain.label()
    )
}

fn package_identity(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_identity_normalizes() {
        assert_eq!(
            package_identity("Capn-Proto"),
            package_identity("capnproto")
        );
    }
}
