//! Canonical adapter from syntax-aware foreign recipes to package plans.

use crate::domain::Toolchain;
use crate::foreign::{guess_easyblock, ForeignFormat, ForeignRecipe, ForeignRuleKind};
use crate::package::{
    BuildSpec, DependencyIntent, DependencyRole, EasyconfigValue, OutputRequest, PackageMetadata,
    PackageOrigin, PackagePlan, PackageRule, PackageRuleKind, PatchArtifact, ProductProfile,
    Residual, ResidualSeverity, ResidualStage, SourceArtifact, PACKAGE_SCHEMA_VERSION,
};
use std::collections::BTreeMap;

/// Lower a parsed foreign recipe into the canonical package plan for a toolchain.
pub fn package_plan_from_foreign(recipe: &ForeignRecipe, toolchain: &Toolchain) -> PackagePlan {
    let sources = recipe
        .sources
        .iter()
        .map(|source| SourceArtifact {
            url: source.url.clone(),
            filename: source.filename.clone(),
            sha256: source.sha256.clone(),
            git: source.git.clone(),
            tag: source.tag.clone(),
            commit: source.commit.clone(),
            target_directory: source.target_directory.clone(),
            condition: source.condition.clone(),
            provenance: Vec::new(),
        })
        .collect();

    let dependencies = recipe
        .dependencies
        .iter()
        .enumerate()
        .map(|(index, dependency)| DependencyIntent {
            id: format!("dep:{index}:{name}", name = dependency.name),
            name: dependency.name.clone(),
            eb_name: None,
            constraint: canonical_version_constraint(recipe.format, dependency.pin.as_deref()),
            toolchain: None,
            roles: dependency_roles(&dependency.role),
            condition: dependency.condition.clone(),
            virtual_capability: foreign_virtual_capability(&dependency.name),
            solver_excluded: false,
            provenance: dependency.provenance.clone(),
        })
        .collect();

    let rules = recipe
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| PackageRule {
            id: format!("rule:{index}:{}", rule.spec),
            kind: match rule.kind {
                ForeignRuleKind::Conflict => PackageRuleKind::Conflict,
                ForeignRuleKind::Requirement => PackageRuleKind::Requirement,
            },
            spec: rule.spec.clone(),
            when: rule.when.clone(),
            condition: rule.condition.clone(),
            message: rule.message.clone(),
            provenance: rule.provenance.clone(),
        })
        .collect();

    let mut easyblock_notes = Vec::new();
    let easyblock = guess_easyblock(recipe, &mut easyblock_notes);
    let mut features = BTreeMap::new();
    let mut parameters = BTreeMap::new();
    for variant in &recipe.variants {
        match variant.default.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("true") => {
                features.insert(variant.name.clone(), true);
            }
            Some(value) if value.eq_ignore_ascii_case("false") => {
                features.insert(variant.name.clone(), false);
            }
            Some(value) => {
                parameters.insert(variant.name.clone(), value.to_string());
            }
            None => {}
        }
    }

    let config_options = recipe.configopts.iter().cloned().collect::<Vec<_>>();
    let profile = ProductProfile {
        name: "default".into(),
        default: true,
        versionsuffix: Vec::new(),
        platform: None,
        architecture: None,
        features,
        parameters,
        toolchain_options: BTreeMap::new(),
        config_options: config_options.clone(),
        easyconfig_parameters: BTreeMap::new(),
        verification_commands: Vec::new(),
    };

    let mut residuals = recipe
        .residuals
        .iter()
        .enumerate()
        .map(|(index, residual)| Residual {
            id: format!("foreign:{}:{index}", residual.category),
            stage: ResidualStage::Parse,
            category: residual.category.clone(),
            severity: residual.severity,
            summary: residual.summary.clone(),
            evidence: residual.evidence.clone(),
            provenance: residual.provenance.clone(),
        })
        .collect::<Vec<_>>();
    // A constraint the requirement language cannot express is reported here
    // rather than handed to the solver, where it would build an empty version
    // set and make the dependency look unsatisfiable.
    for dependency in &recipe.dependencies {
        let Some(constraint) =
            canonical_version_constraint(recipe.format, dependency.pin.as_deref())
        else {
            continue;
        };
        if let Err(unsupported) = crate::version::parse_requirement(&constraint) {
            residuals.push(Residual {
                id: format!("constraint:unparsed:{}", dependency.name),
                stage: ResidualStage::Normalize,
                category: "unparsed-constraint".into(),
                severity: ResidualSeverity::Judgment,
                summary: format!(
                    "{} requirement {constraint:?} is not expressible, so it does not constrain                      the solve: {unsupported}",
                    dependency.name
                ),
                evidence: dependency.original_spec.clone(),
                provenance: dependency.provenance.first().cloned(),
            });
        }
    }
    if recipe.sources.iter().any(|source| source.sha256.is_none()) {
        residuals.push(Residual {
            id: "source:missing-sha256".into(),
            stage: ResidualStage::Normalize,
            category: "checksum".into(),
            severity: ResidualSeverity::Blocking,
            summary: "one or more source artifacts have no sha256".into(),
            evidence: None,
            provenance: None,
        });
    }

    PackagePlan {
        schema_version: PACKAGE_SCHEMA_VERSION,
        origin: match recipe.format {
            ForeignFormat::CondaForge => PackageOrigin::CondaForge,
            ForeignFormat::Spack => PackageOrigin::Spack,
            ForeignFormat::Pypi => PackageOrigin::Pypi,
            ForeignFormat::Cran => PackageOrigin::Cran,
            ForeignFormat::Cargo => PackageOrigin::Cargo,
            ForeignFormat::Luarocks => PackageOrigin::Luarocks,
            ForeignFormat::Raku => PackageOrigin::Raku,
        },
        package: PackageMetadata {
            name: recipe.name.clone(),
            version: recipe.version.clone(),
            upstream_version: None,
            homepage: recipe.homepage.clone(),
            description: recipe
                .description
                .clone()
                .or_else(|| recipe.summary.clone()),
            license: recipe.license.clone(),
        },
        sources,
        dependencies,
        rules,
        build: BuildSpec {
            toolchain: toolchain.clone(),
            easyblock: Some(easyblock),
            build_systems: recipe.build_system_hints.clone(),
            source_root: None,
            config_options,
            // What the package says it is for, when it says: PyPI states it
            // in Trove classifiers. `lang` is for language runtimes, and
            // upstream uses it for 16 of 588 sampled PythonPackage recipes,
            // so it is a poor blanket answer. A package that states no topic
            // at all falls back to `tools`, which is both upstream's most
            // common class for these recipes (145 of 588) and what it chose
            // for archspec and cppy, neither of which states a topic.
            moduleclass: match recipe.format {
                ForeignFormat::Pypi | ForeignFormat::Cran | ForeignFormat::Cargo => Some(
                    crate::foreign::moduleclass_from_classifiers(&recipe.classifiers)
                        .unwrap_or_else(|| "tools".into()),
                ),
                _ => None,
            },
            patches: recipe
                .patches
                .iter()
                .filter_map(|patch| {
                    let condition = patch.condition.specialize_package_version(&recipe.version);
                    if condition == crate::package::ConditionExpr::Never {
                        return None;
                    }
                    let remote = is_remote_patch(&patch.location);
                    Some(PatchArtifact {
                        filename: if remote {
                            remote_patch_filename(&patch.location)?
                        } else {
                            patch.location.clone()
                        },
                        sha256: patch.sha256.clone(),
                        url: remote.then(|| patch.location.clone()),
                        source: None,
                        condition,
                        resolved_source: None,
                    })
                })
                .collect(),
            easyconfig_parameters: foreign_easyconfig_parameters(recipe),
        },
        profiles: vec![profile],
        outputs: vec![OutputRequest {
            profile: "default".into(),
            stack: toolchain.label(),
        }],
        residuals,
        overlay_extensions: Vec::new(),
        package_index: Default::default(),
    }
}

