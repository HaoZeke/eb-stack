//! Build requirements a Python source states in its own `pyproject.toml`.
//!
//! PEP 517 makes a project name what building it needs: `[build-system]
//! requires` lists the backend and its helpers, and pip installs them before
//! it builds. EasyBuild installs with `--no-build-isolation` and no index, so
//! each of those has to be a module already, or the build stops at "Backend
//! 'maturin' is not available". A recipe written for one release says nothing
//! about the next one's backend: PuLP 3.3 built with setuptools and 4.2 builds
//! a Rust extension with maturin. After a version bump the new sdist is the
//! only evidence, so it is read here and each requirement is mapped onto the
//! module that provides it.

use crate::domain::Candidate;
use crate::provides::overlay_package_identity;
use serde::Serialize;

/// Backends that compile a Cargo crate. With a `Cargo.lock` in the source,
/// the recipe needs Rust and its crates, not only the backend.
const RUST_BACKENDS: &[&str] = &["maturin", "setuptools-rust"];

/// What a source archive declares about how it is built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredBuild {
    /// `[build-system] build-backend`, when stated.
    pub backend: Option<String>,
    /// Project names from `[build-system] requires`, version specifiers
    /// stripped.
    pub requires: Vec<String>,
    /// Whether the source carries a `Cargo.lock` outside vendored code.
    pub cargo_lock: bool,
}

impl DeclaredBuild {
    /// Whether the project builds a Rust extension with a locked crate graph.
    pub fn builds_rust_crate(&self) -> bool {
        self.cargo_lock
            && self.requires.iter().any(|requirement| {
                RUST_BACKENDS.iter().any(|backend| {
                    overlay_package_identity(requirement) == overlay_package_identity(backend)
                })
            })
    }
}

/// The build system declared by the top-level `pyproject.toml` of a source
/// archive, or `None` when the archive has none or it does not parse.
pub fn declared_build(bytes: &[u8]) -> Option<DeclaredBuild> {
    let files = crate::index_bump::files_in_archive(bytes).ok()?;
    declared_build_in(&files)
}

