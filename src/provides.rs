//! Virtual candidates generated from EasyBuild `exts_list` entries.
//!
//! A scientific-Python bundle lists its array library in `exts_list`. The
//! solver must treat that as a provide: a requirement for that library at a
//! given version is satisfied by selecting the parent bundle, rather than by
//! inventing a standalone module for it
//! easyconfig. The same rule applies to `Python-bundle-PyPI` and
//! `R-bundle-CRAN`.
//!
//! Expansion is idempotent. Synthetic candidates are marked by
//! [`crate::domain::Candidate::EXT_PROVIDE_MARKER`] in `easyconfig_path` and
//! depend on the parent bundle at its exact version.

use crate::domain::{Candidate, DepReq, ExtEntry};
use std::collections::HashSet;

/// Marker embedded in `easyconfig_path` for a virtual extension provide.
pub const EXT_PROVIDE_MARKER: &str = "#ext:";

/// True when `path` names a synthetic extension provide.
pub fn path_is_extension_provide(path: &str) -> bool {
    path.contains(EXT_PROVIDE_MARKER)
}

/// Parent easyconfig path of a synthetic provide, when `path` is one.
pub fn extension_parent_path(path: &str) -> Option<&str> {
    path.split_once(EXT_PROVIDE_MARKER)
        .map(|(parent, _)| parent)
        .filter(|parent| !parent.is_empty())
}

impl Candidate {
    /// Whether this candidate was generated from a parent `exts_list`.
    pub fn is_extension_provide(&self) -> bool {
        path_is_extension_provide(&self.easyconfig_path)
    }

    /// Easyconfig path of the bundle that provides this extension.
    pub fn extension_parent_path(&self) -> Option<&str> {
        extension_parent_path(&self.easyconfig_path)
    }

    /// Parent bundle name, taken from the single exact dependency.
    pub fn extension_parent_name(&self) -> Option<&str> {
        if !self.is_extension_provide() {
            return None;
        }
        self.dependencies.first().map(|dep| dep.name.as_str())
    }
}

/// Add one virtual candidate per `exts_list` entry that is not already present.
///
/// Existing first-class recipes with the same name remain; Resolvo chooses.
/// Entries with an empty name or version are skipped. Candidates that are
/// already provides are not expanded again.
pub fn expand_extension_provides(candidates: &[Candidate]) -> Vec<Candidate> {
    let mut out = candidates.to_vec();
    let mut seen: HashSet<(String, String, String)> = candidates
        .iter()
        .filter(|candidate| candidate.is_extension_provide())
        .filter_map(|candidate| {
            Some((
                candidate.name.clone(),
                candidate.version.clone(),
                candidate.extension_parent_path()?.to_string(),
            ))
        })
        .collect();

    for parent in candidates {
        if parent.is_extension_provide() {
            continue;
        }
        for ext in &parent.exts_list {
            if let Some(child) = provide_from_parent(parent, ext) {
                let key = (
                    child.name.clone(),
                    child.version.clone(),
                    parent.easyconfig_path.clone(),
                );
                if seen.insert(key) {
                    out.push(child);
                }
            }
        }
    }
    out
}

