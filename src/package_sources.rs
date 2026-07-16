//! Ordered local package source roots for robot-hole discovery.
//!
//! When a package-closure hole has no compatible target-robot candidate and no
//! explicit catalog override, these roots supply EasyBuild recipes at other
//! generations (annual-bump path) and unambiguous conda-forge or Spack recipes
//! (new-package path). Lookup uses package identity normalization and version
//! requirements only — never package-name control flow.

use crate::domain::{Candidate, Toolchain};
use crate::eb_parse::parse_easyconfig_tree;
use crate::foreign::{detect_foreign_format, parse_foreign_path, ForeignFormat};
use crate::hierarchy::{hierarchy_for, is_system_toolchain, known_hierarchy, ToolchainHierarchy};
use crate::package_catalog::{CatalogProviderKind, PackageSourceProvider};
use crate::package_solve::UnsatisfiedDirectDependency;
use crate::version::{cmp_version, matches_req};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const PACKAGE_SOURCE_ROOTS_SCHEMA_VERSION: u32 = 1;

/// Kind of local recipe index under a source root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRootKind {
    /// EasyBuild easyconfig tree (`.eb` files, any generation).
    ///
    /// Public TOML spelling is `easybuild` (not `easy-build`).
    #[serde(rename = "easybuild", alias = "easy-build")]
    EasyBuild,
    /// conda-forge recipes or feedstocks (`meta.yaml` / `recipe.yaml`).
    CondaForge,
    /// Spack package tree (`package.py` files).
    Spack,
}

impl SourceRootKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EasyBuild => "easybuild",
            Self::CondaForge => "conda-forge",
            Self::Spack => "spack",
        }
    }
}

/// One ordered local root in a package-sources configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRoot {
    pub kind: SourceRootKind,
    pub path: PathBuf,
    /// Package-neutral authoring layers applied to foreign recipes discovered
    /// under this root. This is where shared provider aliases and exclusions
    /// live; package-specific policy remains an explicit catalog override.
    #[serde(default)]
    pub package_config: Vec<PathBuf>,
}

/// Layered ordered source roots (package-neutral public configuration).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceRoots {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub source_roots: Vec<SourceRoot>,
}

fn default_schema_version() -> u32 {
    PACKAGE_SOURCE_ROOTS_SCHEMA_VERSION
}

/// One discovered recipe candidate with enough identity for selection evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredCandidate {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub kind: SourceRootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ForeignFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<Toolchain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versionsuffix: Option<String>,
    #[serde(default)]
    pub package_config: Vec<PathBuf>,
    #[serde(default)]
    pub source_checksums: Vec<String>,
}

/// Indexed view of all configured source roots for hole resolution.
#[derive(Debug, Clone, Default)]
pub struct PackageSourceIndex {
    easybuild: Vec<DiscoveredCandidate>,
    foreign: Vec<DiscoveredCandidate>,
}

#[derive(Debug, Error)]
pub enum PackageSourceError {
    #[error("unsupported package-sources schema version {0}")]
    UnsupportedSchema(u32),
    #[error("package-sources TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("read package-sources {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("package-sources entry path cannot be empty")]
    EmptyPath,
    #[error("package-sources package_config path cannot be empty")]
    EmptyPackageConfigPath,
    #[error("EasyBuild source roots cannot declare package_config layers")]
    EasyBuildPackageConfig,
    #[error("package-sources discovery: {0}")]
    Discovery(String),
}

/// Typed discovery failure with candidate evidence for operators and tests.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProviderDiscoveryError {
    #[error(
        "no package source for {name}{version_req}: scanned roots but found no compatible recipe"
    )]
    Missing {
        name: String,
        version_req: String,
        candidates: Vec<DiscoveredCandidate>,
    },
    #[error("ambiguous package sources for {name}{version_req}: {count} compatible candidates")]
    Ambiguous {
        name: String,
        version_req: String,
        count: usize,
        candidates: Vec<DiscoveredCandidate>,
    },
    #[error(
        "discovered provider {name} version {provided} does not satisfy requirement {required}"
    )]
    Incompatible {
        name: String,
        provided: String,
        required: String,
        candidates: Vec<DiscoveredCandidate>,
    },
}

