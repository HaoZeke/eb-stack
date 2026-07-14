//! Resolvo-backed lock generation for a materialized package profile.

use crate::domain::{Candidate, DepReq, Policy};
use crate::hierarchy::{filter_candidates_in_hierarchy, hierarchy_for_with_tree};
use crate::package::{
    materialize_profile, DependencyRole, LockedDependency, PackagePlan, ProfileEnvironment,
    ProfileLock, StackPolicy, PROFILE_LOCK_SCHEMA_VERSION,
};
use crate::resolvo_provider::solve_with_stack_policy;
use std::collections::BTreeMap;
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
    let mut runtime_dependencies = Vec::new();
    let mut build_dependencies = Vec::new();
    for dependency in &materialized.dependencies {
        if dependency.virtual_capability.is_some() {
            continue;
        }
        let Some(name) = dependency.eb_name.as_ref() else {
            continue;
        };
        let build_only = dependency
            .roles
            .iter()
            .all(|role| matches!(role, DependencyRole::Build | DependencyRole::Test));
        direct_roles
            .entry(name.clone())
            .and_modify(|existing| *existing &= build_only)
            .or_insert(build_only);
        let requirement = DepReq {
            name: name.clone(),
            version_req: normalize_requirement(dependency.constraint.as_deref()),
            versionsuffix: None,
            toolchain: None,
        };
        if build_only {
            build_dependencies.push(requirement);
        } else {
            runtime_dependencies.push(requirement);
        }
    }

    let hierarchy = hierarchy_for_with_tree(&plan.build.toolchain, hierarchy_fixture, candidates)
        .map_err(|error| ProfileSolveError::Resolve(error.to_string()))?;
    let original_candidates = filter_candidates_in_hierarchy(candidates, &hierarchy);
    let mut universe = original_candidates.clone();
    for candidate in &mut universe {
        // Existing robot recipes are independently built artifacts. Their
        // build-only tools are not co-loaded into the generated package's
        // runtime environment and therefore have separate version scopes.
        // The synthetic profile candidate below retains its direct build
        // requirements because those tools are needed to build this output.
        candidate.builddependencies.clear();
        candidate.toolchain = plan.build.toolchain.clone();
    }
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
    let result = solve_with_stack_policy(&universe, &policy, None, stack_policy)
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
