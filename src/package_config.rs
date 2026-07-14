//! Layered TOML configuration for public package-profile definitions.

use crate::package::{OutputRequest, PackagePlan, ProductProfile};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

pub const PROFILE_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfigLayer {
    pub schema_version: u32,
    #[serde(default)]
    pub profiles: Vec<ProfilePatch>,
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
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    #[serde(default)]
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    pub toolchain_options: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_options: Option<Vec<String>>,
}

impl ProfileConfigLayer {
    pub fn from_toml_str(input: &str) -> Result<Self, ProfileConfigError> {
        let layer: Self = toml::from_str(input)?;
        layer.validate()?;
        Ok(layer)
    }

    pub fn from_path(path: &Path) -> Result<Self, ProfileConfigError> {
        let input = std::fs::read_to_string(path)
            .map_err(|error| ProfileConfigError::Io(path.display().to_string(), error))?;
        Self::from_toml_str(&input)
    }

    fn validate(&self) -> Result<(), ProfileConfigError> {
        if self.schema_version != PROFILE_CONFIG_SCHEMA_VERSION {
            return Err(ProfileConfigError::UnsupportedSchema(self.schema_version));
        }
        for profile in &self.profiles {
            if profile.name.trim().is_empty() {
                return Err(ProfileConfigError::EmptyProfileName);
            }
        }
        Ok(())
    }
}

pub fn apply_profile_layers(
    plan: &mut PackagePlan,
    layers: &[ProfileConfigLayer],
) -> Result<(), ProfileConfigError> {
    for layer in layers {
        layer.validate()?;
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
                        .ok_or_else(|| ProfileConfigError::MissingParent {
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
                    features: BTreeMap::new(),
                    parameters: BTreeMap::new(),
                    toolchain_options: BTreeMap::new(),
                    config_options: Vec::new(),
                },
            };

            if let Some(default) = patch.default {
                profile.default = default;
            }
            if let Some(versionsuffix) = &patch.versionsuffix {
                profile.versionsuffix = versionsuffix.clone();
            }
            profile.features.extend(patch.features.clone());
            profile.parameters.extend(patch.parameters.clone());
            profile
                .toolchain_options
                .extend(patch.toolchain_options.clone());
            if let Some(config_options) = &patch.config_options {
                profile.config_options = config_options.clone();
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
        return Err(ProfileConfigError::DefaultProfileCount(default_count));
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

#[derive(Debug, Error)]
pub enum ProfileConfigError {
    #[error("unsupported profile config schema version {0}")]
    UnsupportedSchema(u32),
    #[error("profile config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("read profile config {0}: {1}")]
    Io(String, std::io::Error),
    #[error("profile name cannot be empty")]
    EmptyProfileName,
    #[error("profile {profile} inherits missing profile {parent}")]
    MissingParent { profile: String, parent: String },
    #[error("package plan must contain exactly one default profile, found {0}")]
    DefaultProfileCount(usize),
}
