//! Public layered package-source catalog for recursive new-package closure planning.
//!
//! Catalog data maps a canonical EasyBuild provider identity onto the foreign
//! recipe path/format, package-config layers, positional source checksums,
//! requested product profile, and toolchain policy needed to plan that package
//! when the robot tree has no compatible candidate. Paths resolve relative to
//! the catalog file. Package names never appear as Rust control flow.

use crate::domain::Toolchain;
use crate::foreign::ForeignFormat;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const PACKAGE_CATALOG_SCHEMA_VERSION: u32 = 1;

/// One public TOML layer of package-source providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCatalogLayer {
    pub schema_version: u32,
    #[serde(default)]
    pub packages: Vec<PackageSourcePatch>,
    /// Directory used to resolve relative paths when this layer was loaded from disk.
    #[serde(skip)]
    pub base_directory: Option<PathBuf>,
}

/// Partial or complete provider description inside a catalog layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PackageSourcePatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ForeignFormat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_config: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_checksums: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<Toolchain>,
    /// Optional stack-policy TOML/JSON path used when planning this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_policy: Option<String>,
}

/// Fully resolved provider ready for recursive package planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceProvider {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ForeignFormat>,
    #[serde(default)]
    pub package_config: Vec<PathBuf>,
    #[serde(default)]
    pub source_checksums: Vec<String>,
    pub profile: String,
    pub toolchain: Toolchain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_policy: Option<PathBuf>,
}

/// Resolved catalog of unique package-source providers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackageSourceCatalog {
    providers: Vec<PackageSourceProvider>,
}

#[derive(Debug, Error)]
pub enum PackageCatalogError {
    #[error("unsupported package catalog schema version {0}")]
    UnsupportedSchema(u32),
    #[error("package catalog TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("read package catalog {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("package catalog entry name cannot be empty")]
    EmptyPackageName,
    #[error("package catalog entry {name} is missing a foreign recipe source path")]
    MissingSource { name: String },
    #[error("package catalog entry {name} is missing toolchain policy")]
    MissingToolchain { name: String },
    #[error("package catalog entry {name} is incomplete: {reason}")]
    IncompleteProvider { name: String, reason: String },
    #[error("duplicate package-source providers for {name}{version}: entries are ambiguous")]
    DuplicateProvider { name: String, version: String },
    #[error(
        "ambiguous package-source providers for {name}{version}: {count} catalog entries match"
    )]
    AmbiguousProvider {
        name: String,
        version: String,
        count: usize,
    },
    #[error("no package-source catalog entry for {name}{version}")]
    MissingProvider { name: String, version: String },
}

impl PackageCatalogLayer {
    pub fn from_toml_str(input: &str) -> Result<Self, PackageCatalogError> {
        let mut layer: Self = toml::from_str(input)?;
        layer.base_directory = None;
        layer.validate()?;
        Ok(layer)
    }

    pub fn from_path(path: &Path) -> Result<Self, PackageCatalogError> {
        let input = std::fs::read_to_string(path)
            .map_err(|error| PackageCatalogError::Io(path.display().to_string(), error))?;
        let mut layer: Self = toml::from_str(&input)?;
        layer.base_directory = path.parent().map(Path::to_path_buf);
        layer.validate()?;
        Ok(layer)
    }

    fn validate(&self) -> Result<(), PackageCatalogError> {
        if self.schema_version != PACKAGE_CATALOG_SCHEMA_VERSION {
            return Err(PackageCatalogError::UnsupportedSchema(self.schema_version));
        }
        for package in &self.packages {
            if package.name.trim().is_empty() {
                return Err(PackageCatalogError::EmptyPackageName);
            }
        }
        Ok(())
    }
}

impl PackageSourceCatalog {
    pub fn providers(&self) -> &[PackageSourceProvider] {
        &self.providers
    }