fn foreign_easyconfig_parameters(recipe: &ForeignRecipe) -> BTreeMap<String, EasyconfigValue> {
    let mut parameters = BTreeMap::new();
    if recipe.format == ForeignFormat::Cran {
        let mut paths = BTreeMap::new();
        paths.insert("files".into(), EasyconfigValue::List(Vec::new()));
        paths.insert(
            "dirs".into(),
            EasyconfigValue::List(vec![EasyconfigValue::String(recipe.name.clone())]),
        );
        parameters.insert("sanity_check_paths".into(), EasyconfigValue::Table(paths));
    }
    if recipe.format == ForeignFormat::Cargo {
        // Host Cargo wrapper env (sccache) is not in the EESSI module graph.
        parameters.insert(
            "preinstallopts".into(),
            EasyconfigValue::String(crate::cargo::eessi_cargo_host_isolation().into()),
        );
    }
    parameters
}

fn is_remote_patch(location: &str) -> bool {
    ["http://", "https://", "ftp://"]
        .iter()
        .any(|prefix| location.starts_with(prefix))
}

fn remote_patch_filename(location: &str) -> Option<String> {
    let path = location
        .split(['?', '#'])
        .next()
        .unwrap_or(location)
        .trim_end_matches('/');
    let filename = path.rsplit('/').next()?;
    (!filename.is_empty()).then(|| filename.to_string())
}