impl PackageSourceRoots {
    pub fn from_toml_str(input: &str) -> Result<Self, PackageSourceError> {
        let roots: Self = toml::from_str(input)?;
        roots.validate()?;
        Ok(roots)
    }

    pub fn from_path(path: &Path) -> Result<Self, PackageSourceError> {
        let input = std::fs::read_to_string(path)
            .map_err(|error| PackageSourceError::Io(path.display().to_string(), error))?;
        let mut roots: Self = toml::from_str(&input)?;
        let base = path.parent().map(Path::to_path_buf);
        for root in &mut roots.source_roots {
            if root.path.as_os_str().is_empty() {
                return Err(PackageSourceError::EmptyPath);
            }
            if !root.path.is_absolute() {
                if let Some(base_dir) = &base {
                    root.path = base_dir.join(&root.path);
                }
            }
            for config in &mut root.package_config {
                if config.as_os_str().is_empty() {
                    return Err(PackageSourceError::EmptyPackageConfigPath);
                }
                if !config.is_absolute() {
                    if let Some(base_dir) = &base {
                        *config = base_dir.join(&*config);
                    }
                }
            }
        }
        roots.validate()?;
        Ok(roots)
    }

    pub fn is_empty(&self) -> bool {
        self.source_roots.is_empty()
    }

    fn validate(&self) -> Result<(), PackageSourceError> {
        if self.schema_version != PACKAGE_SOURCE_ROOTS_SCHEMA_VERSION {
            return Err(PackageSourceError::UnsupportedSchema(self.schema_version));
        }
        for root in &self.source_roots {
            if root.path.as_os_str().is_empty() {
                return Err(PackageSourceError::EmptyPath);
            }
            if root.kind == SourceRootKind::EasyBuild && !root.package_config.is_empty() {
                return Err(PackageSourceError::EasyBuildPackageConfig);
            }
            if root
                .package_config
                .iter()
                .any(|path| path.as_os_str().is_empty())
            {
                return Err(PackageSourceError::EmptyPackageConfigPath);
            }
        }
        Ok(())
    }

    /// Append roots from another layer; later roots retain later-list order.
    pub fn extend_from(&mut self, other: &PackageSourceRoots) {
        self.source_roots.extend(other.source_roots.iter().cloned());
    }

    pub fn push(&mut self, kind: SourceRootKind, path: PathBuf) {
        self.source_roots.push(SourceRoot {
            kind,
            path,
            package_config: Vec::new(),
        });
    }
}

impl PackageSourceIndex {
    /// Walk every configured root and index parseable package identities.
    pub fn build(roots: &PackageSourceRoots) -> Result<Self, PackageSourceError> {
        let mut index = Self::default();
        for root in &roots.source_roots {
            match root.kind {
                SourceRootKind::EasyBuild => index.index_easybuild(&root.path)?,
                SourceRootKind::CondaForge => index.index_conda(root)?,
                SourceRootKind::Spack => index.index_spack(root)?,
            }
        }
        Ok(index)
    }

    pub fn easybuild_candidates(&self) -> &[DiscoveredCandidate] {
        &self.easybuild
    }

    pub fn foreign_candidates(&self) -> &[DiscoveredCandidate] {
        &self.foreign
    }

    fn index_easybuild(&mut self, root: &Path) -> Result<(), PackageSourceError> {
        if !root.exists() {
            return Ok(());
        }
        let tree = parse_easyconfig_tree(root).map_err(|error| {
            PackageSourceError::Discovery(format!("easybuild root {}: {error}", root.display()))
        })?;
        for candidate in tree.candidates {
            self.easybuild.push(discovered_from_eb_candidate(candidate));
        }
        Ok(())
    }