/// Overlay policy: which foreign names share an identity, and which packages
/// an overlay must never pip-install.
///
/// Loaded from `data/overlay-policy.toml` rather than written as match arms,
/// because both are packaging decisions about named packages and the driver
/// contract keeps those out of production code.
#[derive(Debug, Default, serde::Deserialize)]
struct OverlayPolicy {
    #[serde(default)]
    aliases: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    refuse_overlay: RefuseOverlay,
    #[serde(default)]
    python_modules: PythonModules,
    #[serde(default)]
    build_requires: BuildRequires,
    #[serde(default)]
    python_provides: PythonProvides,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PythonProvides {
    #[serde(default)]
    names: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct BuildRequires {
    #[serde(default)]
    ignore: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PythonModules {
    #[serde(default, rename = "crate")]
    crates: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    marker_crates: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct RefuseOverlay {
    #[serde(default)]
    names: Vec<String>,
}

fn overlay_policy() -> &'static OverlayPolicy {
    static POLICY: std::sync::OnceLock<OverlayPolicy> = std::sync::OnceLock::new();
    POLICY.get_or_init(|| {
        toml::from_str(include_str!("../data/overlay-policy.toml"))
            .expect("data/overlay-policy.toml ships with the crate and must parse")
    })
}

/// Whether depending on this crate means the crate builds a Python extension.
pub fn is_python_marker_crate(name: &str) -> bool {
    let identity = crate::package_sources::package_identity(name);
    overlay_policy()
        .python_modules
        .marker_crates
        .iter()
        .any(|marker| crate::package_sources::package_identity(marker) == identity)
}

/// Whether a stated build requirement should stay out of the emitted recipe.
///
/// A Python project names the language and its array library among its build
/// requirements; neither is an EasyBuild build dependency of an overlay, since
/// the stack supplies both.
pub fn ignored_build_requirement(name: &str) -> bool {
    let identity = crate::package_sources::package_identity(name);
    overlay_policy()
        .build_requires
        .ignore
        .iter()
        .any(|ignored| crate::package_sources::package_identity(ignored) == identity)
}

/// Whether the EasyBuild `Python` module already ships this package.
///
/// EasyBuild builds `setuptools`, `pip` and `wheel` into Python itself, so a
/// project that states one as a requirement already has it once it depends on
/// Python. Emitting a dependency instead sends the solver looking for anything
/// that ships the name, and what it finds can be an unrelated application that
/// happens to carry it as an extension.
pub fn shipped_with_python(name: &str) -> bool {
    let identity = crate::package_sources::package_identity(name);
    overlay_policy()
        .python_provides
        .names
        .iter()
        .any(|shipped| crate::package_sources::package_identity(shipped) == identity)
}

/// The Python module a PyO3 crate imports as, when it differs from the crate
/// name.
pub fn python_module_for_crate(crate_name: &str) -> Option<String> {
    overlay_policy()
        .python_modules
        .crates
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(crate_name))
        .map(|(_, module)| module.clone())
}

/// Identity used when matching a PyPI/CRAN name to a robot module or
/// `exts_list` provide, so a package known by two names collapses to one.
pub fn overlay_package_identity(name: &str) -> String {
    let identity = crate::package_sources::package_identity(name);
    overlay_policy()
        .aliases
        .get(&identity)
        .cloned()
        .unwrap_or(identity)
}

/// The module name an alias maps a foreign name to, or the name unchanged.
///
/// [`overlay_package_identity`] answers "are these the same package", which is
/// a normalised key and not something to write into a recipe: the identity of
/// `poetry-core` is `poetrycore`, and no module is called that.
pub fn aliased_module_name(name: &str) -> String {
    let identity = crate::package_sources::package_identity(name);
    overlay_policy()
        .aliases
        .get(&identity)
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

/// True when `--format pypi` must not emit a `PythonBundle` overlay.
///
/// These are toolchain-built extensions. A pip wheel on top of EESSI
/// (or any EasyBuild scientific Python) is the wrong install.
pub fn refuses_pip_overlay(name: &str) -> bool {
    let identity = overlay_package_identity(name);
    overlay_policy()
        .refuse_overlay
        .names
        .iter()
        .any(|refused| crate::package_sources::package_identity(refused) == identity)
}

/// Bundle or first-class module in `candidates` that already ships `name`.
///
/// Prefers an `exts_list` parent (the bundle that ships the package) over a
/// same-named first-class recipe.
pub fn existing_language_provider<'a>(
    name: &str,
    candidates: &'a [Candidate],
) -> Option<&'a Candidate> {
    let identity = overlay_package_identity(name);
    if let Some(parent) = candidates.iter().find(|candidate| {
        !candidate.is_extension_provide()
            && candidate.exts_list.iter().any(|ext| {
                overlay_package_identity(&ext.name) == identity && !ext.version.is_empty()
            })
    }) {
        return Some(parent);
    }
    candidates.iter().find(|candidate| {
        !candidate.is_extension_provide() && overlay_package_identity(&candidate.name) == identity
    })
}

/// Collapse a selected extension provide to its parent bundle candidate.
pub fn resolve_extension_provider<'a>(
    selected: &'a Candidate,
    selected_set: &'a [Candidate],
) -> &'a Candidate {
    let Some(parent_name) = selected.extension_parent_name() else {
        return selected;
    };
    selected_set
        .iter()
        .find(|candidate| candidate.name == parent_name && !candidate.is_extension_provide())
        .unwrap_or(selected)
}

