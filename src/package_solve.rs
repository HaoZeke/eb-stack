//! Resolvo-backed lock generation for a materialized package profile.

use crate::domain::{Candidate, DepReq, Policy};
use crate::hierarchy::{
    filter_candidates_in_hierarchy, hierarchy_for_with_tree, is_system_toolchain,
    pick_consensus_version, prefer_non_system_candidates, toolchains_match, ToolchainHierarchy,
};
use crate::package::{
    materialize_profile, DependencyRole, LockedDependency, PackageOrigin, PackagePlan,
    ProfileEnvironment, ProfileLock, StackPin, StackPinMode, StackPolicy,
    PROFILE_LOCK_SCHEMA_VERSION,
};
use crate::provides::{expand_extension_provides, resolve_extension_provider};
use crate::resolvo_provider::solve_curated_with_stack_policy;
use crate::version::matches_req;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
/// Why a profile's dependencies could not be selected.
pub enum ProfileSolveError {
    #[error("profile materialization: {0}")]
    /// The profile's conditions could not be resolved.
    Materialize(String),
    #[error("profile dependency solve: {0}")]
    /// No selection satisfies the profile.
    Resolve(String),
    #[error("Resolvo did not select direct dependency {0}")]
    /// A required dependency has no selection in the result.
    MissingSelection(String),
}

/// A direct profile dependency with no compatible candidate in the universe.
///
/// Detected by inspecting the admitted candidate set — never by parsing
/// Resolvo or solver error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsatisfiedDirectDependency {
    /// Dependency name.
    pub name: String,
    /// Version requirement it must satisfy.
    pub version_req: String,
    /// True for a build-time-only dependency.
    pub build: bool,
}

/// List direct dependencies that have no compatible candidate after hierarchy
/// admission and stack-pin closure expansion.
pub fn unsatisfied_direct_dependencies(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
) -> Result<Vec<UnsatisfiedDirectDependency>, ProfileSolveError> {
    unsatisfied_direct_dependencies_with_hierarchy(
        plan,
        profile_name,
        environment,
        candidates,
        stack_policy,
        None,
    )
}