    fn index_conda(&mut self, root: &SourceRoot) -> Result<(), PackageSourceError> {
        if !root.path.exists() {
            return Ok(());
        }
        let mut paths = Vec::new();
        collect_named_files(
            &root.path,
            &["meta.yaml", "meta.yml", "recipe.yaml", "recipe.yml"],
            &mut paths,
        )
        .map_err(|error| PackageSourceError::Io(root.path.display().to_string(), error))?;
        paths.sort();
        for path in paths {
            match parse_foreign_path(&path, Some(ForeignFormat::CondaForge)) {
                Ok(recipe) => {
                    self.foreign.push(DiscoveredCandidate {
                        name: recipe.name,
                        version: recipe.version,
                        path,
                        kind: SourceRootKind::CondaForge,
                        format: Some(ForeignFormat::CondaForge),
                        toolchain: None,
                        versionsuffix: None,
                        package_config: root.package_config.clone(),
                        source_checksums: foreign_checksums(&recipe.sha256, &recipe.sources),
                    });
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }

    fn index_spack(&mut self, root: &SourceRoot) -> Result<(), PackageSourceError> {
        if !root.path.exists() {
            return Ok(());
        }
        let mut paths = Vec::new();
        collect_named_files(&root.path, &["package.py"], &mut paths)
            .map_err(|error| PackageSourceError::Io(root.path.display().to_string(), error))?;
        paths.sort();
        for path in paths {
            // Only index paths that look like Spack package recipes.
            if detect_foreign_format(&path) != Some(ForeignFormat::Spack) {
                continue;
            }
            match parse_foreign_path(&path, Some(ForeignFormat::Spack)) {
                Ok(recipe) => {
                    self.foreign.push(DiscoveredCandidate {
                        name: recipe.name,
                        version: recipe.version,
                        path,
                        kind: SourceRootKind::Spack,
                        format: Some(ForeignFormat::Spack),
                        toolchain: None,
                        versionsuffix: None,
                        package_config: root.package_config.clone(),
                        source_checksums: foreign_checksums(&recipe.sha256, &recipe.sources),
                    });
                }
                Err(_) => continue,
            }
        }
        Ok(())
    }
}

fn discovered_from_eb_candidate(candidate: Candidate) -> DiscoveredCandidate {
    DiscoveredCandidate {
        name: candidate.name,
        version: candidate.version,
        path: PathBuf::from(&candidate.easyconfig_path),
        kind: SourceRootKind::EasyBuild,
        format: None,
        toolchain: Some(candidate.toolchain),
        versionsuffix: candidate.versionsuffix,
        package_config: Vec::new(),
        source_checksums: Vec::new(),
    }
}

fn foreign_checksums(
    primary: &Option<String>,
    sources: &[crate::foreign::ForeignSource],
) -> Vec<String> {
    if !sources.is_empty() {
        return sources
            .iter()
            .filter_map(|source| source.sha256.clone())
            .collect();
    }
    primary.iter().cloned().collect()
}

fn collect_named_files(
    root: &Path,
    names: &[&str],
    out: &mut Vec<PathBuf>,
) -> Result<(), std::io::Error> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                let lower = name.to_ascii_lowercase();
                if names.iter().any(|wanted| *wanted == lower) {
                    out.push(path);
                }
            }
        }
    }
    Ok(())
}

/// Map a source EasyBuild recipe's toolchain family onto the target generation.
///
/// Subtoolchains stay in-family: a `GCCcore` source becomes the `GCCcore`
/// member of the requested target hierarchy (for example `GCCcore-15.2.0` under
/// `foss-2026.1`), never the composite parent. Composite sources of the parent
/// family retarget to the parent. SYSTEM stays SYSTEM.
pub fn map_source_toolchain_to_target(
    source: Option<&Toolchain>,
    target_parent: &Toolchain,
    hierarchy: Option<&ToolchainHierarchy>,
) -> Toolchain {
    let Some(source) = source else {
        return target_parent.clone();
    };
    if is_system_toolchain(source) {
        return Toolchain {
            name: "system".into(),
            version: String::new(),
        };
    }
    if source.name == target_parent.name {
        return target_parent.clone();
    }
    if let Some(member) = hierarchy.and_then(|h| hierarchy_family_member(h, &source.name)) {
        return member;
    }
    if let Ok(built_in) = hierarchy_for(target_parent, None) {
        if let Some(member) = hierarchy_family_member(&built_in, &source.name) {
            return member;
        }
    }
    if let Some(known) = known_hierarchy(target_parent) {
        if let Some(member) = hierarchy_family_member(&known, &source.name) {
            return member;
        }
    }
    // Unknown family with no hierarchy member: do not invent a composite parent.
    // Keep the source family name and adopt the target parent version only when
    // the names already match (handled above); otherwise preserve the source
    // toolchain identity so admission/bump evidence stays honest.
    source.clone()
}