/// As [`declared_build`], over files already read out of an archive.
pub fn declared_build_in(files: &[(String, Vec<u8>)]) -> Option<DeclaredBuild> {
    // The project's own file sits at the archive root or one directory down
    // (`pulp-4.2.0/pyproject.toml`); deeper ones belong to bundled packages.
    let (_, pyproject) = files
        .iter()
        .filter(|(path, _)| {
            path.rsplit('/').next() == Some("pyproject.toml")
                && path.trim_start_matches("./").matches('/').count() <= 1
        })
        .min_by_key(|(path, _)| path.len())?;
    let document: toml::Value = toml::from_str(std::str::from_utf8(pyproject).ok()?).ok()?;
    let build_system = document.get("build-system")?;
    let requires = build_system
        .get("requires")
        .and_then(toml::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(toml::Value::as_str)
                .filter_map(requirement_name)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let backend = build_system
        .get("build-backend")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    let cargo_lock = files.iter().any(|(path, _)| {
        path.rsplit('/').next() == Some("Cargo.lock") && !path.contains("/vendor/")
    });
    Some(DeclaredBuild {
        backend,
        requires,
        cargo_lock,
    })
}

/// The project name of a PEP 508 requirement: `maturin>=1.4,<2.0` is
/// `maturin`, `setuptools[toml]` is `setuptools`. A requirement with an
/// environment marker that excludes this platform still names a project the
/// build may need, so it is kept.
fn requirement_name(requirement: &str) -> Option<String> {
    let name = requirement
        .trim()
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .next()?
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A build requirement and what answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildRequirement {
    /// The project the source names, or `Rust` for a locked crate graph.
    pub requirement: String,
    /// The module to add as a build dependency, or `None` when one of the
    /// recipe's dependencies already provides it.
    pub module: Option<String>,
    /// Which dependency provides it when no module is added.
    pub provided_by: Option<String>,
}

/// Map each declared requirement onto the recipe's dependencies or onto a
/// module in `admitted` (the robot candidates of the target hierarchy).
///
/// `recipe_dependencies` are the candidates the recipe already depends on;
/// their own name and every extension they bundle count as provided, which
/// is how `setuptools` and `wheel` are answered by the Python module. A
/// requirement nothing provides and no module is named after comes back with
/// neither field set.
pub fn map_build_requirements(
    declared: &DeclaredBuild,
    recipe_dependencies: &[&Candidate],
    admitted: &[Candidate],
) -> Vec<BuildRequirement> {
    let mut wanted = declared.requires.clone();
    if declared.builds_rust_crate() {
        wanted.push("Rust".into());
    }
    let mut mapped: Vec<BuildRequirement> = Vec::new();
    for requirement in wanted {
        let identity = overlay_package_identity(&requirement);
        if mapped
            .iter()
            .any(|seen| overlay_package_identity(&seen.requirement) == identity)
        {
            continue;
        }
        let provider = recipe_dependencies.iter().find(|candidate| {
            overlay_package_identity(&candidate.name) == identity
                || candidate
                    .exts_list
                    .iter()
                    .any(|ext| overlay_package_identity(&ext.name) == identity)
        });
        if let Some(provider) = provider {
            mapped.push(BuildRequirement {
                requirement,
                module: None,
                provided_by: Some(provider.name.clone()),
            });
            continue;
        }
        let module = admitted
            .iter()
            .filter(|candidate| !candidate.is_extension_provide())
            .find(|candidate| overlay_package_identity(&candidate.name) == identity)
            .or_else(|| crate::provides::existing_language_provider(&requirement, admitted))
            .map(|candidate| candidate.name.clone());
        mapped.push(BuildRequirement {
            requirement,
            module,
            provided_by: None,
        });
    }
    mapped
}

/// Whether this recipe installs one Python project from its `sources` with
/// pip, so that project's `pyproject.toml` decides what the build needs. A
/// bundle's backends belong to its extensions, not to the bundle source.
pub fn installs_one_python_project(recipe: &crate::eb_parse::ResolvedEasyconfig) -> bool {
    matches!(
        recipe.easyblock.as_deref(),
        Some("PythonPackage" | "CargoPythonPackage")
    )
}

/// The package-config layer that adds every declared build requirement the
/// recipe does not already have, mapped onto modules of the target
/// hierarchy, and the full mapping for the record.
///
/// `recipe` is the bumped recipe, so its dependencies are the ones the new
/// release will be built with.
pub fn build_requirement_layer(
    request: &crate::package_workflow::BumpPackageRequest,
    recipe: &crate::eb_parse::ResolvedEasyconfig,
    declared: &DeclaredBuild,
) -> Result<
    (
        Option<crate::package_config::PackageConfigLayer>,
        Vec<BuildRequirement>,
    ),
    crate::package_workflow::PackageWorkflowError,
> {
    use crate::package_workflow::PackageWorkflowError;
    let roots = request
        .easyconfig_roots
        .iter()
        .map(std::path::PathBuf::as_path)
        .collect::<Vec<_>>();
    let tree = crate::eb_parse::parse_easyconfig_trees(&roots)
        .map_err(|error| PackageWorkflowError::Robot(error.to_string()))?;
    let hierarchy = crate::hierarchy::hierarchy_for_with_tree(
        &request.toolchain,
        request.hierarchy_fixture.as_deref(),
        &tree.candidates,
    )
    .map_err(|error| PackageWorkflowError::Solve(error.to_string()))?;
    let admitted = crate::hierarchy::filter_candidates_in_hierarchy(&tree.candidates, &hierarchy);
    let recipe_dependencies = recipe
        .dependencies
        .iter()
        .chain(recipe.builddependencies.iter())
        .filter_map(|dependency| {
            admitted.iter().find(|candidate| {
                candidate.name == dependency.name && candidate.version == dependency.version
            })
        })
        .collect::<Vec<_>>();
    let mapped = map_build_requirements(declared, &recipe_dependencies, &admitted);
    let stated = |name: &str| {
        recipe
            .dependencies
            .iter()
            .chain(recipe.builddependencies.iter())
            .any(|dependency| dependency.name.eq_ignore_ascii_case(name))
    };
    let mut modules: Vec<&str> = Vec::new();
    for module in mapped.iter().filter_map(|entry| entry.module.as_deref()) {
        if !stated(module) && !modules.contains(&module) {
            modules.push(module);
        }
    }
    if modules.is_empty() {
        return Ok((None, mapped));
    }
    let layer = crate::package_config::PackageConfigLayer {
        schema_version: crate::package_config::PACKAGE_CONFIG_SCHEMA_VERSION,
        package: None,
        build: None,
        dependencies: Some(crate::package_config::DependencyPatch {
            requirements: modules
                .iter()
                .map(|module| crate::package_config::DependencyRequirement {
                    name: (*module).to_string(),
                    constraint: None,
                    roles: vec![crate::package::DependencyRole::Build],
                    features: Default::default(),
                })
                .collect(),
            ..Default::default()
        }),
        profiles: Vec::new(),
    };
    Ok((Some(layer), mapped))
}

/// Residuals for requirements nothing in the target hierarchy answers.
pub fn unmapped_residuals(mapped: &[BuildRequirement]) -> Vec<crate::package::Residual> {
    mapped
        .iter()
        .filter(|entry| entry.module.is_none() && entry.provided_by.is_none())
        .map(|entry| crate::package::Residual {
            id: format!("build-requirement:{}", entry.requirement),
            stage: crate::package::ResidualStage::Resolve,
            category: "build-requirement-unmapped".into(),
            severity: crate::package::ResidualSeverity::Judgment,
            summary: format!(
                "the new source's pyproject.toml requires {} to build, and no module in the \
                 target hierarchy provides it",
                entry.requirement
            ),
            evidence: None,
            provenance: None,
        })
        .collect()
}

/// Turn a `PythonPackage` recipe whose new release builds a locked Rust crate
/// into a `CargoPythonPackage` with an empty `crates` list after `sources`.
/// The crate-list rewrite then fills it, with checksums, from the release's
/// own `Cargo.lock`. Returns whether the text changed.
pub fn adopt_cargo_build(
    bundle: &mut crate::package_workflow::PackageBundle,
    declared: &DeclaredBuild,
) -> Result<bool, crate::package_workflow::PackageWorkflowError> {
    use crate::package_workflow::PackageWorkflowError;
    if !declared.builds_rust_crate() {
        return Ok(false);
    }
    let Some(easyconfig) = bundle.easyconfigs.first_mut() else {
        return Ok(false);
    };
    let recipe = crate::eb_parse::resolve_easyconfig_str(&easyconfig.text)
        .map_err(|error| PackageWorkflowError::EasyBuild(error.to_string()))?;
    if recipe.easyblock.as_deref() != Some("PythonPackage")
        || crate::eb_emit::easyconfig_list_span(&easyconfig.text, "crates")
            .ok()
            .flatten()
            .is_some()
    {
        return Ok(false);
    }
    let rewrite =
        |error: crate::eb_emit::EmitError| PackageWorkflowError::EasyBuild(error.to_string());
    let text = crate::eb_emit::rewrite_easyblock_assignment(&easyconfig.text, "CargoPythonPackage")
        .map_err(rewrite)?;
    let text = crate::eb_emit::insert_assignment_after(&text, "sources", "crates", "[]")
        .map_err(rewrite)?;
    easyconfig.text = text;
    bundle.plan.build.easyblock = Some("CargoPythonPackage".into());
    Ok(true)
}