fn admit_profile_universe(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<
    (
        crate::package::MaterializedProfile,
        crate::hierarchy::ToolchainHierarchy,
        Vec<Candidate>,
    ),
    ProfileSolveError,
> {
    let materialized = materialize_profile(plan, profile_name, environment)
        .map_err(|error| ProfileSolveError::Materialize(error.to_string()))?;
    let hierarchy = hierarchy_for_with_tree(&plan.build.toolchain, hierarchy_fixture, candidates)
        .map_err(|error| ProfileSolveError::Resolve(error.to_string()))?;
    let mut admitted = filter_candidates_in_hierarchy(candidates, &hierarchy);
    admit_stack_pin_closures(candidates, &mut admitted, stack_policy);
    admit_named_dependency_toolchains(candidates, &mut admitted, &materialized.dependencies);
    admitted = expand_extension_provides(admitted);
    Ok((materialized, hierarchy, admitted))
}

/// Like [`unsatisfied_direct_dependencies`], with an optional hierarchy fixture.
pub fn unsatisfied_direct_dependencies_with_hierarchy(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<Vec<UnsatisfiedDirectDependency>, ProfileSolveError> {
    let (materialized, _hierarchy, admitted) = admit_profile_universe(
        plan,
        profile_name,
        environment,
        candidates,
        stack_policy,
        hierarchy_fixture,
    )?;
    let robot_names = robot_name_index(candidates);

    let mut holes = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for dependency in &materialized.dependencies {
        if dependency.solver_excluded || dependency.virtual_capability.is_some() {
            continue;
        }
        let name = dependency
            .eb_name
            .clone()
            .unwrap_or_else(|| match_robot_name_in(&dependency.name, &robot_names));
        let build_only = !dependency.roles.is_empty()
            && dependency
                .roles
                .iter()
                .all(|role| matches!(role, DependencyRole::Build | DependencyRole::Test));
        let version_req = normalize_requirement(dependency.constraint.as_deref());
        if !seen.insert(format!(
            "{}|{version_req}|{}",
            name,
            dependency.versionsuffix.as_deref().unwrap_or("")
        )) {
            continue;
        }
        let has_compatible = admitted.iter().any(|candidate| {
            candidate.name == name
                && candidate_matches_version_req(
                    candidate,
                    &version_req,
                    dependency.versionsuffix.as_deref(),
                )
                && dependency
                    .toolchain
                    .as_ref()
                    .is_none_or(|toolchain| toolchains_match(&candidate.toolchain, toolchain))
                && !(plan.origin == PackageOrigin::EasyBuild
                    && dependency.toolchain.is_none()
                    && is_system_toolchain(&candidate.toolchain)
                    && !is_system_toolchain(&plan.build.toolchain))
        });
        if !has_compatible {
            holes.push(UnsatisfiedDirectDependency {
                name,
                version_req,
                build: build_only,
            });
        }
    }
    Ok(holes)
}

/// Same three spellings Resolvo accepts: version, version+suffix, and the
/// EasyBuild module identity (`5.0.3-GCC-13.3.0`). A hole check that only
/// reads `candidate.version` reports a companion for a pin the solve locks.
fn candidate_matches_version_req(
    candidate: &Candidate,
    version_req: &str,
    versionsuffix: Option<&str>,
) -> bool {
    if let Some(want) = versionsuffix {
        if candidate.versionsuffix.as_deref().unwrap_or("") != want {
            return false;
        }
    }
    candidate_matches_any_spelling(candidate, version_req)
}

/// Version, version+suffix, module without suffix, and module with suffix.
///
/// A CUDA pin stores `==5.0.3-GCC-13.3.0` in the version field and
/// `-CUDA-12.6.0` separately. The module spelling without the suffix must
/// still match.
fn candidate_matches_any_spelling(candidate: &Candidate, version_req: &str) -> bool {
    let suffix = candidate.versionsuffix.as_deref().unwrap_or("");
    if matches_req(&candidate.version, version_req)
        || matches_req(&format!("{}{suffix}", candidate.version), version_req)
    {
        return true;
    }
    if is_system_toolchain(&candidate.toolchain) {
        return false;
    }
    let module = format!(
        "{}-{}-{}",
        candidate.version, candidate.toolchain.name, candidate.toolchain.version
    );
    matches_req(&module, version_req) || matches_req(&format!("{module}{suffix}"), version_req)
}

/// Select dependencies for one profile against a candidate set.
pub fn solve_package_profile(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
) -> Result<ProfileLock, ProfileSolveError> {
    solve_package_profile_with_hierarchy(
        plan,
        profile_name,
        environment,
        candidates,
        stack_policy,
        None,
    )
}

/// As [`solve_package_profile`], resolving against a toolchain hierarchy so
/// a dependency may be taken from a subtoolchain.
pub fn solve_package_profile_with_hierarchy(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<ProfileLock, ProfileSolveError> {
    let (materialized, hierarchy, mut original_candidates) = admit_profile_universe(
        plan,
        profile_name,
        environment,
        candidates,
        stack_policy,
        hierarchy_fixture,
    )?;
    let robot_names = robot_name_index(candidates);
    let synthetic_name = format!("__package_profile__{}__{}", plan.package.name, profile_name);
    let mut direct_roles: BTreeMap<String, bool> = BTreeMap::new();
    let mut implicit_easybuild_dependencies = HashSet::new();
    let mut runtime_dependencies = Vec::new();
    let mut build_dependencies = Vec::new();
    for dependency in &materialized.dependencies {
        if dependency.solver_excluded || dependency.virtual_capability.is_some() {
            continue;
        }
        let name = dependency
            .eb_name
            .clone()
            .unwrap_or_else(|| match_robot_name_in(&dependency.name, &robot_names));
        let build_only = !dependency.roles.is_empty()
            && dependency
                .roles
                .iter()
                .all(|role| matches!(role, DependencyRole::Build | DependencyRole::Test));
        direct_roles
            .entry(name.clone())
            .and_modify(|existing| *existing &= build_only)
            .or_insert(build_only);
        if plan.origin == PackageOrigin::EasyBuild && dependency.toolchain.is_none() {
            implicit_easybuild_dependencies.insert(name.clone());
        }
        let requirement = DepReq {
            name: name.clone(),
            version_req: normalize_requirement(dependency.constraint.as_deref()),
            versionsuffix: dependency.versionsuffix.clone(),
            toolchain: dependency.toolchain.clone(),
        };
        if build_only {
            build_dependencies.push(requirement);
        } else {
            runtime_dependencies.push(requirement);
        }
    }

    // A recipe does not compete with itself. A bundle's own `exts_list` holds
    // packages it installs, and expanding those into provides puts them beside
    // the modules the same recipe depends on: a CUDA wheel bundle carries
    // `nvidia-nccl-cu12` as an extension and depends on the `NCCL` module, and
    // reading its own extension as a second NCCL makes the recipe conflict
    // with itself.
    let own = plan.package.name.clone();
    original_candidates.retain(|candidate| {
        !candidate.is_extension_provide()
            || candidate
                .extension_parent_name()
                .is_none_or(|parent| parent != own)
    });
    // A dependency written without a toolchain means "at my own level", so
    // for a recipe inside a generation the system-level build of that name is
    // the bootstrap one and must not stand in for it. A recipe that is itself
    // at the system toolchain has no other level to take it from: GCCcore
    // 14.2.0 is built at SYSTEM and needs the SYSTEM M4, and dropping those
    // candidates leaves the compilers unbuildable, which is most of what a
    // site builds first.
    let target_is_system = is_system_toolchain(&plan.build.toolchain);
    original_candidates.retain(|candidate| {
        target_is_system
            || !(implicit_easybuild_dependencies.contains(&candidate.name)
                && is_system_toolchain(&candidate.toolchain))
    });
    let mut universe = original_candidates.clone();
    for candidate in &mut universe {
        // Existing robot recipes are already-built modules. Overlay planning
        // only needs their identity (a language runtime, a bundle, …). Their own
        // dependency trees — including filter-deps such as binutils — are not
        // rebuilt and must not make the candidate uninstallable.
        // The synthetic profile candidate below retains its direct
        // requirements because those are what the new recipe will declare.
        candidate.builddependencies.clear();
        if !candidate.is_extension_provide() {
            // Everything the root does not itself ask for goes: those are the
            // module's own already-satisfied internals. What the root asks for
            // stays, because two requirements on one package are a real
            // conflict and dropping the module's side would let an
            // unsatisfiable solve pass as a plan.
            candidate
                .dependencies
                .retain(|dependency| direct_roles.contains_key(&dependency.name));
        }
    }
    scope_cross_generation_pin_closures(&mut universe, stack_policy, &hierarchy);
    universe.push(Candidate {
        name: synthetic_name.clone(),
        version: plan.package.version.clone(),
        toolchain: plan.build.toolchain.clone(),
        versionsuffix: (!materialized.versionsuffix.is_empty())
            .then_some(materialized.versionsuffix.clone()),
        easyconfig_path: format!("__package_profile__/{profile_name}.eb"),
        dependencies: runtime_dependencies,
        builddependencies: build_dependencies,
        exts_list: Vec::new(),
        moduleclass: None,
    });
    let mut stack_policy = stack_policy.clone();
    apply_generation_consensus_pins(
        &mut stack_policy,
        &direct_roles,
        candidates,
        &original_candidates,
        &hierarchy,
        &materialized.dependencies,
    );
    let policy = Policy {
        prefer_installed: false,
        toolchain: plan.build.toolchain.clone(),
        roots: vec![synthetic_name.clone()],
        root_priority: None,
        pins: Vec::new(),
        forbid: Vec::new(),
        objective: "prefer_newer".into(),
        require_upgrade: Vec::new(),
    };
    let result = solve_curated_with_stack_policy(&universe, &policy, None, &stack_policy)
        .map_err(ProfileSolveError::Resolve)?;

    let selected_by_name: HashMap<&str, &Candidate> = result
        .selected
        .iter()
        .map(|candidate| (candidate.name.as_str(), candidate))
        .collect();
    let original_by_path: HashMap<&str, &Candidate> = original_candidates
        .iter()
        .map(|candidate| (candidate.easyconfig_path.as_str(), candidate))
        .collect();
    let mut dependencies: Vec<LockedDependency> = Vec::new();
    let mut seen_providers = HashSet::new();
    for (name, build) in direct_roles {
        let selected = *selected_by_name
            .get(name.as_str())
            .ok_or_else(|| ProfileSolveError::MissingSelection(name.clone()))?;
        let selected = original_by_path
            .get(selected.easyconfig_path.as_str())
            .copied()
            .unwrap_or(selected);
        let provider = resolve_extension_provider(selected, &result.selected);
        let provider = original_by_path
            .get(provider.easyconfig_path.as_str())
            .copied()
            .unwrap_or(provider);
        if !seen_providers.insert(provider.name.clone()) {
            // BTreeMap order is not role order. A build-only extra that
            // sorts first would otherwise lock the parent as build-only and
            // drop a later runtime extra that collapsed to the same bundle.
            if !build {
                if let Some(existing) = dependencies
                    .iter_mut()
                    .find(|dependency| dependency.name == provider.name)
                {
                    existing.build = false;
                }
            }
            continue;
        }
        dependencies.push(LockedDependency {
            name: provider.name.clone(),
            version: provider.version.clone(),
            versionsuffix: provider.versionsuffix.clone(),
            toolchain: provider.toolchain.clone(),
            easyconfig_path: provider.easyconfig_path.clone(),
            build,
        });
    }

    Ok(ProfileLock {
        schema_version: PROFILE_LOCK_SCHEMA_VERSION,
        package: plan.package.name.clone(),
        version: plan.package.version.clone(),
        profile: profile_name.to_string(),
        toolchain: plan.build.toolchain.clone(),
        versionsuffix: materialized.versionsuffix,
        dependencies,
        pin_outcomes: result.pin_outcomes,
        exclusions: result.exclusions,
        solver: "resolvo".into(),
    })
}

struct RobotNameIndex {
    modules: HashMap<String, Vec<String>>,
    all: HashMap<String, Vec<String>>,
}

fn robot_name_index(candidates: &[Candidate]) -> RobotNameIndex {
    let mut modules: HashMap<String, Vec<String>> = HashMap::new();
    let mut all: HashMap<String, Vec<String>> = HashMap::new();
    for candidate in candidates {
        let identity = normalize_package_identity(&candidate.name);
        if !candidate.is_extension_provide() {
            let names = modules.entry(identity.clone()).or_default();
            if !names.iter().any(|name| name == &candidate.name) {
                names.push(candidate.name.clone());
            }
        }
        let names = all.entry(identity).or_default();
        if !names.iter().any(|name| name == &candidate.name) {
            names.push(candidate.name.clone());
        }
    }
    RobotNameIndex { modules, all }
}

fn match_robot_name_in(foreign_name: &str, index: &RobotNameIndex) -> String {
    let identity = normalize_package_identity(foreign_name);
    if let Some(names) = index.modules.get(&identity) {
        if names.len() == 1 {
            return names[0].clone();
        }
    }
    if let Some(names) = index.all.get(&identity) {
        if names.len() == 1 {
            return names[0].clone();
        }
    }
    foreign_name.to_string()
}

fn match_robot_name(foreign_name: &str, candidates: &[Candidate]) -> String {
    match_robot_name_in(foreign_name, &robot_name_index(candidates))
}

fn normalize_package_identity(name: &str) -> String {
    crate::provides::overlay_package_identity(name)
}

/// Admit candidates at a toolchain a dependency names outright.
///
/// A dependency tuple may state its own toolchain, and that is the one case
/// where EasyBuild looks outside the recipe's hierarchy:
/// `intel-compilers` is built at SYSTEM and asks for
/// `('binutils', '2.42', '', ('GCCcore', '14.2.0'))`. A universe filtered to
/// the recipe's own hierarchy cannot contain it, and the compilers of a whole
/// generation then read as unbuildable.
fn admit_named_dependency_toolchains(
    all: &[crate::domain::Candidate],
    admitted: &mut Vec<crate::domain::Candidate>,
    dependencies: &[crate::package::DependencyIntent],
) {
    let known: HashSet<String> = admitted
        .iter()
        .map(|candidate| candidate.easyconfig_path.clone())
        .collect();
    let mut wanted_toolchains: HashMap<String, Vec<&crate::domain::Toolchain>> = HashMap::new();
    let mut module_pins: HashSet<(String, String, String)> = HashSet::new();
    for dependency in dependencies {
        let name = dependency
            .eb_name
            .as_deref()
            .unwrap_or(dependency.name.as_str());
        if let Some(toolchain) = dependency.toolchain.as_ref() {
            wanted_toolchains
                .entry(name.to_string())
                .or_default()
                .push(toolchain);
        }
        if let Some(constraint) = dependency.constraint.as_deref() {
            let pinned = constraint.strip_prefix("==").unwrap_or(constraint);
            if pinned.contains('-') {
                module_pins.insert((
                    name.to_string(),
                    dependency.versionsuffix.clone().unwrap_or_default(),
                    pinned.to_string(),
                ));
            }
        }
    }
    for candidate in all {
        if known.contains(&candidate.easyconfig_path) {
            continue;
        }
        if wanted_toolchains
            .get(&candidate.name)
            .is_some_and(|wanted| {
                wanted
                    .iter()
                    .any(|want| toolchains_match(&candidate.toolchain, want))
            })
        {
            admitted.push(candidate.clone());
            continue;
        }
        // A dependency may name the module instead of the version, with the
        // toolchain inside the string: a system-level application asks for
        // `('OpenMPI', '5.0.3-GCC-13.3.0')`. That names one build as exactly
        // as a toolchain element does, so it admits the same way.
        if module_pins.is_empty() {
            continue;
        }
        let suffix = candidate.versionsuffix.as_deref().unwrap_or("");
        let module_without_suffix = if is_system_toolchain(&candidate.toolchain) {
            continue;
        } else {
            format!(
                "{}-{}-{}",
                candidate.version, candidate.toolchain.name, candidate.toolchain.version
            )
        };
        let module_version = format!("{module_without_suffix}{suffix}");
        if module_pins.contains(&(
            candidate.name.clone(),
            suffix.to_string(),
            module_without_suffix,
        )) || module_pins.contains(&(candidate.name.clone(), suffix.to_string(), module_version))
        {
            admitted.push(candidate.clone());
        }
    }
}

fn admit_stack_pin_closures(
    candidates: &[Candidate],
    admitted: &mut Vec<Candidate>,
    stack_policy: &StackPolicy,
) {
    let mut paths = admitted
        .iter()
        .map(|candidate| candidate.easyconfig_path.clone())
        .collect::<HashSet<_>>();
    let mut queue = VecDeque::new();

    for pin in &stack_policy.pins {
        for candidate in candidates
            .iter()
            .filter(|candidate| stack_pin_candidate_matches(candidate, pin))
        {
            // A version-only pin constrains candidates in the target
            // hierarchy. Crossing toolchain generations requires the pin to
            // identify the foreign toolchain explicitly.
            if pin.toolchain.is_some() && paths.insert(candidate.easyconfig_path.clone()) {
                admitted.push(candidate.clone());
                queue.push_back(candidate.clone());
            }
        }
    }

    let by_name = candidates_by_name(candidates);
    let mut hierarchy_cache = HashMap::new();
    while let Some(parent) = queue.pop_front() {
        let parent_hierarchy =
            cached_hierarchy(&mut hierarchy_cache, &parent.toolchain, candidates);
        for dependency in &parent.dependencies {
            let Some(named) = by_name.get(&dependency.name) else {
                continue;
            };
            for candidate in named.iter().copied().filter(|candidate| {
                dependency_candidate_matches(candidate, dependency, parent_hierarchy)
            }) {
                if paths.insert(candidate.easyconfig_path.clone()) {
                    admitted.push(candidate.clone());
                    queue.push_back(candidate.clone());
                }
            }
        }
    }
}

fn candidates_by_name(candidates: &[Candidate]) -> HashMap<String, Vec<&Candidate>> {
    let mut by_name: HashMap<String, Vec<&Candidate>> = HashMap::new();
    for candidate in candidates {
        by_name
            .entry(candidate.name.clone())
            .or_default()
            .push(candidate);
    }
    by_name
}

fn cached_hierarchy<'a>(
    cache: &'a mut HashMap<String, Option<ToolchainHierarchy>>,
    toolchain: &crate::domain::Toolchain,
    candidates: &[Candidate],
) -> Option<&'a ToolchainHierarchy> {
    let key = toolchain.identity_label();
    cache
        .entry(key)
        .or_insert_with(|| hierarchy_for_with_tree(toolchain, None, candidates).ok())
        .as_ref()
}

fn scope_cross_generation_pin_closures(
    universe: &mut Vec<Candidate>,
    stack_policy: &StackPolicy,
    target_hierarchy: &ToolchainHierarchy,
) {
    let mut root_updates: Vec<(usize, Vec<DepReq>)> = Vec::new();
    let mut scoped_candidates = Vec::new();
    {
        let by_name = candidates_by_name(universe);
        let mut hierarchy_cache = HashMap::new();
        for (pin_index, pin) in stack_policy.pins.iter().enumerate() {
            let root_indexes = universe
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    stack_pin_candidate_matches(candidate, pin)
                        && !target_hierarchy.contains(&candidate.toolchain)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            for root_index in root_indexes {
                let scope = format!("pin{pin_index}");
                let root = universe[root_index].clone();
                let mut queue = VecDeque::new();
                let mut visited = HashSet::new();
                let root_deps = scoped_dependencies(
                    &root,
                    universe,
                    &by_name,
                    &mut hierarchy_cache,
                    &scope,
                    &mut queue,
                    &mut visited,
                );
                root_updates.push((root_index, root_deps));
                while let Some(candidate) = queue.pop_front() {
                    let mut scoped = candidate.clone();
                    scoped.name = scoped_dependency_name(&scope, &candidate.name);
                    scoped.dependencies = scoped_dependencies(
                        &candidate,
                        universe,
                        &by_name,
                        &mut hierarchy_cache,
                        &scope,
                        &mut queue,
                        &mut visited,
                    );
                    scoped.builddependencies.clear();
                    scoped_candidates.push(scoped);
                }
            }
        }
    }
    for (root_index, dependencies) in root_updates {
        universe[root_index].dependencies = dependencies;
    }
    universe.extend(scoped_candidates);
}

fn scoped_dependencies(
    parent: &Candidate,
    candidates: &[Candidate],
    by_name: &HashMap<String, Vec<&Candidate>>,
    hierarchy_cache: &mut HashMap<String, Option<ToolchainHierarchy>>,
    scope: &str,
    queue: &mut VecDeque<Candidate>,
    visited: &mut HashSet<String>,
) -> Vec<DepReq> {
    let parent_hierarchy = cached_hierarchy(hierarchy_cache, &parent.toolchain, candidates);
    parent
        .dependencies
        .iter()
        .map(|dependency| {
            let named = by_name
                .get(&dependency.name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for candidate in named.iter().copied().filter(|candidate| {
                dependency_candidate_matches(candidate, dependency, parent_hierarchy)
            }) {
                let identity = format!(
                    "{}|{}|{}|{}|{}",
                    candidate.name,
                    candidate.version,
                    candidate.toolchain.label(),
                    candidate.versionsuffix.as_deref().unwrap_or_default(),
                    candidate.easyconfig_path
                );
                if visited.insert(identity) {
                    queue.push_back(candidate.clone());
                }
            }
            let mut scoped = dependency.clone();
            scoped.name = scoped_dependency_name(scope, &dependency.name);
            scoped
        })
        .collect()
}

fn scoped_dependency_name(scope: &str, name: &str) -> String {
    format!("__stack_context_{scope}__{name}")
}

fn stack_pin_candidate_matches(candidate: &Candidate, pin: &crate::package::StackPin) -> bool {
    candidate.name == pin.name
        && matches_req(&candidate.version, &pin.version_requirement)
        && pin
            .toolchain
            .as_ref()
            .map(|toolchain| toolchains_match(&candidate.toolchain, toolchain))
            .unwrap_or(true)
        && pin
            .versionsuffix
            .as_deref()
            .map(|versionsuffix| {
                candidate.versionsuffix.as_deref().unwrap_or_default() == versionsuffix
            })
            .unwrap_or(true)
}

fn dependency_candidate_matches(
    candidate: &Candidate,
    dependency: &DepReq,
    parent_hierarchy: Option<&ToolchainHierarchy>,
) -> bool {
    if candidate.name != dependency.name
        || !candidate_matches_any_spelling(candidate, &dependency.version_req)
    {
        return false;
    }
    if dependency
        .versionsuffix
        .as_deref()
        .is_some_and(|suffix| candidate.versionsuffix.as_deref().unwrap_or_default() != suffix)
    {
        return false;
    }
    if let Some(toolchain) = &dependency.toolchain {
        return toolchains_match(&candidate.toolchain, toolchain);
    }
    parent_hierarchy
        .map(|hierarchy| hierarchy.contains(&candidate.toolchain))
        .unwrap_or(false)
}

fn normalize_requirement(constraint: Option<&str>) -> String {
    let Some(constraint) = constraint.map(str::trim).filter(|value| !value.is_empty()) else {
        return ">=0".into();
    };
    if matches!(
        constraint.chars().next(),
        Some('<' | '>' | '=' | '!' | '~' | '^')
    ) {
        return constraint.to_string();
    }
    if let Some((minimum, maximum)) = constraint.split_once(':') {
        return match (minimum.trim(), maximum.trim()) {
            ("", "") => ">=0".into(),
            ("", maximum) => format!("<={maximum}"),
            (minimum, "") => format!(">={minimum}"),
            (minimum, maximum) => format!(">={minimum},<={maximum}"),
        };
    }
    format!("=={constraint}")
}

fn apply_generation_consensus_pins(
    stack_policy: &mut StackPolicy,
    direct_roles: &BTreeMap<String, bool>,
    all_candidates: &[Candidate],
    admitted: &[Candidate],
    hierarchy: &ToolchainHierarchy,
    dependencies: &[crate::package::DependencyIntent],
) {
    let all_counts = crate::hierarchy::count_generation_dep_versions_all(all_candidates, hierarchy);
    for name in direct_roles.keys() {
        if stack_policy.pins.iter().any(|pin| pin.name == *name) {
            continue;
        }
        let empty = std::collections::HashMap::new();
        let counts = all_counts
            .get(&(name.clone(), String::new()))
            .cloned()
            .unwrap_or(empty);
        let admitted_for_name: Vec<&crate::domain::Candidate> = admitted
            .iter()
            .filter(|candidate| candidate.name.eq_ignore_ascii_case(name))
            .collect();
        let preferred = prefer_non_system_candidates(&admitted_for_name);
        let package_bound = dependencies.iter().find_map(|dependency| {
            let dep_name = dependency
                .eb_name
                .as_deref()
                .unwrap_or(dependency.name.as_str());
            (dep_name == name).then_some(dependency.constraint.as_deref())
        });
        let mut eligible = preferred
            .iter()
            .map(|candidate| candidate.version.clone())
            .filter(|version| match package_bound {
                Some(Some(constraint)) => crate::version::matches_req(version, constraint),
                _ => true,
            })
            .collect::<Vec<_>>();
        eligible.sort();
        eligible.dedup();
        let Some(version) = pick_consensus_version(&counts, &eligible) else {
            continue;
        };
        stack_policy.pins.push(StackPin {
            name: name.clone(),
            version_requirement: format!("=={version}"),
            toolchain: None,
            versionsuffix: None,
            mode: StackPinMode::Preferred,
            source: Some("generation-consensus".into()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{
        admit_named_dependency_toolchains, apply_generation_consensus_pins,
        candidate_matches_version_req, dependency_candidate_matches, match_robot_name,
        normalize_requirement,
    };
    use crate::domain::{Candidate, DepReq, Toolchain};
    use crate::hierarchy::ToolchainHierarchy;
    use crate::package::{ConditionExpr, DependencyIntent, DependencyRole};
    use crate::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
    use std::collections::BTreeMap;

    #[test]
    fn normalizes_foreign_version_syntax_for_resolvo() {
        assert_eq!(normalize_requirement(None), ">=0");
        assert_eq!(normalize_requirement(Some("1.8:")), ">=1.8");
        assert_eq!(normalize_requirement(Some(":2.0")), "<=2.0");
        assert_eq!(normalize_requirement(Some("1.8:2.0")), ">=1.8,<=2.0");
        assert_eq!(normalize_requirement(Some("1.14.2")), "==1.14.2");
        assert_eq!(normalize_requirement(Some(">=1.14")), ">=1.14");
        assert_eq!(normalize_requirement(Some("^1.2.3")), "^1.2.3");
    }

    #[test]
    fn match_robot_name_prefers_the_module_over_an_extension() {
        let module = cand("PyTorch", "2.5.1", "foss", "2024a");
        let extra = Candidate {
            easyconfig_path: "Python-bundle-PyPI.eb#ext:torch".into(),
            ..cand("torch", "2.5.1", "foss", "2024a")
        };
        assert_eq!(match_robot_name("pytorch", &[module, extra]), "PyTorch");
        assert_eq!(
            match_robot_name("pytorch", &[cand("PyTorch", "2.5.1", "foss", "2024a")]),
            "PyTorch"
        );
    }

    #[test]
    fn a_lowercase_site_pin_does_not_suppress_consensus() {
        let gcc = cand("Python", "3.12.3", "GCCcore", "13.3.0");
        let mut policy = StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2024a".into(),
            },
            pins: vec![crate::package::StackPin {
                name: "python".into(),
                version_requirement: "==3.12.3".into(),
                toolchain: None,
                versionsuffix: None,
                mode: crate::package::StackPinMode::Preferred,
                source: Some("site".into()),
            }],
            exclusions: Vec::new(),
        };
        let mut roles = BTreeMap::new();
        roles.insert("Python".into(), true);
        let hierarchy = ToolchainHierarchy {
            parent: Toolchain {
                name: "foss".into(),
                version: "2024a".into(),
            },
            members: vec![Toolchain {
                name: "GCCcore".into(),
                version: "13.3.0".into(),
            }],
        };
        apply_generation_consensus_pins(
            &mut policy,
            &roles,
            &[gcc.clone()],
            &[gcc],
            &hierarchy,
            &[],
        );
        assert!(
            policy
                .pins
                .iter()
                .any(|pin| pin.name == "Python"
                    && pin.source.as_deref() == Some("generation-consensus")),
            "{:?}",
            policy.pins
        );
    }

    #[test]
    fn module_form_admission_keeps_the_cuda_suffix_on_the_constraint() {
        let cuda = Candidate {
            versionsuffix: Some("-CUDA-12.6.0".into()),
            ..cand("OpenMPI", "5.0.3", "GCC", "13.3.0")
        };
        let plain = cand("OpenMPI", "5.0.3", "GCC", "13.3.0");
        let dep = DependencyIntent {
            id: "dep:OpenMPI".into(),
            name: "OpenMPI".into(),
            eb_name: None,
            constraint: Some("==5.0.3-GCC-13.3.0".into()),
            toolchain: None,
            versionsuffix: Some("-CUDA-12.6.0".into()),
            roles: vec![DependencyRole::Run],
            condition: ConditionExpr::Always,
            virtual_capability: None,
            solver_excluded: false,
            provenance: Vec::new(),
        };
        let mut admitted = Vec::new();
        admit_named_dependency_toolchains(&[cuda.clone(), plain], &mut admitted, &[dep]);
        assert_eq!(admitted.len(), 1, "{admitted:?}");
        assert_eq!(admitted[0].versionsuffix.as_deref(), Some("-CUDA-12.6.0"));
    }

    #[test]
    fn hole_check_accepts_a_module_form_plus_cuda_suffix() {
        let cuda = Candidate {
            versionsuffix: Some("-CUDA-12.6.0".into()),
            ..cand("OpenMPI", "5.0.3", "GCC", "13.3.0")
        };
        assert!(candidate_matches_version_req(
            &cuda,
            "==5.0.3-GCC-13.3.0",
            Some("-CUDA-12.6.0"),
        ));
        assert!(!candidate_matches_version_req(
            &cuda,
            "==5.0.3-GCC-13.3.0",
            Some(""),
        ));
    }

    #[test]
    fn pin_closure_matches_a_module_form_version_req() {
        let omp = cand("OpenMPI", "5.0.3", "GCC", "13.3.0");
        let dep = DepReq {
            name: "OpenMPI".into(),
            version_req: "==5.0.3-GCC-13.3.0".into(),
            versionsuffix: None,
            toolchain: None,
        };
        let hierarchy = ToolchainHierarchy {
            parent: Toolchain {
                name: "GCC".into(),
                version: "13.3.0".into(),
            },
            members: vec![Toolchain {
                name: "GCC".into(),
                version: "13.3.0".into(),
            }],
        };
        assert!(dependency_candidate_matches(&omp, &dep, Some(&hierarchy)));
        let foss = cand("OpenMPI", "5.0.3", "foss", "2023b");
        assert!(!dependency_candidate_matches(&foss, &dep, Some(&hierarchy)));
    }

    #[test]
    fn named_toolchain_admission_is_only_the_named_package() {
        let binutils = cand("binutils", "2.42", "GCCcore", "14.2.0");
        let m4 = cand("M4", "1.4.19", "GCCcore", "14.2.0");
        let dep = DependencyIntent {
            id: "dep:binutils".into(),
            name: "binutils".into(),
            eb_name: None,
            constraint: Some("==2.42".into()),
            toolchain: Some(Toolchain {
                name: "GCCcore".into(),
                version: "14.2.0".into(),
            }),
            versionsuffix: None,
            roles: vec![DependencyRole::Build],
            condition: ConditionExpr::Always,
            virtual_capability: None,
            solver_excluded: false,
            provenance: Vec::new(),
        };
        let mut admitted = Vec::new();
        admit_named_dependency_toolchains(&[binutils.clone(), m4], &mut admitted, &[dep]);
        assert_eq!(admitted.len(), 1, "{admitted:?}");
        assert_eq!(admitted[0].name, "binutils");
    }

    fn cand(name: &str, ver: &str, tc_name: &str, tc_ver: &str) -> Candidate {
        Candidate {
            name: name.into(),
            version: ver.into(),
            toolchain: Toolchain {
                name: tc_name.into(),
                version: tc_ver.into(),
            },
            versionsuffix: None,
            easyconfig_path: format!("{name}-{ver}-{tc_name}-{tc_ver}.eb"),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
            moduleclass: None,
        }
    }

    #[test]
    fn generation_consensus_prefers_gcccore_over_newer_system() {
        let gcc = cand("CMake", "3.29.3", "GCCcore", "13.3.0");
        let sys = cand("CMake", "3.31.8", "system", "system");
        let mut policy = StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2024a".into(),
            },
            pins: Vec::new(),
            exclusions: Vec::new(),
        };
        let mut roles = BTreeMap::new();
        roles.insert("CMake".into(), true);
        let hierarchy = ToolchainHierarchy {
            parent: Toolchain {
                name: "foss".into(),
                version: "2024a".into(),
            },
            members: vec![
                Toolchain {
                    name: "system".into(),
                    version: "system".into(),
                },
                Toolchain {
                    name: "GCCcore".into(),
                    version: "13.3.0".into(),
                },
            ],
        };
        apply_generation_consensus_pins(
            &mut policy,
            &roles,
            &[gcc.clone(), sys.clone()],
            &[gcc, sys],
            &hierarchy,
            &[],
        );
        let pin = policy
            .pins
            .iter()
            .find(|pin| pin.name == "CMake")
            .expect("CMake pin");
        assert_eq!(pin.version_requirement, "==3.29.3");
    }
}