    /// Look up a provider by EasyBuild name and optional exact version.
    ///
    /// Names match after case/punctuation normalization. When `version` is
    /// `None`, exactly one catalog entry must match the name.
    pub fn lookup(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<&PackageSourceProvider, PackageCatalogError> {
        let identity = package_identity(name);
        let matches: Vec<&PackageSourceProvider> = self
            .providers
            .iter()
            .filter(|provider| package_identity(&provider.name) == identity)
            .filter(|provider| match version {
                Some(requested) => match provider.version.as_deref() {
                    Some(provided) => provided == requested,
                    // Unversioned providers may fill an exact version request when unique.
                    None => true,
                },
                None => true,
            })
            .collect();

        match matches.as_slice() {
            [provider] => Ok(*provider),
            [] => Err(PackageCatalogError::MissingProvider {
                name: name.to_string(),
                version: version_suffix(version),
            }),
            many => Err(PackageCatalogError::AmbiguousProvider {
                name: name.to_string(),
                version: version_suffix(version),
                count: many.len(),
            }),
        }
    }
}

/// Merge layered catalog patches into unique resolved providers.
///
/// Later layers override earlier fields for the same identity
/// (`normalized name` + optional version). Within a single layer, a repeated
/// identity is an error.
pub fn resolve_package_catalog_layers(
    layers: &[PackageCatalogLayer],
) -> Result<PackageSourceCatalog, PackageCatalogError> {
    let mut order: Vec<String> = Vec::new();
    let mut merged: HashMap<String, MergedEntry> = HashMap::new();

    for layer in layers {
        layer.validate()?;
        let mut seen_in_layer: HashMap<String, ()> = HashMap::new();
        for patch in &layer.packages {
            if patch.name.trim().is_empty() {
                return Err(PackageCatalogError::EmptyPackageName);
            }
            let key = provider_key(&patch.name, patch.version.as_deref());
            if seen_in_layer.insert(key.clone(), ()).is_some() {
                return Err(PackageCatalogError::DuplicateProvider {
                    name: patch.name.clone(),
                    version: version_suffix(patch.version.as_deref()),
                });
            }
            if !merged.contains_key(&key) {
                order.push(key.clone());
                merged.insert(
                    key.clone(),
                    MergedEntry {
                        name: patch.name.clone(),
                        version: patch.version.clone(),
                        source: None,
                        format: None,
                        package_config: Vec::new(),
                        source_checksums: Vec::new(),
                        profile: None,
                        toolchain: None,
                        stack_policy: None,
                    },
                );
            }
            let entry = merged.get_mut(&key).expect("entry inserted");
            entry.name = patch.name.clone();
            if patch.version.is_some() {
                entry.version = patch.version.clone();
            }
            if let Some(source) = &patch.source {
                entry.source = Some(resolve_path(layer.base_directory.as_deref(), source));
            }
            if patch.format.is_some() {
                entry.format = patch.format;
            }
            if !patch.package_config.is_empty() {
                entry.package_config = patch
                    .package_config
                    .iter()
                    .map(|path| resolve_path(layer.base_directory.as_deref(), path))
                    .collect();
            }
            if !patch.source_checksums.is_empty() {
                entry.source_checksums = patch.source_checksums.clone();
            }
            if let Some(profile) = &patch.profile {
                entry.profile = Some(profile.clone());
            }
            if let Some(toolchain) = &patch.toolchain {
                entry.toolchain = Some(toolchain.clone());
            }
            if let Some(stack_policy) = &patch.stack_policy {
                entry.stack_policy =
                    Some(resolve_path(layer.base_directory.as_deref(), stack_policy));
            }
        }
    }

    let mut providers = Vec::with_capacity(order.len());
    for key in order {
        let entry = merged.remove(&key).expect("merged entry");
        providers.push(entry.into_provider()?);
    }

    Ok(PackageSourceCatalog { providers })
}

#[derive(Debug, Clone)]
struct MergedEntry {
    name: String,
    version: Option<String>,
    source: Option<PathBuf>,
    format: Option<ForeignFormat>,
    package_config: Vec<PathBuf>,
    source_checksums: Vec<String>,
    profile: Option<String>,
    toolchain: Option<Toolchain>,
    stack_policy: Option<PathBuf>,
}

impl MergedEntry {
    fn into_provider(self) -> Result<PackageSourceProvider, PackageCatalogError> {
        if self.name.trim().is_empty() {
            return Err(PackageCatalogError::EmptyPackageName);
        }
        let source = self
            .source
            .ok_or_else(|| PackageCatalogError::MissingSource {
                name: self.name.clone(),
            })?;
        let toolchain = self
            .toolchain
            .ok_or_else(|| PackageCatalogError::MissingToolchain {
                name: self.name.clone(),
            })?;
        if toolchain.name.trim().is_empty() || toolchain.version.trim().is_empty() {
            return Err(PackageCatalogError::IncompleteProvider {
                name: self.name.clone(),
                reason: "toolchain name and version must be non-empty".into(),
            });
        }
        let profile = self
            .profile
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "default".into());
        Ok(PackageSourceProvider {
            name: self.name,
            version: self.version.filter(|value| !value.trim().is_empty()),
            source,
            format: self.format,
            package_config: self.package_config,
            source_checksums: self.source_checksums,
            profile,
            toolchain,
            stack_policy: self.stack_policy,
        })
    }
}

fn resolve_path(base_directory: Option<&Path>, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base) = base_directory {
        base.join(path)
    } else {
        path.to_path_buf()
    }
}

fn provider_key(name: &str, version: Option<&str>) -> String {
    format!(
        "{}@{}",
        package_identity(name),
        version.unwrap_or("").trim()
    )
}

fn package_identity(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn version_suffix(version: Option<&str>) -> String {
    match version {
        Some(version) if !version.is_empty() => format!(" {version}"),
        _ => String::new(),
    }
}
