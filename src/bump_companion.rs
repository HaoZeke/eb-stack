//! Companion bumps: the dependency recipes a version bump needs and the tree
//! does not have.
//!
//! A recipe that writes a dependency version in terms of its own, as
//! `('Boost', version)` or `('protobuf', version[2:])`, asks for a new
//! dependency version every time it moves. When the robot has that name in
//! the target hierarchy at another version, the dependency is bumped too, from
//! that recipe, into the same overlay, and the root is solved against it.
//! A dependency with no recipe of that name anywhere in the hierarchy is left
//! to the root bump, which records it as before.

use crate::domain::Candidate;
use crate::eb_parse::{easyconfig_letter_dir, parse_easyconfig_trees, resolve_easyconfig_str};
use crate::hierarchy::hierarchy_for_with_tree;
use crate::package::ProfileEnvironment;
use crate::package_solve::unsatisfied_direct_dependencies_with_hierarchy;
use crate::package_workflow::{
    plan_package_bump_with, prepare_package_bump, stack_policy_with_bump_overrides,
    BumpPackageRequest, PackageBundle, PackageEmitOptions, PackageWorkflowError,
};
use crate::registry::RegistryClient;
use crate::source_fetch::{plan_package_bump_fetching, SourceFetch};
use crate::version::cmp_version;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Bound on how deep one companion may pull in another. A real chain is one
/// or two long; the bound only stops a tree that keeps naming new versions.
const MAX_COMPANION_DEPTH: usize = 4;

/// One dependency bumped because the root needs a version the tree lacks.
#[derive(Debug, Clone)]
pub struct CompanionBump {
    /// The package that asked for it.
    pub required_by: String,
    /// The request the companion was planned from.
    pub request: BumpPackageRequest,
    /// The planned companion.
    pub bundle: PackageBundle,
    /// What fetching its new source found, when sources were fetched.
    pub fetched: Vec<SourceFetch>,
}

/// A bump together with the companions it needed, dependencies first.
#[derive(Debug, Clone)]
pub struct BumpWithCompanions {
    /// The bump that was asked for.
    pub root: PackageBundle,
    /// What fetching the root's new source found.
    pub fetched: Vec<SourceFetch>,
    /// Companion bumps in build order.
    pub companions: Vec<CompanionBump>,
}

/// The record written beside a bundle that carries companions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanionRecord {
    /// Companion package name.
    pub name: String,
    /// Version it was bumped to.
    pub version: String,
    /// Toolchain it was bumped on.
    pub toolchain: String,
    /// The recipe it was bumped from.
    pub source: String,
    /// The package whose recipe asked for this version.
    pub required_by: String,
}

impl CompanionBump {
    /// The record of this companion for `companions.json`.
    pub fn record(&self) -> CompanionRecord {
        CompanionRecord {
            name: self.bundle.plan.package.name.clone(),
            version: self.bundle.plan.package.version.clone(),
            toolchain: self.bundle.plan.build.toolchain.label(),
            source: self.request.source.display().to_string(),
            required_by: self.required_by.clone(),
        }
    }
}

/// Plan a bump and every companion bump it needs.
///
/// `client` fetches each new source and writes its checksum, as
/// `--fetch-checksums` does for the root; `None` plans without fetching.
pub fn plan_package_bump_with_companions(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
    client: Option<&dyn RegistryClient>,
) -> Result<BumpWithCompanions, PackageWorkflowError> {
    plan_package_bump_with_companions_deciding(
        request,
        options,
        client,
        &crate::source_fetch::BumpDecisions::default(),
    )
}

/// [`plan_package_bump_with_companions`] with the operator's decisions for
/// the root bump. A companion takes none of them: a dropped patch names a
/// file of the root recipe, not of the recipes it pulls in.
pub fn plan_package_bump_with_companions_deciding(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
    client: Option<&dyn RegistryClient>,
    decisions: &crate::source_fetch::BumpDecisions,
) -> Result<BumpWithCompanions, PackageWorkflowError> {
    let mut companions = Vec::new();
    let mut path = Vec::new();
    let (root, fetched) = plan_closed(
        request,
        options,
        client,
        Some(decisions),
        &mut companions,
        &mut path,
    )?;
    Ok(BumpWithCompanions {
        root,
        fetched,
        companions,
    })
}

