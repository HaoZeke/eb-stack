//! Catalog-backed recursive package-closure planner.
//!
//! Closes a root foreign package plan over robot candidates and catalog-backed
//! companions. Compatible robot candidates win; catalog packages are planned
//! only for typed robot holes. Package names never appear as control flow.

use crate::domain::Candidate;
use crate::eb_parse::resolve_easyconfig_str;
use crate::package::{OutputRequest, PackagePlan, ProfileEnvironment, StackPolicy};
use crate::package_catalog::{PackageSourceCatalog, PackageSourceProvider};
use crate::package_config::PackageConfigLayer;
use crate::package_solve::{
    unsatisfied_direct_dependencies, ProfileSolveError, UnsatisfiedDirectDependency,
};
use crate::package_workflow::{
    complete_package_bundle, prepare_new_package_plan, NewPackageRequest, PackageBundle,
    PackageWorkflowError,
};
use crate::version::matches_req;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Root package bundle plus generated companions in topological build order.
#[derive(Debug, Clone)]
pub struct PackageClosure {
    pub root: PackageBundle,
    /// Generated companion bundles, dependencies before dependents.
    pub companions: Vec<PackageBundle>,
}

#[derive(Debug, Error)]
pub enum PackageClosureError {
    #[error(transparent)]
    Workflow(#[from] PackageWorkflowError),
    #[error(transparent)]
    Catalog(#[from] PackageCatalogError),
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
}

fn path_display(path: &[String]) -> String {
    path.join(" -> ")
}

/// Plan a root package and recursively close catalog-backed robot holes.
///
/// The EasyBuild robot is parsed once. Each requested root profile is solved
/// against robot candidates plus generated companions. Holes are discovered by
/// inspecting the admitted candidate universe, never by parsing solver text.
pub fn plan_package_closure(
    request: &NewPackageRequest,
    catalog: &PackageSourceCatalog,
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

    let (plan, sbom) = prepare_new_package_plan(request)?;
    let mut state = ClosureState {
        robot: tree.candidates,
        generated: HashMap::new(),
        topo: Vec::new(),
        catalog,
        easyconfig_roots: request.easyconfig_roots.clone(),
        default_stack_policy: request.stack_policy.clone(),
    };

    let root_path = vec![plan.package.name.clone()];
    let root = state.close_package(plan, sbom, &request.stack_policy, &root_path)?;

    let companions = state
        .topo
        .iter()
        .filter_map(|key| state.generated.get(key).map(|entry| entry.bundle.clone()))
        .collect();

    Ok(PackageClosure { root, companions })
}

struct GeneratedEntry {
    bundle: PackageBundle,
    candidates: Vec<Candidate>,
}

struct ClosureState<'a> {
    robot: Vec<Candidate>,
    generated: HashMap<String, GeneratedEntry>,
    topo: Vec<String>,
    catalog: &'a PackageSourceCatalog,
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
        plan: PackagePlan,
        sbom: serde_json::Value,
        stack_policy: &StackPolicy,
        path: &[String],
    ) -> Result<PackageBundle, PackageClosureError> {
        // Fill holes for every requested profile before the final multi-profile solve.
        let profiles: Vec<String> = plan.outputs.iter().map(|o| o.profile.clone()).collect();
        for profile in &profiles {
            self.fill_holes_for_profile(&plan, profile, stack_policy, path)?;
        }

        let candidates = self.universe();
        complete_package_bundle(plan, sbom, &candidates, stack_policy).map_err(Into::into)
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
                self.ensure_companion_for_hole(hole, path)?;
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

        let provider = select_catalog_provider(self.catalog, hole)?;

        if let Some(provided_version) = provider.version.as_deref() {
            if !matches_req(provided_version, &hole.version_req) {
                return Err(PackageClosureError::IncompatibleProviderVersion {
                    name: provider.name.clone(),
                    provided: provided_version.to_string(),
                    required: hole.version_req.clone(),
                });
            }
        }

        let key = companion_key(provider);
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

        let companion_request =
            request_from_provider(provider, &self.easyconfig_roots, &self.default_stack_policy)?;
        let (mut companion_plan, companion_sbom) = prepare_new_package_plan(&companion_request)?;
        select_provider_profile(&mut companion_plan, &provider.profile)?;

        let mut child_path = path.to_vec();
        child_path.push(companion_plan.package.name.clone());

        let companion_policy = companion_request.stack_policy.clone();
        let companion_bundle = self.close_package(
            companion_plan,
            companion_sbom,
            &companion_policy,
            &child_path,
        )?;

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
}

fn select_catalog_provider<'a>(
    catalog: &'a PackageSourceCatalog,
    hole: &UnsatisfiedDirectDependency,
) -> Result<&'a PackageSourceProvider, PackageClosureError> {
    let named = catalog
        .providers()
        .iter()
        .filter(|provider| package_identity(&provider.name) == package_identity(&hole.name))
        .collect::<Vec<_>>();
    if named.is_empty() {
        return Err(PackageClosureError::MissingProvider {
            name: hole.name.clone(),
            version: format!(" ({})", hole.version_req),
        });
    }

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
    match compatible.as_slice() {
        [provider] => Ok(*provider),
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
    }
}

fn request_from_provider(
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
    let stack_policy = if let Some(path) = &provider.stack_policy {
        load_stack_policy(path)?
    } else {
        let mut policy = default_stack_policy.clone();
        policy.toolchain = provider.toolchain.clone();
        policy
    };
    Ok(NewPackageRequest {
        source: provider.source.clone(),
        format: provider.format,
        toolchain: provider.toolchain.clone(),
        source_checksums: provider.source_checksums.clone(),
        package_layers,
        easyconfig_roots: easyconfig_roots.to_vec(),
        stack_policy,
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