fn hierarchy_family_member(hierarchy: &ToolchainHierarchy, family: &str) -> Option<Toolchain> {
    hierarchy
        .members
        .iter()
        .find(|member| member.name == family)
        .cloned()
}

/// Resolve a hole to a single provider: EasyBuild cross-generation first, then
/// foreign. Fail with typed evidence when zero or many compatible candidates remain.
///
/// `target_parent` is the root package toolchain. EasyBuild bumps remap the
/// source recipe's toolchain family through `hierarchy` (see
/// [`map_source_toolchain_to_target`]).
pub fn discover_provider_for_hole(
    index: &PackageSourceIndex,
    hole: &UnsatisfiedDirectDependency,
    target_parent: &Toolchain,
    hierarchy: Option<&ToolchainHierarchy>,
) -> Result<PackageSourceProvider, ProviderDiscoveryError> {
    let identity = package_identity(&hole.name);

    // Robot-first is enforced by the caller (only holes reach here). Prefer an
    // EasyBuild recipe at another generation over foreign archives.
    let eb_matches = index
        .easybuild
        .iter()
        .filter(|candidate| package_identity(&candidate.name) == identity)
        .filter(|candidate| matches_req(&candidate.version, &hole.version_req))
        .cloned()
        .collect::<Vec<_>>();

    if !eb_matches.is_empty() {
        return select_easybuild_provider(&eb_matches, hole, target_parent, hierarchy);
    }

    let foreign_matches = index
        .foreign
        .iter()
        .filter(|candidate| package_identity(&candidate.name) == identity)
        .filter(|candidate| matches_req(&candidate.version, &hole.version_req))
        .cloned()
        .collect::<Vec<_>>();

    select_foreign_provider(&foreign_matches, hole, target_parent)
}