fn provide_from_parent(parent: &Candidate, ext: &ExtEntry) -> Option<Candidate> {
    if ext.name.is_empty() || ext.version.is_empty() {
        return None;
    }
    Some(Candidate {
        name: ext.name.clone(),
        version: ext.version.clone(),
        toolchain: parent.toolchain.clone(),
        versionsuffix: parent.versionsuffix.clone(),
        easyconfig_path: format!("{}{EXT_PROVIDE_MARKER}{}", parent.easyconfig_path, ext.name),
        dependencies: vec![DepReq {
            name: parent.name.clone(),
            version_req: format!("=={}", parent.version),
            versionsuffix: parent.versionsuffix.clone(),
            toolchain: Some(parent.toolchain.clone()),
        }],
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Toolchain;

    fn toolchain() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        }
    }

    fn bundle() -> Candidate {
        Candidate {
            name: "SciPy-bundle".into(),
            version: "2025.06".into(),
            toolchain: toolchain(),
            versionsuffix: None,
            easyconfig_path: "SciPy-bundle-2025.06-foss-2026.1.eb".into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: vec![
                ExtEntry {
                    name: "numpy".into(),
                    version: "2.3.1".into(),
                },
                ExtEntry {
                    name: "scipy".into(),
                    version: "1.15.3".into(),
                },
            ],
        }
    }

    #[test]
    fn expand_creates_one_provide_per_ext() {
        let expanded = expand_extension_provides(&[bundle()]);
        assert_eq!(expanded.len(), 3);
        let numpy = expanded
            .iter()
            .find(|candidate| candidate.name == "numpy")
            .expect("numpy provide");
        assert!(numpy.is_extension_provide());
        assert_eq!(numpy.version, "2.3.1");
        assert_eq!(numpy.extension_parent_name(), Some("SciPy-bundle"));
        assert_eq!(numpy.dependencies[0].version_req, "==2025.06");
    }

    #[test]
    fn expand_is_idempotent() {
        let once = expand_extension_provides(&[bundle()]);
        let twice = expand_extension_provides(&once);
        assert_eq!(once.len(), twice.len());
        let numpy_count = twice
            .iter()
            .filter(|candidate| candidate.name == "numpy")
            .count();
        assert_eq!(numpy_count, 1);
    }

    #[test]
    fn empty_ext_entries_are_skipped() {
        let mut parent = bundle();
        parent.exts_list.push(ExtEntry {
            name: String::new(),
            version: "1.0".into(),
        });
        parent.exts_list.push(ExtEntry {
            name: "blankver".into(),
            version: String::new(),
        });
        let expanded = expand_extension_provides(&[parent]);
        assert!(expanded
            .iter()
            .all(|candidate| candidate.name != "blankver"));
        assert_eq!(
            expanded
                .iter()
                .filter(|candidate| candidate.is_extension_provide())
                .count(),
            2
        );
    }

    #[test]
    fn resolve_extension_provider_returns_parent() {
        let expanded = expand_extension_provides(&[bundle()]);
        let numpy = expanded
            .iter()
            .find(|candidate| candidate.name == "numpy")
            .unwrap();
        let parent = resolve_extension_provider(numpy, &expanded);
        assert_eq!(parent.name, "SciPy-bundle");
        assert!(!parent.is_extension_provide());
    }

    #[test]
    fn torch_and_pytorch_share_overlay_identity() {
        assert_eq!(overlay_package_identity("torch"), "pytorch");
        assert_eq!(overlay_package_identity("PyTorch"), "pytorch");
        assert_eq!(overlay_package_identity("numpy"), "numpy");
        assert!(refuses_pip_overlay("numpy"));
        assert!(refuses_pip_overlay("SciPy"));
        assert!(refuses_pip_overlay("torch"));
        assert!(!refuses_pip_overlay("beautifulsoup4"));
    }

    #[test]
    fn existing_provider_prefers_bundle_over_name_match() {
        let universe = vec![bundle()];
        let provider = existing_language_provider("numpy", &universe).expect("bundle");
        assert_eq!(provider.name, "SciPy-bundle");
        assert!(existing_language_provider("torch", &universe).is_none());
    }

    #[test]
    fn existing_provider_finds_first_class_pytorch() {
        let mut pytorch = bundle();
        pytorch.name = "PyTorch".into();
        pytorch.version = "2.9.1".into();
        pytorch.exts_list.clear();
        pytorch.easyconfig_path = "PyTorch-2.9.1-foss-2026.1.eb".into();
        let universe = [pytorch];
        let provider = existing_language_provider("torch", &universe).expect("module");
        assert_eq!(provider.name, "PyTorch");
    }
}
