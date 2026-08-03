//! Layered TOML configuration for public package-profile definitions.

use crate::package::{
    is_easyconfig_parameter_name, ConditionExpr, ConditionPredicate, DependencyIntent,
    DependencyRole, EasyconfigValue, OutputRequest, PackagePlan, PatchArtifact, ProductProfile,
    VerificationCommand,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// Schema version this build reads. A layer declaring anything else is
/// rejected rather than interpreted.
pub const PACKAGE_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One TOML layer of package configuration.
///
/// Layers apply in order over a plan extracted from a foreign recipe; a later
/// layer overrides an earlier one. Every section is optional, so a layer states
/// only what it changes.
pub struct PackageConfigLayer {
    /// Must equal [`PACKAGE_CONFIG_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Package identity and metadata overrides.
    pub package: Option<PackagePatch>,
    /// Build-system and easyconfig-parameter overrides.
    pub build: Option<BuildPatch>,
    /// Dependency mapping, exclusions and additions.
    pub dependencies: Option<DependencyPatch>,
    /// Profile definitions, matched to existing profiles by name.
    pub profiles: Vec<ProfilePatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// Package identity overrides. `None` leaves the extracted value alone.
pub struct PackagePatch {
    /// Rename the package, e.g. to EasyBuild's capitalisation.
    pub name: Option<String>,
    /// Override the version. Changing it records the recipe's version as the
    /// upstream one, so the difference stays visible.
    pub version: Option<String>,
    /// Project homepage for the emitted easyconfig.
    pub homepage: Option<String>,
    /// Description for the emitted easyconfig.
    pub description: Option<String>,
    /// License string for the emitted easyconfig.
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// Build-side overrides. Each `None` leaves the extracted value alone; a
/// `Some` list replaces rather than appends, except where `patches_mode` says
/// otherwise.
pub struct BuildPatch {
    /// EasyBuild easyblock class, e.g. `CMakeMake`.
    pub easyblock: Option<String>,
    /// Build systems the recipe uses, when the extraction guessed wrong.
    pub build_systems: Option<Vec<String>>,
    /// Subdirectory of the unpacked source to build from.
    pub source_root: Option<String>,
    /// Configure flags, replacing any extracted set.
    pub config_options: Option<Vec<String>>,
    /// EasyBuild moduleclass, e.g. `lib`.
    pub moduleclass: Option<String>,
    /// Whether `patches` replaces the existing list or merges into it.
    pub patches_mode: PatchMergeMode,
    /// Patch artifacts. An explicitly empty list under `Replace` clears the
    /// patches the source recipe carried.
    pub patches: Option<Vec<PatchArtifact>>,
    /// Raw easyconfig parameters written through verbatim. Keys are validated
    /// against the known EasyBuild parameter names.
    pub easyconfig_parameters: BTreeMap<String, EasyconfigValue>,
}

/// How a package layer combines its patches with artifacts extracted from the
/// source recipe or preceding configuration layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PatchMergeMode {
    /// The layer owns the complete patch list, including an explicitly empty
    /// list used to clear source-recipe patches.
    #[default]
    Replace,
    /// Preserve existing patches, append new filenames, and replace an
    /// existing artifact when the layer supplies the same filename.
    Merge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// How foreign dependency names become EasyBuild ones.
pub struct DependencyPatch {
    /// Foreign name to EasyBuild provider, keyed by the foreign name.
    pub aliases: BTreeMap<String, DependencyAlias>,
    /// Virtual package to the concrete provider that satisfies it.
    pub virtuals: BTreeMap<String, String>,
    /// Foreign names the solve ignores entirely, for things the toolchain
    /// already supplies.
    pub exclude_from_solve: Vec<String>,
    /// Requirements EasyBuild needs that the foreign recipe never declared.
    pub requirements: Vec<DependencyRequirement>,
}

/// An EasyBuild-side requirement that foreign metadata may not declare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyRequirement {
    /// EasyBuild package name required.
    pub name: String,
    /// Version constraint, or `None` for any version.
    pub constraint: Option<String>,
    /// Build, run, or both. Defaults to run-only, and must not be empty.
    pub roles: Vec<DependencyRole>,
    /// Feature flags gating this requirement: it applies only to a profile
    /// whose features match every entry.
    pub features: BTreeMap<String, bool>,
}

fn default_requirement_roles() -> Vec<DependencyRole> {
    vec![DependencyRole::Run]
}

/// Foreign dependency identity mapped to an EasyBuild provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DependencyAlias {
    /// Preserve the foreign version constraint when provider versions have the
    /// same meaning.
    Direct(String),
    /// Control how a component constraint applies to a containing provider.
    Provider {
        /// EasyBuild package that provides the foreign dependency.
        provider: String,
        /// Whether the foreign version constraint carries over.
        constraint: AliasConstraint,
    },
}

impl DependencyAlias {
    /// The EasyBuild package this alias resolves to, either spelling.
    pub fn provider(&self) -> &str {
        match self {
            Self::Direct(provider) | Self::Provider { provider, .. } => provider,
        }
    }

    fn drops_constraint(&self) -> bool {
        matches!(
            self,
            Self::Provider {
                constraint: AliasConstraint::Drop,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
/// Whether a foreign version constraint survives an alias mapping.
pub enum AliasConstraint {
    /// Keep the constraint: the provider's versions mean the same thing.
    #[default]
    Preserve,
    /// Discard it: the provider is a bundle whose versions do not correspond
    /// to the component's, so carrying the constraint would be wrong.
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One build variant of the package, matched to an existing profile by name.
pub struct ProfilePatch {
    /// Profile name, the key layers are matched on. Must not be empty.
    pub name: String,
    /// Profile to inherit settings from. The parent must exist in the plan.
    pub inherits: Option<String>,
    /// Whether this is the default profile. Exactly one profile must be the
    /// default once every layer has applied.
    pub default: Option<bool>,
    /// Versionsuffix fragments, concatenated in order.
    pub versionsuffix: Option<Vec<String>>,
    /// Platform this profile targets, when it is restricted to one.
    pub platform: Option<String>,
    /// Architecture this profile targets, when it is restricted to one.
    pub architecture: Option<String>,
    /// Feature flags, which gate conditional dependencies and rules.
    pub features: BTreeMap<String, bool>,
    /// Free-form parameters consumed by conditions.
    pub parameters: BTreeMap<String, String>,
    /// EasyBuild toolchain options such as `pic` or `openmp`.
    pub toolchain_options: BTreeMap<String, bool>,
    /// Configure flags for this profile, replacing the build-level set.
    pub config_options: Option<Vec<String>>,
    /// Raw easyconfig parameters for this profile only.
    pub easyconfig_parameters: BTreeMap<String, EasyconfigValue>,
    /// Commands proving the build works, run after installation.
    pub verification_commands: Option<Vec<VerificationCommand>>,
}

impl PackageConfigLayer {
    /// Parse a layer from TOML text, resolving patch sources against no
    /// directory and validating the result.
    pub fn from_toml_str(input: &str) -> Result<Self, PackageConfigError> {
        let mut layer: Self = toml::from_str(input)?;
        layer.resolve_patch_sources(None);
        layer.validate()?;
        Ok(layer)
    }

    /// Parse a layer from a file, resolving relative patch paths against the
    /// file's own directory so a layer is movable with its patches.
    pub fn from_path(path: &Path) -> Result<Self, PackageConfigError> {
        let input = std::fs::read_to_string(path)
            .map_err(|error| PackageConfigError::Io(path.display().to_string(), error))?;
        let mut layer: Self = toml::from_str(&input)?;
        layer.resolve_patch_sources(path.parent());
        layer.validate()?;
        Ok(layer)
    }

    fn resolve_patch_sources(&mut self, base_directory: Option<&Path>) {
        let Some(patches) = self.build.as_mut().and_then(|build| build.patches.as_mut()) else {
            return;
        };
        for patch in patches {
            let Some(source) = patch.source.as_deref() else {
                continue;
            };
            let source = Path::new(source);
            patch.resolved_source = Some(if source.is_absolute() {
                source.to_path_buf()
            } else if let Some(base_directory) = base_directory {
                base_directory.join(source)
            } else {
                source.to_path_buf()
            });
        }
    }

    fn validate(&self) -> Result<(), PackageConfigError> {
        if self.schema_version != PACKAGE_CONFIG_SCHEMA_VERSION {
            return Err(PackageConfigError::UnsupportedSchema(self.schema_version));
        }
        for profile in &self.profiles {
            if profile.name.trim().is_empty() {
                return Err(PackageConfigError::EmptyProfileName);
            }
        }
        if self
            .package
            .as_ref()
            .and_then(|package| package.name.as_deref())
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(PackageConfigError::EmptyPackageName);
        }
        if self
            .package
            .as_ref()
            .and_then(|package| package.version.as_deref())
            .is_some_and(|version| version.trim().is_empty())
        {
            return Err(PackageConfigError::EmptyPackageVersion);
        }
        if self
            .build
            .as_ref()
            .and_then(|build| build.easyblock.as_deref())
            .is_some_and(|easyblock| easyblock.trim().is_empty())
        {
            return Err(PackageConfigError::EmptyEasyblock);
        }
        if self
            .build
            .as_ref()
            .and_then(|build| build.moduleclass.as_deref())
            .is_some_and(|moduleclass| moduleclass.trim().is_empty())
        {
            return Err(PackageConfigError::EmptyModuleclass);
        }
        if let Some(build) = &self.build {
            validate_easyconfig_parameter_names(&build.easyconfig_parameters)?;
            if let Some(patches) = &build.patches {
                for patch in patches {
                    let path = Path::new(&patch.filename);
                    if path.file_name().and_then(|name| name.to_str())
                        != Some(patch.filename.as_str())
                    {
                        return Err(PackageConfigError::InvalidPatchFilename(
                            patch.filename.clone(),
                        ));
                    }
                }
            }
        }
        for profile in &self.profiles {
            validate_easyconfig_parameter_names(&profile.easyconfig_parameters)?;
        }
        if let Some(dependencies) = &self.dependencies {
            for requirement in &dependencies.requirements {
                if requirement.name.trim().is_empty() {
                    return Err(PackageConfigError::EmptyDependencyRequirement);
                }
                if requirement.roles.is_empty() {
                    return Err(PackageConfigError::EmptyDependencyRoles(
                        requirement.name.clone(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Apply layers to a plan in order, later layers overriding earlier ones.
///
/// Every layer is validated before it is applied, so a malformed layer fails
/// the call rather than half-modifying the plan.
pub fn apply_package_layers(
    plan: &mut PackagePlan,
    layers: &[PackageConfigLayer],
) -> Result<(), PackageConfigError> {
    for (layer_index, layer) in layers.iter().enumerate() {
        layer.validate()?;
        if let Some(package) = &layer.package {
            if let Some(name) = &package.name {
                plan.package.name = name.clone();
            }
            if let Some(version) = &package.version {
                if version != &plan.package.version && plan.package.upstream_version.is_none() {
                    plan.package.upstream_version = Some(plan.package.version.clone());
                }
                plan.package.version = version.clone();
            }
            if let Some(homepage) = &package.homepage {
                plan.package.homepage = Some(homepage.clone());
            }
            if let Some(description) = &package.description {
                plan.package.description = Some(description.clone());
            }
            if let Some(license) = &package.license {
                plan.package.license = Some(license.clone());
            }
        }
        if let Some(build) = &layer.build {
            if let Some(easyblock) = &build.easyblock {
                plan.build.easyblock = if easyblock.eq_ignore_ascii_case("auto") {
                    None
                } else {
                    Some(easyblock.clone())
                };
            }
            if let Some(build_systems) = &build.build_systems {
                plan.build.build_systems = build_systems.clone();
            }
            if let Some(source_root) = &build.source_root {
                plan.build.source_root = Some(source_root.clone());
            }
            if let Some(config_options) = &build.config_options {
                plan.build.config_options = config_options.clone();
            }
            if let Some(moduleclass) = &build.moduleclass {
                plan.build.moduleclass = Some(moduleclass.clone());
            }
            if let Some(patches) = &build.patches {
                match build.patches_mode {
                    PatchMergeMode::Replace => plan.build.patches = patches.clone(),
                    PatchMergeMode::Merge => {
                        for patch in patches {
                            if let Some(existing) = plan
                                .build
                                .patches
                                .iter_mut()
                                .find(|existing| existing.filename == patch.filename)
                            {
                                *existing = patch.clone();
                            } else {
                                plan.build.patches.push(patch.clone());
                            }
                        }
                    }
                }
            }
            plan.build
                .easyconfig_parameters
                .extend(build.easyconfig_parameters.clone());
        }
        if let Some(dependencies) = &layer.dependencies {
            for dependency in &mut plan.dependencies {
                if let Some(alias) = alias_policy_value(&dependencies.aliases, &dependency.name) {
                    dependency.eb_name = Some(alias.provider().to_string());
                    if alias.drops_constraint() {
                        dependency.constraint = None;
                    }
                }
                if let Some(capability) = policy_value(&dependencies.virtuals, &dependency.name) {
                    dependency.virtual_capability = Some(capability.clone());
                }
                if dependencies
                    .exclude_from_solve
                    .iter()
                    .any(|name| package_identity(name) == package_identity(&dependency.name))
                {
                    dependency.solver_excluded = true;
                }
            }
            for (requirement_index, requirement) in dependencies.requirements.iter().enumerate() {
                ensure_dependency_requirement(plan, requirement, layer_index, requirement_index);
            }
        }
        for patch in &layer.profiles {
            let existing_index = plan
                .profiles
                .iter()
                .position(|profile| profile.name == patch.name);
            let mut profile = match (existing_index, patch.inherits.as_deref()) {
                (Some(index), _) => plan.profiles[index].clone(),
                (None, Some(parent)) => {
                    let mut inherited = plan
                        .profiles
                        .iter()
                        .find(|profile| profile.name == parent)
                        .cloned()
                        .ok_or_else(|| PackageConfigError::MissingParent {
                            profile: patch.name.clone(),
                            parent: parent.to_string(),
                        })?;
                    inherited.name = patch.name.clone();
                    inherited.default = false;
                    inherited
                }
                (None, None) => ProductProfile {
                    name: patch.name.clone(),
                    default: false,
                    versionsuffix: Vec::new(),
                    platform: None,
                    architecture: None,
                    features: BTreeMap::new(),
                    parameters: BTreeMap::new(),
                    toolchain_options: BTreeMap::new(),
                    config_options: Vec::new(),
                    easyconfig_parameters: BTreeMap::new(),
                    verification_commands: Vec::new(),
                },
            };

            if let Some(default) = patch.default {
                profile.default = default;
            }
            if let Some(versionsuffix) = &patch.versionsuffix {
                profile.versionsuffix = versionsuffix.clone();
            }
            if let Some(platform) = &patch.platform {
                profile.platform = Some(platform.clone());
            }
            if let Some(architecture) = &patch.architecture {
                profile.architecture = Some(architecture.clone());
            }
            profile.features.extend(patch.features.clone());
            profile.parameters.extend(patch.parameters.clone());
            profile
                .toolchain_options
                .extend(patch.toolchain_options.clone());
            if let Some(config_options) = &patch.config_options {
                profile.config_options = config_options.clone();
            }
            profile
                .easyconfig_parameters
                .extend(patch.easyconfig_parameters.clone());
            if let Some(verification_commands) = &patch.verification_commands {
                profile.verification_commands = verification_commands.clone();
            }

            match existing_index {
                Some(index) => plan.profiles[index] = profile,
                None => plan.profiles.push(profile),
            }
        }
    }

    let default_count = plan
        .profiles
        .iter()
        .filter(|profile| profile.default)
        .count();
    if default_count != 1 {
        return Err(PackageConfigError::DefaultProfileCount(default_count));
    }
    let stack = plan.build.toolchain.label();
    plan.outputs = plan
        .profiles
        .iter()
        .map(|profile| OutputRequest {
            profile: profile.name.clone(),
            stack: stack.clone(),
        })
        .collect();
    Ok(())
}

fn validate_easyconfig_parameter_names(
    parameters: &BTreeMap<String, EasyconfigValue>,
) -> Result<(), PackageConfigError> {
    for name in parameters.keys() {
        if !is_easyconfig_parameter_name(name) {
            return Err(PackageConfigError::InvalidEasyconfigParameter(name.clone()));
        }
    }
    Ok(())
}

fn ensure_dependency_requirement(
    plan: &mut PackagePlan,
    requirement: &DependencyRequirement,
    layer_index: usize,
    requirement_index: usize,
) {
    let condition = requirement_condition(&requirement.features);
    let identity = package_identity(&requirement.name);
    let existing = plan.dependencies.iter_mut().find(|dependency| {
        let effective_name = dependency.eb_name.as_deref().unwrap_or(&dependency.name);
        package_identity(effective_name) == identity
            && (condition == ConditionExpr::Always || dependency.condition == condition)
    });
    if let Some(dependency) = existing {
        dependency.eb_name = Some(requirement.name.clone());
        if let Some(constraint) = &requirement.constraint {
            dependency.constraint = Some(constraint.clone());
        }
        for role in &requirement.roles {
            if !dependency.roles.contains(role) {
                dependency.roles.push(*role);
            }
        }
        dependency.condition = condition;
        dependency.solver_excluded = false;
        return;
    }

    plan.dependencies.push(DependencyIntent {
        id: format!(
            "package-policy:{layer_index}:{requirement_index}:{}",
            package_identity(&requirement.name)
        ),
        name: requirement.name.clone(),
        eb_name: Some(requirement.name.clone()),
        constraint: requirement.constraint.clone(),
        toolchain: None,
        roles: requirement.roles.clone(),
        condition,
        virtual_capability: None,
        solver_excluded: false,
        provenance: Vec::new(),
    });
}

fn requirement_condition(features: &BTreeMap<String, bool>) -> ConditionExpr {
    if features.is_empty() {
        ConditionExpr::Always
    } else {
        ConditionExpr::All(
            features
                .iter()
                .map(|(name, enabled)| {
                    ConditionExpr::Predicate(ConditionPredicate::Feature {
                        name: name.clone(),
                        enabled: *enabled,
                    })
                })
                .collect(),
        )
    }
}

fn policy_value<'a>(policy: &'a BTreeMap<String, String>, name: &str) -> Option<&'a String> {
    policy.get(name).or_else(|| {
        let identity = package_identity(name);
        policy
            .iter()
            .find(|(key, _)| package_identity(key) == identity)
            .map(|(_, value)| value)
    })
}

fn alias_policy_value<'a>(
    policy: &'a BTreeMap<String, DependencyAlias>,
    name: &str,
) -> Option<&'a DependencyAlias> {
    policy.get(name).or_else(|| {
        let identity = package_identity(name);
        policy
            .iter()
            .find(|(key, _)| package_identity(key) == identity)
            .map(|(_, value)| value)
    })
}

fn package_identity(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Debug, Error)]
/// Why a package configuration layer could not be read or applied.
pub enum PackageConfigError {
    /// The layer declares a schema this build does not read.
    #[error("unsupported package config schema version {0}")]
    UnsupportedSchema(u32),
    /// The file is not valid TOML, or has a field this schema rejects.
    #[error("package config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    /// The file could not be read.
    #[error("read package config {0}: {1}")]
    Io(String, std::io::Error),
    /// A profile has no name, so no layer could ever match it.
    #[error("profile name cannot be empty")]
    EmptyProfileName,
    /// The package override blanks the name.
    #[error("package name cannot be empty")]
    EmptyPackageName,
    /// The package override blanks the version.
    #[error("package version cannot be empty")]
    EmptyPackageVersion,
    /// The build override blanks the easyblock.
    #[error("EasyBuild easyblock cannot be empty")]
    EmptyEasyblock,
    /// The build override blanks the moduleclass.
    #[error("EasyBuild moduleclass cannot be empty")]
    EmptyModuleclass,
    /// A raw easyconfig parameter is not a name EasyBuild recognises, which
    /// would emit a recipe eb rejects.
    #[error("invalid EasyBuild parameter name {0:?}")]
    InvalidEasyconfigParameter(String),
    /// An added requirement has no package name.
    #[error("dependency requirement name cannot be empty")]
    EmptyDependencyRequirement,
    /// An added requirement lists no roles, so it would apply nowhere.
    #[error("dependency requirement {0} must have at least one role")]
    EmptyDependencyRoles(String),
    /// A patch names a path rather than a filename; EasyBuild takes the
    /// basename only.
    #[error("patch filename must not contain a directory: {0:?}")]
    InvalidPatchFilename(String),
    /// A profile inherits from one that does not exist in the plan.
    #[error("profile {profile} inherits missing profile {parent}")]
    MissingParent {
        /// Profile declaring the inheritance.
        profile: String,
        /// Parent it names, which is absent.
        parent: String,
    },
    /// After every layer applied, the plan has no single default profile.
    #[error("package plan must contain exactly one default profile, found {0}")]
    DefaultProfileCount(usize),
}
