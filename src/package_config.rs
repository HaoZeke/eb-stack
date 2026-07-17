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

pub const PACKAGE_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageConfigLayer {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<PackagePatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<DependencyPatch>,
    #[serde(default)]
    pub profiles: Vec<ProfilePatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PackagePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BuildPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easyblock: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_systems: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moduleclass: Option<String>,
    #[serde(default)]
    pub patches_mode: PatchMergeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patches: Option<Vec<PatchArtifact>>,
    #[serde(default)]
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
pub struct DependencyPatch {
    #[serde(default)]
    pub aliases: BTreeMap<String, DependencyAlias>,
    #[serde(default)]
    pub virtuals: BTreeMap<String, String>,
    #[serde(default)]
    pub exclude_from_solve: Vec<String>,
    #[serde(default)]
    pub requirements: Vec<DependencyRequirement>,
}

/// An EasyBuild-side requirement that foreign metadata may not declare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyRequirement {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
    #[serde(default = "default_requirement_roles")]
    pub roles: Vec<DependencyRole>,
    #[serde(default)]
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
        provider: String,
        #[serde(default)]
        constraint: AliasConstraint,
    },
}

impl DependencyAlias {
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
pub enum AliasConstraint {
    #[default]
    Preserve,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versionsuffix: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub toolchain_options: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<String>>,
    #[serde(default)]
    pub easyconfig_parameters: BTreeMap<String, EasyconfigValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_commands: Option<Vec<VerificationCommand>>,
}

impl PackageConfigLayer {
    pub fn from_toml_str(input: &str) -> Result<Self, PackageConfigError> {
        let mut layer: Self = toml::from_str(input)?;
        layer.resolve_patch_sources(None);
        layer.validate()?;
        Ok(layer)
    }

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
pub enum PackageConfigError {
    #[error("unsupported package config schema version {0}")]
    UnsupportedSchema(u32),
    #[error("package config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("read package config {0}: {1}")]
    Io(String, std::io::Error),
    #[error("profile name cannot be empty")]
    EmptyProfileName,
    #[error("package name cannot be empty")]
    EmptyPackageName,
    #[error("package version cannot be empty")]
    EmptyPackageVersion,
    #[error("EasyBuild easyblock cannot be empty")]
    EmptyEasyblock,
    #[error("EasyBuild moduleclass cannot be empty")]
    EmptyModuleclass,
    #[error("invalid EasyBuild parameter name {0:?}")]
    InvalidEasyconfigParameter(String),
    #[error("dependency requirement name cannot be empty")]
    EmptyDependencyRequirement,
    #[error("dependency requirement {0} must have at least one role")]
    EmptyDependencyRoles(String),
    #[error("patch filename must not contain a directory: {0:?}")]
    InvalidPatchFilename(String),
    #[error("profile {profile} inherits missing profile {parent}")]
    MissingParent { profile: String, parent: String },
    #[error("package plan must contain exactly one default profile, found {0}")]
    DefaultProfileCount(usize),
}