fn plan_closed(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
    client: Option<&dyn RegistryClient>,
    decisions: Option<&crate::source_fetch::BumpDecisions>,
    companions: &mut Vec<CompanionBump>,
    path: &mut Vec<String>,
) -> Result<(PackageBundle, Vec<SourceFetch>), PackageWorkflowError> {
    let mut options = options.clone();
    if request.version.is_some() && path.len() < MAX_COMPANION_DEPTH {
        let root = crate::eb_parse::resolve_easyconfig_file(&request.source)
            .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
        path.push(root.name);
        for (companion_request, required_by) in companion_requests(request, &options)? {
            let name = crate::eb_parse::resolve_easyconfig_file(&companion_request.source)
                .map(|recipe| recipe.name)
                .unwrap_or_default();
            if path.iter().any(|step| step.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let (bundle, fetched) =
                plan_closed(&companion_request, &options, client, None, companions, path)?;
            options
                .extra_candidates
                .extend(overlay_candidates(&bundle)?);
            companions.push(CompanionBump {
                required_by,
                request: companion_request,
                bundle,
                fetched,
            });
        }
        path.pop();
    }
    match (client, decisions) {
        (Some(client), Some(decisions)) => crate::source_fetch::plan_package_bump_fetching_with(
            request, &options, client, decisions,
        ),
        (Some(client), None) => plan_package_bump_fetching(request, &options, client),
        (None, _) => Ok((plan_package_bump_with(request, &options)?, Vec::new())),
    }
}

/// The bump request for each dependency the bumped recipe needs at an exact
/// version no candidate has, when the tree has that dependency in the target
/// hierarchy at another version to bump from.
pub fn companion_requests(
    request: &BumpPackageRequest,
    options: &PackageEmitOptions,
) -> Result<Vec<(BumpPackageRequest, String)>, PackageWorkflowError> {
    if request.version.is_none() {
        return Ok(Vec::new());
    }
    let (plan, _sbom) = prepare_package_bump(request)?;
    let roots = request
        .easyconfig_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    let mut candidates = tree.candidates;
    candidates.extend(options.extra_candidates.iter().cloned());
    let policy = stack_policy_with_bump_overrides(&request.stack_policy, &request.overrides);
    let holes = unsatisfied_direct_dependencies_with_hierarchy(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &candidates,
        &policy,
        request.hierarchy_fixture.as_deref(),
    )
    .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?;
    if holes.is_empty() {
        return Ok(Vec::new());
    }
    let hierarchy = hierarchy_for_with_tree(
        &request.toolchain,
        request.hierarchy_fixture.as_deref(),
        &candidates,
    )
    .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?;
    let source = crate::eb_parse::resolve_easyconfig_file(&request.source)
        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    let stated: HashMap<&str, &str> = source
        .dependencies
        .iter()
        .chain(source.builddependencies.iter())
        .map(|dependency| (dependency.name.as_str(), dependency.version.as_str()))
        .collect();
    let derived = crate::package_workflow::version_derived_dependencies(request, &source);

    let mut requests = Vec::new();
    for hole in holes {
        // Only a version the recipe derives from its own: a literal pin the
        // tree lacks is a different gap, and bumping it would invent a
        // version nobody wrote.
        let Some((_, version)) = derived
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&hole.name))
        else {
            continue;
        };
        let in_reach = candidates
            .iter()
            .filter(|candidate| candidate.name.eq_ignore_ascii_case(&hole.name))
            .filter(|candidate| hierarchy.contains(&candidate.toolchain))
            .filter(|candidate| candidate.versionsuffix.as_deref().unwrap_or("").is_empty())
            .filter(|candidate| PathBuf::from(&candidate.easyconfig_path).is_file())
            .collect::<Vec<_>>();
        // The version the recipe was written against, when the tree has it;
        // otherwise the newest one below the version asked for.
        let previous = stated.get(hole.name.as_str()).copied();
        let provider = in_reach
            .iter()
            .find(|candidate| Some(candidate.version.as_str()) == previous)
            .or_else(|| {
                in_reach
                    .iter()
                    .filter(|candidate| cmp_version(&candidate.version, version).is_lt())
                    .max_by(|a, b| cmp_version(&a.version, &b.version))
            });
        let Some(provider) = provider else {
            continue;
        };
        let mut stack_policy = request.stack_policy.clone();
        stack_policy.toolchain = provider.toolchain.clone();
        requests.push((
            BumpPackageRequest {
                source: PathBuf::from(&provider.easyconfig_path),
                toolchain: provider.toolchain.clone(),
                version: Some(version.clone()),
                source_checksum: None,
                easyconfig_roots: request.easyconfig_roots.clone(),
                hierarchy_fixture: None,
                overrides: HashMap::new(),
                stack_policy,
                strict_patches: request.strict_patches,
                package_layers: Vec::new(),
            },
            plan.package.name.clone(),
        ));
    }
    Ok(requests)
}