fn select_easybuild_provider(
    matches: &[DiscoveredCandidate],
    hole: &UnsatisfiedDirectDependency,
    target_parent: &Toolchain,
    hierarchy: Option<&ToolchainHierarchy>,
) -> Result<PackageSourceProvider, ProviderDiscoveryError> {
    let mut unique = Vec::new();
    for candidate in matches {
        if !unique.contains(candidate) {
            unique.push(candidate.clone());
        }
    }

    // A dependency without a suffix or toolchain-family requirement cannot
    // choose between independently installable EasyBuild variants.
    let mut identities = unique
        .iter()
        .map(|candidate| {
            (
                candidate.version.as_str(),
                candidate.versionsuffix.as_deref().unwrap_or(""),
                candidate
                    .toolchain
                    .as_ref()
                    .map(|toolchain| toolchain.name.as_str())
                    .unwrap_or(""),
            )
        })
        .collect::<Vec<_>>();
    identities.sort();
    identities.dedup();
    if identities.len() > 1 {
        return Err(ProviderDiscoveryError::Ambiguous {
            name: hole.name.clone(),
            version_req: format!(" ({})", hole.version_req),
            count: unique.len(),
            candidates: unique,
        });
    }

    // Within one version/family/suffix identity, the newest source toolchain
    // generation is the most direct annual-bump baseline. Equal-generation
    // recipes at different paths remain ambiguous because their mechanics may
    // differ even though their filenames claim the same identity.
    unique.sort_by(|left, right| {
        cmp_version(
            left.toolchain
                .as_ref()
                .map(|toolchain| toolchain.version.as_str())
                .unwrap_or(""),
            right
                .toolchain
                .as_ref()
                .map(|toolchain| toolchain.version.as_str())
                .unwrap_or(""),
        )
    });
    let selected = unique.last().expect("non-empty easybuild matches");
    let selected_generation = selected
        .toolchain
        .as_ref()
        .map(|toolchain| toolchain.version.as_str())
        .unwrap_or("");
    let same_generation = unique
        .iter()
        .filter(|candidate| {
            candidate
                .toolchain
                .as_ref()
                .map(|toolchain| toolchain.version.as_str())
                .unwrap_or("")
                == selected_generation
        })
        .cloned()
        .collect::<Vec<_>>();
    if same_generation.len() > 1 {
        return Err(ProviderDiscoveryError::Ambiguous {
            name: hole.name.clone(),
            version_req: format!(" ({})", hole.version_req),
            count: same_generation.len(),
            candidates: same_generation,
        });
    }
    let bump_toolchain =
        map_source_toolchain_to_target(selected.toolchain.as_ref(), target_parent, hierarchy);
    Ok(PackageSourceProvider {
        name: selected.name.clone(),
        provider: CatalogProviderKind::EasyBuildBump,
        version: Some(selected.version.clone()),
        source: selected.path.clone(),
        format: None,
        package_config: Vec::new(),
        source_checksums: selected.source_checksums.clone(),
        profile: "default".into(),
        toolchain: bump_toolchain,
        stack_policy: None,
    })
}

fn select_foreign_provider(
    matches: &[DiscoveredCandidate],
    hole: &UnsatisfiedDirectDependency,
    toolchain: &Toolchain,
) -> Result<PackageSourceProvider, ProviderDiscoveryError> {
    let mut unique = Vec::new();
    for candidate in matches {
        if !unique.contains(candidate) {
            unique.push(candidate.clone());
        }
    }
    match unique.as_slice() {
        [] => Err(ProviderDiscoveryError::Missing {
            name: hole.name.clone(),
            version_req: format!(" ({})", hole.version_req),
            candidates: Vec::new(),
        }),
        [selected] => Ok(provider_from_foreign(selected, toolchain)),
        many => Err(ProviderDiscoveryError::Ambiguous {
            name: hole.name.clone(),
            version_req: format!(" ({})", hole.version_req),
            count: many.len(),
            candidates: many.to_vec(),
        }),
    }
}

fn provider_from_foreign(
    selected: &DiscoveredCandidate,
    toolchain: &Toolchain,
) -> PackageSourceProvider {
    PackageSourceProvider {
        name: selected.name.clone(),
        provider: CatalogProviderKind::Foreign,
        version: Some(selected.version.clone()),
        source: selected.path.clone(),
        format: selected.format,
        package_config: selected.package_config.clone(),
        source_checksums: selected.source_checksums.clone(),
        profile: "default".into(),
        toolchain: toolchain.clone(),
        stack_policy: None,
    }
}

