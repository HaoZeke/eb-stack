//! Resolvo-backed lock generation for a materialized package profile.

use crate::domain::{Candidate, DepReq, Policy};
use crate::hierarchy::{
    filter_candidates_in_hierarchy, hierarchy_for_with_tree, is_system_toolchain, toolchains_match,
    ToolchainHierarchy,
};
use crate::package::{
    materialize_profile, DependencyRole, LockedDependency, PackageOrigin, PackagePlan,
    ProfileEnvironment, ProfileLock, StackPolicy, PROFILE_LOCK_SCHEMA_VERSION,
};
use crate::resolvo_provider::solve_curated_with_stack_policy;
use crate::version::matches_req;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProfileSolveError {
    #[error("profile materialization: {0}")]
    Materialize(String),
    #[error("profile dependency solve: {0}")]
    Resolve(String),
    #[error("Resolvo did not select direct dependency {0}")]
    MissingSelection(String),
}

/// A direct profile dependency with no compatible candidate in the universe.
///
/// Detected by inspecting the admitted candidate set — never by parsing
/// Resolvo or solver error text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsatisfiedDirectDependency {
    pub name: String,
    pub version_req: String,
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

/// Like [`unsatisfied_direct_dependencies`], with an optional hierarchy fixture.
pub fn unsatisfied_direct_dependencies_with_hierarchy(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<Vec<UnsatisfiedDirectDependency>, ProfileSolveError> {
    let materialized = materialize_profile(plan, profile_name, environment)
        .map_err(|error| ProfileSolveError::Materialize(error.to_string()))?;
    let hierarchy = hierarchy_for_with_tree(&plan.build.toolchain, hierarchy_fixture, candidates)
        .map_err(|error| ProfileSolveError::Resolve(error.to_string()))?;
    let mut admitted = filter_candidates_in_hierarchy(candidates, &hierarchy);
    admit_stack_pin_closures(candidates, &mut admitted, stack_policy);

    let mut holes = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for dependency in &materialized.dependencies {
        if dependency.solver_excluded || dependency.virtual_capability.is_some() {
            continue;
        }
        let name = dependency
            .eb_name
            .clone()
            .unwrap_or_else(|| match_robot_name(&dependency.name, candidates));
        let build_only = !dependency.roles.is_empty()
            && dependency
                .roles
                .iter()
                .all(|role| matches!(role, DependencyRole::Build | DependencyRole::Test));
        if !seen.insert(name.clone()) {
            continue;
        }
        let version_req = normalize_requirement(dependency.constraint.as_deref());
        let has_compatible = admitted.iter().any(|candidate| {
            package_identities_match(&candidate.name, &name)
                && matches_req(&candidate.version, &version_req)
                && dependency
                    .toolchain
                    .as_ref()
                    .is_none_or(|toolchain| toolchains_match(&candidate.toolchain, toolchain))
                && !(plan.origin == PackageOrigin::EasyBuild
                    && dependency.toolchain.is_none()
                    && is_system_toolchain(&candidate.toolchain))
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

fn package_identities_match(left: &str, right: &str) -> bool {
    normalize_package_identity(left) == normalize_package_identity(right)
}

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

pub fn solve_package_profile_with_hierarchy(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
    candidates: &[Candidate],
    stack_policy: &StackPolicy,
    hierarchy_fixture: Option<&Path>,
) -> Result<ProfileLock, ProfileSolveError> {
    let materialized = materialize_profile(plan, profile_name, environment)
        .map_err(|error| ProfileSolveError::Materialize(error.to_string()))?;
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
            .unwrap_or_else(|| match_robot_name(&dependency.name, candidates));
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
            versionsuffix: None,
            toolchain: dependency.toolchain.clone(),
        };
        if build_only {
            build_dependencies.push(requirement);
        } else {
            runtime_dependencies.push(requirement);
        }
    }

    let hierarchy = hierarchy_for_with_tree(&plan.build.toolchain, hierarchy_fixture, candidates)
        .map_err(|error| ProfileSolveError::Resolve(error.to_string()))?;
    let mut original_candidates = filter_candidates_in_hierarchy(candidates, &hierarchy);
    admit_stack_pin_closures(candidates, &mut original_candidates, stack_policy);
    original_candidates.retain(|candidate| {
        !(implicit_easybuild_dependencies.contains(&candidate.name)
            && is_system_toolchain(&candidate.toolchain))
    });
    let mut universe = original_candidates.clone();
    for candidate in &mut universe {
        // Existing robot recipes are independently built artifacts. Their
        // build-only tools are not co-loaded into the generated package's
        // runtime environment and therefore have separate version scopes.
        // The synthetic profile candidate below retains its direct build
        // requirements because those tools are needed to build this output.
        candidate.builddependencies.clear();
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
    });
    let policy = Policy {
        toolchain: plan.build.toolchain.clone(),
        roots: vec![synthetic_name.clone()],
        root_priority: None,
        pins: Vec::new(),
        forbid: Vec::new(),
        objective: "prefer_newer".into(),
        require_upgrade: Vec::new(),
    };
    let result = solve_curated_with_stack_policy(&universe, &policy, None, stack_policy)
        .map_err(ProfileSolveError::Resolve)?;

    let mut dependencies = Vec::new();
    for (name, build) in direct_roles {
        let selected = result
            .selected
            .iter()
            .find(|candidate| candidate.name == name)
            .ok_or_else(|| ProfileSolveError::MissingSelection(name.clone()))?;
        let selected = original_candidates
            .iter()
            .find(|candidate| candidate.easyconfig_path == selected.easyconfig_path)
            .unwrap_or(selected);
        dependencies.push(LockedDependency {
            name,
            version: selected.version.clone(),
            versionsuffix: selected.versionsuffix.clone(),
            toolchain: selected.toolchain.clone(),
            easyconfig_path: selected.easyconfig_path.clone(),
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

fn match_robot_name(foreign_name: &str, candidates: &[Candidate]) -> String {
    let identity = normalize_package_identity(foreign_name);
    let mut names = candidates
        .iter()
        .filter(|candidate| normalize_package_identity(&candidate.name) == identity)
        .map(|candidate| candidate.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    if names.len() == 1 {
        names[0].to_string()
    } else {
        foreign_name.to_string()
    }
}

fn normalize_package_identity(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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

    while let Some(parent) = queue.pop_front() {
        let parent_hierarchy = hierarchy_for_with_tree(&parent.toolchain, None, candidates).ok();
        for dependency in &parent.dependencies {
            for candidate in candidates.iter().filter(|candidate| {
                dependency_candidate_matches(candidate, dependency, parent_hierarchy.as_ref())
            }) {
                if paths.insert(candidate.easyconfig_path.clone()) {
                    admitted.push(candidate.clone());
                    queue.push_back(candidate.clone());
                }
            }
        }
    }
}

fn scope_cross_generation_pin_closures(
    universe: &mut Vec<Candidate>,
    stack_policy: &StackPolicy,
    target_hierarchy: &ToolchainHierarchy,
) {
    let base = universe.clone();
    let mut scoped_candidates = Vec::new();
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
            universe[root_index].dependencies =
                scoped_dependencies(&root, &base, &scope, &mut queue, &mut visited);
            while let Some(candidate) = queue.pop_front() {
                let mut scoped = candidate.clone();
                scoped.name = scoped_dependency_name(&scope, &candidate.name);
                scoped.dependencies =
                    scoped_dependencies(&candidate, &base, &scope, &mut queue, &mut visited);
                scoped.builddependencies.clear();
                scoped_candidates.push(scoped);
            }
        }
    }
    universe.extend(scoped_candidates);
}

fn scoped_dependencies(
    parent: &Candidate,
    candidates: &[Candidate],
    scope: &str,
    queue: &mut VecDeque<Candidate>,
    visited: &mut HashSet<String>,
) -> Vec<DepReq> {
    let parent_hierarchy = hierarchy_for_with_tree(&parent.toolchain, None, candidates).ok();
    parent
        .dependencies
        .iter()
        .map(|dependency| {
            for candidate in candidates.iter().filter(|candidate| {
                dependency_candidate_matches(candidate, dependency, parent_hierarchy.as_ref())
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
        || !matches_req(&candidate.version, &dependency.version_req)
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
        .unwrap_or(true)
}

fn normalize_requirement(constraint: Option<&str>) -> String {
    let Some(constraint) = constraint.map(str::trim).filter(|value| !value.is_empty()) else {
        return ">=0".into();
    };
    if matches!(constraint.chars().next(), Some('<' | '>' | '=' | '!' | '~')) {
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

#[cfg(test)]
mod tests {
    use super::normalize_requirement;

    #[test]
    fn normalizes_foreign_version_syntax_for_resolvo() {
        assert_eq!(normalize_requirement(None), ">=0");
        assert_eq!(normalize_requirement(Some("1.8:")), ">=1.8");
        assert_eq!(normalize_requirement(Some(":2.0")), "<=2.0");
        assert_eq!(normalize_requirement(Some("1.8:2.0")), ">=1.8,<=2.0");
        assert_eq!(normalize_requirement(Some("1.14.2")), "==1.14.2");
        assert_eq!(normalize_requirement(Some(">=1.14")), ">=1.14");
    }
}