/// The companion's recipes as robot candidates, at the overlay path they are
/// written to.
fn overlay_candidates(bundle: &PackageBundle) -> Result<Vec<Candidate>, PackageWorkflowError> {
    let mut candidates = Vec::new();
    for emitted in &bundle.easyconfigs {
        let mut resolved = resolve_easyconfig_str(&emitted.text)
            .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
        resolved.easyconfig_path = format!(
            "easyconfigs/{}/{}/{}",
            easyconfig_letter_dir(&bundle.plan.package.name),
            bundle.plan.package.name,
            emitted.filename
        );
        candidates.push(resolved.to_candidate());
    }
    Ok(candidates)
}

/// Where [`write_companions`] put things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenCompanions {
    /// Each companion recipe in the shared overlay, in build order.
    pub easyconfigs: Vec<PathBuf>,
    /// `companions.json`: what was bumped, from where, and for whom.
    pub record: PathBuf,
    /// `build-order.json`: the companions, then the root.
    pub build_order: PathBuf,
}

/// Write each companion into `out_dir`: its plan, SBOM and lock under
/// `packages/<name>-<version>-<toolchain>/`, its recipe and patches into the
/// shared `easyconfigs/` overlay the root is written to. `build-order.json`
/// lists the companions before the root, so a campaign builds them first.
pub fn write_companions(
    root: &PackageBundle,
    companions: &[CompanionBump],
    out_dir: &std::path::Path,
) -> Result<WrittenCompanions, PackageWorkflowError> {
    let io = |path: &std::path::Path, error: std::io::Error| {
        PackageWorkflowError::Io(path.to_path_buf(), error)
    };
    let mut claimed = std::collections::BTreeMap::new();
    let mut easyconfigs = Vec::new();
    let mut order = Vec::new();
    for companion in companions {
        let segment = crate::package_closure::package_layout_segment(&companion.bundle)
            .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
        let artifacts = out_dir.join("packages").join(segment);
        let written = crate::package_workflow::write_package_bundle_into(
            &companion.bundle,
            &artifacts,
            out_dir,
            &mut claimed,
        )?;
        if !companion.fetched.is_empty() {
            let path = artifacts.join("sources.fetch.json");
            let text = serde_json::to_string_pretty(&companion.fetched)
                .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
            std::fs::write(&path, text).map_err(|error| io(&path, error))?;
        }
        for path in written.easyconfigs {
            order.push(crate::package_workflow::relative_posix(
                path.strip_prefix(out_dir).unwrap_or(&path),
            ));
            easyconfigs.push(path);
        }
    }
    let letter = easyconfig_letter_dir(&root.plan.package.name);
    for emitted in &root.easyconfigs {
        order.push(format!(
            "easyconfigs/{letter}/{}/{}",
            root.plan.package.name, emitted.filename
        ));
    }
    let records = companions
        .iter()
        .map(CompanionBump::record)
        .collect::<Vec<_>>();
    let record = out_dir.join("companions.json");
    let text = serde_json::to_string_pretty(&records)
        .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
    std::fs::write(&record, text + "\n").map_err(|error| io(&record, error))?;
    let build_order = out_dir.join("build-order.json");
    let document = crate::package_closure::ClosureBuildOrder {
        schema_version: crate::package_closure::CLOSURE_BUNDLE_SCHEMA_VERSION,
        recipes: order,
    };
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| PackageWorkflowError::Config(error.to_string()))?;
    std::fs::write(&build_order, text + "\n").map_err(|error| io(&build_order, error))?;
    Ok(WrittenCompanions {
        easyconfigs,
        record,
        build_order,
    })
}