pub fn package_identity(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn package_identity_normalizes_punctuation() {
        assert_eq!(package_identity("Foo-Bar"), package_identity("foobar"));
        assert_eq!(package_identity("Lib_X"), package_identity("libx"));
    }

    #[test]
    fn source_roots_toml_roundtrip_and_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let eb = root.join("eb");
        fs::create_dir_all(&eb).unwrap();
        let config = root.join("sources.toml");
        write(
            &config,
            r#"
schema_version = 1

[[source_roots]]
kind = "easybuild"
path = "eb"

[[source_roots]]
kind = "conda-forge"
path = "conda"

[[source_roots]]
kind = "spack"
path = "spack"
"#,
        );
        let roots = PackageSourceRoots::from_path(&config).expect("load");
        assert_eq!(roots.source_roots.len(), 3);
        assert_eq!(roots.source_roots[0].kind, SourceRootKind::EasyBuild);
        assert_eq!(roots.source_roots[0].path, eb);
        assert_eq!(roots.source_roots[1].kind, SourceRootKind::CondaForge);
        assert_eq!(roots.source_roots[2].kind, SourceRootKind::Spack);
    }

    #[test]
    fn index_discovers_easybuild_conda_and_spack_without_name_branches() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let eb = root.join("eb");
        let conda = root.join("conda");
        let spack = root.join("spack");
        write(
            &eb.join("LeafLib-1.2-foss-2023b.eb"),
            "name = 'LeafLib'\nversion = '1.2'\ntoolchain = {'name': 'foss', 'version': '2023b'}\n",
        );
        write(
            &conda.join("leaflib").join("meta.yaml"),
            r#"
package:
  name: leaflib
  version: "1.2"
source:
  url: https://example.invalid/leaflib-1.2.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#,
        );
        write(
            &spack.join("packages").join("leaflib").join("package.py"),
            r#"
from spack.package import *
class Leaflib(Package):
    version("1.2", sha256="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
"#,
        );
        let mut roots = PackageSourceRoots {
            schema_version: 1,
            source_roots: Vec::new(),
        };
        roots.push(SourceRootKind::EasyBuild, eb);
        roots.push(SourceRootKind::CondaForge, conda);
        roots.push(SourceRootKind::Spack, spack);
        let index = PackageSourceIndex::build(&roots).expect("index");
        assert_eq!(index.easybuild_candidates().len(), 1);
        assert_eq!(index.foreign_candidates().len(), 2);
        assert!(index
            .easybuild_candidates()
            .iter()
            .any(|c| package_identity(&c.name) == package_identity("LeafLib")));
    }

    #[test]
    fn closure_index_reuses_candidates_for_covered_easybuild_roots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("robot");
        write(&root.join("broken.eb"), "name =\n");
        let candidate = Candidate {
            name: "Reusable".into(),
            version: "1.0".into(),
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2026.1".into(),
            },
            versionsuffix: None,
            easyconfig_path: root
                .join("Reusable-1.0-foss-2026.1.eb")
                .display()
                .to_string(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
        };
        let mut roots = PackageSourceRoots {
            schema_version: 1,
            source_roots: Vec::new(),
        };
        roots.push(SourceRootKind::EasyBuild, root.clone());
        let index = PackageSourceIndex::build_with_easybuild_candidates(
            &roots,
            std::slice::from_ref(&candidate),
            &[root],
        )
        .expect("covered root is not parsed twice");
        assert_eq!(index.easybuild_candidates().len(), 1);
        assert_eq!(index.easybuild_candidates()[0].name, candidate.name);
    }

    #[test]
    fn discovery_prefers_easybuild_over_foreign_for_same_identity() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let eb = root.join("eb");
        let conda = root.join("conda");
        write(
            &eb.join("MidLib-3.0-foss-2023b.eb"),
            "name = 'MidLib'\nversion = '3.0'\ntoolchain = {'name': 'foss', 'version': '2023b'}\n",
        );
        write(
            &conda.join("midlib").join("meta.yaml"),
            r#"
package:
  name: midlib
  version: "3.0"
source:
  url: https://example.invalid/midlib-3.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#,
        );
        let mut roots = PackageSourceRoots {
            schema_version: 1,
            source_roots: Vec::new(),
        };
        roots.push(SourceRootKind::EasyBuild, eb);
        roots.push(SourceRootKind::CondaForge, conda);
        let index = PackageSourceIndex::build(&roots).expect("index");
        let hole = UnsatisfiedDirectDependency {
            name: "MidLib".into(),
            version_req: ">=3.0".into(),
            build: false,
        };
        let target = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let provider = discover_provider_for_hole(&index, &hole, &target, None).expect("discover");
        assert_eq!(provider.provider, CatalogProviderKind::EasyBuildBump);
        assert_eq!(provider.version.as_deref(), Some("3.0"));
        assert_eq!(provider.toolchain.name, "foss");
        assert_eq!(provider.toolchain.version, "2026.1");
    }

    #[test]
    fn easybuild_gcccore_source_maps_to_target_hierarchy_member() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let eb = root.join("eb");
        write(
            &eb.join("CoreLib-1.0-GCCcore-13.3.0.eb"),
            "name = 'CoreLib'\n\
             version = '1.0'\n\
             toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
             sources = ['CoreLib-1.0.tar.gz']\n\
             checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n",
        );
        let mut roots = PackageSourceRoots {
            schema_version: 1,
            source_roots: Vec::new(),
        };
        roots.push(SourceRootKind::EasyBuild, eb);
        let index = PackageSourceIndex::build(&roots).expect("index");
        let hole = UnsatisfiedDirectDependency {
            name: "CoreLib".into(),
            version_req: ">=1.0".into(),
            build: false,
        };
        let target = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let hierarchy = known_hierarchy(&target).expect("foss-2026.1 hierarchy");
        let provider =
            discover_provider_for_hole(&index, &hole, &target, Some(&hierarchy)).expect("discover");
        assert_eq!(provider.provider, CatalogProviderKind::EasyBuildBump);
        assert_eq!(
            provider.toolchain.name, "GCCcore",
            "must stay in GCCcore family, not inherit foss"
        );
        assert_eq!(
            provider.toolchain.version, "15.2.0",
            "must use foss-2026.1 hierarchy GCCcore member"
        );
        // Direct mapper unit contract (same input as discovery).
        let mapped = map_source_toolchain_to_target(
            Some(&Toolchain {
                name: "GCCcore".into(),
                version: "13.3.0".into(),
            }),
            &target,
            Some(&hierarchy),
        );
        assert_eq!(mapped.name, "GCCcore");
        assert_eq!(mapped.version, "15.2.0");
    }

    #[test]
    fn foreign_only_conda_and_spack_are_selected_when_unique() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let conda = root.join("conda");
        write(
            &conda.join("uniquelib").join("recipe.yaml"),
            r#"
package:
  name: uniquelib
  version: "0.4"
source:
  url: https://example.invalid/uniquelib-0.4.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#,
        );
        let mut roots = PackageSourceRoots {
            schema_version: 1,
            source_roots: Vec::new(),
        };
        roots.push(SourceRootKind::CondaForge, conda);
        let index = PackageSourceIndex::build(&roots).expect("index");
        let hole = UnsatisfiedDirectDependency {
            name: "UniqueLib".into(),
            version_req: ">=0.4".into(),
            build: false,
        };
        let target = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let provider = discover_provider_for_hole(&index, &hole, &target, None).expect("conda");
        assert_eq!(provider.provider, CatalogProviderKind::Foreign);
        assert_eq!(provider.format, Some(ForeignFormat::CondaForge));
    }

    #[test]
    fn ambiguous_foreign_providers_return_candidate_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let conda = root.join("conda");
        let spack = root.join("spack");
        write(
            &conda.join("dupe").join("meta.yaml"),
            r#"
package:
  name: dupe
  version: "1.0"
source:
  url: https://example.invalid/dupe-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
"#,
        );
        write(
            &spack.join("packages").join("dupe").join("package.py"),
            r#"
from spack.package import *
class Dupe(Package):
    version("1.0", sha256="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
"#,
        );
        let mut roots = PackageSourceRoots {
            schema_version: 1,
            source_roots: Vec::new(),
        };
        roots.push(SourceRootKind::CondaForge, conda);
        roots.push(SourceRootKind::Spack, spack);
        let index = PackageSourceIndex::build(&roots).expect("index");
        let hole = UnsatisfiedDirectDependency {
            name: "dupe".into(),
            version_req: ">=1.0".into(),
            build: false,
        };
        let target = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let err = discover_provider_for_hole(&index, &hole, &target, None).expect_err("ambiguous");
        match err {
            ProviderDiscoveryError::Ambiguous {
                count, candidates, ..
            } => {
                assert_eq!(count, 2);
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {other}"),
        }
    }
}