fn canonical_version_constraint(format: ForeignFormat, pin: Option<&str>) -> Option<String> {
    let pin = pin.map(str::trim).filter(|pin| !pin.is_empty())?;
    let version_field = pin.split_whitespace().next().unwrap_or(pin);
    if version_field.contains('*') && !version_field.chars().any(|value| value.is_ascii_digit()) {
        None
    } else if format == ForeignFormat::Spack {
        canonical_spack_version_constraint(version_field)
    } else {
        Some(pin.to_string())
    }
}

fn canonical_spack_version_constraint(version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() {
        return None;
    }
    if let Some(exact) = version.strip_prefix('=') {
        return Some(format!("=={exact}"));
    }
    if matches!(version.chars().next(), Some('<' | '>' | '!' | '~')) {
        return Some(version.to_string());
    }
    if let Some((minimum, maximum)) = version.split_once(':') {
        let minimum = minimum.trim();
        let maximum = maximum.trim();
        let mut terms = Vec::new();
        if !minimum.is_empty() {
            terms.push(format!(">={minimum}"));
        }
        if !maximum.is_empty() {
            terms.push(spack_prefix_successor(maximum).map_or_else(
                || format!("<={maximum}"),
                |successor| format!("<{successor}"),
            ));
        }
        return (!terms.is_empty()).then(|| terms.join(","));
    }
    spack_prefix_successor(version)
        .map(|successor| format!(">={version},<{successor}"))
        .or_else(|| Some(format!("=={version}")))
}

fn spack_prefix_successor(version: &str) -> Option<String> {
    let version = version.strip_suffix(".*").unwrap_or(version);
    let mut components = version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let last = components.last_mut()?;
    *last = last.checked_add(1)?;
    Some(
        components
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}

fn dependency_roles(role: &str) -> Vec<DependencyRole> {
    let mut roles = Vec::new();
    for value in role.split('+').map(str::trim) {
        let role = match value {
            "build" => Some(DependencyRole::Build),
            "host" => Some(DependencyRole::Host),
            "run" => Some(DependencyRole::Run),
            "test" => Some(DependencyRole::Test),
            _ => None,
        };
        if let Some(role) = role {
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
    }
    if roles.is_empty() {
        roles.push(DependencyRole::Run);
    }
    roles
}

fn is_canonical_virtual(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "mpi" | "blas" | "lapack" | "fftw" | "fftw-api" | "c" | "cxx" | "fortran"
    )
}

fn foreign_virtual_capability(name: &str) -> Option<String> {
    let normalized = name.to_ascii_lowercase();
    if is_canonical_virtual(&normalized)
        || matches!(
            normalized.as_str(),
            "libblas" | "libcblas" | "liblapack" | "liblapacke"
        )
        || normalized.starts_with("__")
    {
        Some(name.to_string())
    } else {
        None
    }
}
