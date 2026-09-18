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
use std::collections::{HashMap, HashSet};

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
pub fn expand_extension_provides(mut candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut seen: HashSet<(String, String, String)> = candidates
        .iter()
        .filter(|candidate| candidate.is_extension_provide())
        .map(|candidate| {
            (
                candidate.name.clone(),
                candidate.version.clone(),
                candidate.extension_parent_path().unwrap_or("").to_string(),
            )
        })
        .collect();

    let mut extra = Vec::new();
    for parent in &candidates {
        if parent.is_extension_provide() {
            continue;
        }
        for ext in &parent.exts_list {
            if let Some(child) = provide_from_parent(parent, ext) {
                let key = (
                    ext.name.clone(),
                    ext.version.clone(),
                    parent.easyconfig_path.clone(),
                );
                if seen.insert(key) {
                    extra.push(child);
                }
            }
        }
    }
    candidates.extend(extra);
    candidates
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
    #[serde(default)]
    mpi_test_ranks: MpiTestRanks,
}

#[derive(Debug, Default, serde::Deserialize)]
struct MpiTestRanks {
    #[serde(default)]
    names: Vec<String>,
    #[serde(default)]
    ranks: u32,
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
        .find(|(known, _)| {
            crate::package_sources::package_identity(known)
                == crate::package_sources::package_identity(crate_name)
        })
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

/// MPI test rank count the overlay policy pins for this package, when the
/// easyblock would otherwise take `$parallel`.
pub fn mpi_test_rank_pin(package: &str) -> Option<u32> {
    let identity = crate::package_sources::package_identity(package);
    let policy = overlay_policy();
    let listed = policy
        .mpi_test_ranks
        .names
        .iter()
        .any(|name| crate::package_sources::package_identity(name) == identity);
    if !listed || policy.mpi_test_ranks.ranks == 0 {
        return None;
    }
    Some(policy.mpi_test_ranks.ranks)
}

/// Whether this recipe text is a CUDA + usempi build that still leaves MPI
/// test ranks to the easyblock default.
pub fn missing_mpi_test_rank_pin(package: &str, text: &str) -> bool {
    mpi_test_rank_pin(package).is_some()
        && recipe_has_usempi(text)
        && recipe_has_cuda(text)
        && !recipe_has_mpi_numprocs(text)
}

fn recipe_has_usempi(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("usempi") && (lower.contains("true") || lower.contains("'true'"))
}

fn recipe_has_cuda(text: &str) -> bool {
    text.to_ascii_lowercase().contains("cuda")
}

fn recipe_has_mpi_numprocs(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim_start().strip_prefix('#').is_none()
            && line
                .split('#')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with("mpi_numprocs")
    })
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

/// Candidates indexed under `name` or its aliased module name.
///
/// [`provide_from_parent`] writes `Candidate.name` as the aliased module
/// (`poetry`). Specs still use the foreign spelling (`poetry-core`). Returns
/// the SAT index key that actually holds the rows.
pub fn lookup_named_candidates<'a>(
    by_name: &HashMap<&str, Vec<&'a Candidate>>,
    name: &str,
) -> Option<(String, Vec<&'a Candidate>)> {
    if let Some(named) = by_name.get(name) {
        return Some((name.to_string(), named.clone()));
    }
    if let Some((key, named)) = by_name
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        return Some(((*key).to_string(), named.clone()));
    }
    let aliased = aliased_module_name(name);
    if !aliased.eq_ignore_ascii_case(name) {
        if let Some(named) = by_name.get(aliased.as_str()) {
            return Some((aliased, named.clone()));
        }
        if let Some((key, named)) = by_name
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(&aliased))
        {
            return Some(((*key).to_string(), named.clone()));
        }
    }
    None
}

/// True when `candidate` satisfies an exact-name request for `name`.
///
/// Matches the stored module name or a foreign spelling that aliases to it.
pub fn candidate_answers_name(candidate: &Candidate, name: &str) -> bool {
    candidate.name == name
        || overlay_package_identity(&candidate.name) == overlay_package_identity(name)
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
        .any(|refused| overlay_package_identity(refused) == identity)
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
    let mut first_class = None;
    for candidate in candidates {
        if candidate.is_extension_provide() {
            continue;
        }
        if candidate
            .exts_list
            .iter()
            .any(|ext| overlay_package_identity(&ext.name) == identity && !ext.version.is_empty())
        {
            return Some(candidate);
        }
        if first_class.is_none() && overlay_package_identity(&candidate.name) == identity {
            first_class = Some(candidate);
        }
    }
    first_class
}

/// Overlay-identity map for [`existing_language_provider_in`].
///
/// Bundle `exts_list` parents are keyed first so they win over a standalone
/// first-class recipe, matching [`existing_language_provider`]. First-class
/// names only fill identities no bundle already provides.
pub fn language_provider_index(candidates: &[Candidate]) -> HashMap<String, &Candidate> {
    let mut index = HashMap::new();
    for candidate in candidates {
        if candidate.is_extension_provide() {
            continue;
        }
        for ext in &candidate.exts_list {
            if ext.version.is_empty() {
                continue;
            }
            index
                .entry(overlay_package_identity(&ext.name))
                .or_insert(candidate);
        }
    }
    for candidate in candidates {
        if candidate.is_extension_provide() {
            continue;
        }
        index
            .entry(overlay_package_identity(&candidate.name))
            .or_insert(candidate);
    }
    index
}

/// [`existing_language_provider`] against a prebuilt index.
pub fn existing_language_provider_in<'a>(
    name: &str,
    index: &HashMap<String, &'a Candidate>,
) -> Option<&'a Candidate> {
    index.get(&overlay_package_identity(name)).copied()
}

/// Collapse a selected extension provide to its parent bundle candidate.
pub fn resolve_extension_provider<'a>(
    selected: &'a Candidate,
    selected_set: &'a [Candidate],
) -> &'a Candidate {
    let Some(parent_name) = selected.extension_parent_name() else {
        return selected;
    };
    let parent_path = selected.extension_parent_path();
    selected_set
        .iter()
        .find(|candidate| {
            !candidate.is_extension_provide()
                && (parent_path.is_some_and(|path| candidate.easyconfig_path == path)
                    || (parent_path.is_none() && candidate.name == parent_name))
        })
        .unwrap_or(selected)
}

fn provide_from_parent(parent: &Candidate, ext: &ExtEntry) -> Option<Candidate> {
    if ext.name.is_empty() || ext.version.is_empty() {
        return None;
    }
    Some(Candidate {
        name: aliased_module_name(&ext.name),
        version: ext.version.clone(),
        toolchain: parent.toolchain.clone(),
        // The language identity is unsuffixed. The parent pin still carries
        // the bundle suffix so SAT selects the CUDA bundle, not a second
        // language module.
        versionsuffix: None,
        easyconfig_path: format!("{}{EXT_PROVIDE_MARKER}{}", parent.easyconfig_path, ext.name),
        dependencies: vec![DepReq {
            name: parent.name.clone(),
            version_req: format!("=={}", parent.version),
            versionsuffix: parent.versionsuffix.clone(),
            toolchain: Some(parent.toolchain.clone()),
        }],
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
        moduleclass: parent.moduleclass.clone(),
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
            moduleclass: None,
        }
    }

    #[test]
    fn expand_creates_one_provide_per_ext() {
        let expanded = expand_extension_provides(vec![bundle()]);
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
        let once = expand_extension_provides(vec![bundle()]);
        let twice = expand_extension_provides(once.clone());
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
        let expanded = expand_extension_provides(vec![parent]);
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
        let expanded = expand_extension_provides(vec![bundle()]);
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
        assert_ne!(
            overlay_package_identity("hatch-vcs"),
            overlay_package_identity("hatchling")
        );
    }

    #[test]
    fn hatch_vcs_is_not_provided_by_hatchling() {
        let mut hatchling = bundle();
        hatchling.name = "hatchling".into();
        hatchling.version = "1.27.0".into();
        hatchling.exts_list.clear();
        hatchling.easyconfig_path = "hatchling-1.27.0.eb".into();
        assert!(existing_language_provider("hatch-vcs", &[hatchling]).is_none());
    }

    #[test]
    fn aliased_provides_keep_distinct_paths() {
        let mut parent = bundle();
        parent.name = "Python-bundle-PyPI".into();
        parent.exts_list = vec![
            ExtEntry {
                name: "poetry-core".into(),
                version: "1.9.0".into(),
            },
            ExtEntry {
                name: "poetry".into(),
                version: "1.8.3".into(),
            },
        ];
        let expanded = expand_extension_provides(vec![parent]);
        let provides: Vec<&Candidate> = expanded
            .iter()
            .filter(|candidate| candidate.is_extension_provide())
            .collect();
        assert_eq!(provides.len(), 2, "{provides:?}");
        assert_ne!(
            provides[0].easyconfig_path,
            provides[1].easyconfig_path,
            "{:?}",
            provides
                .iter()
                .map(|candidate| candidate.easyconfig_path.as_str())
                .collect::<Vec<_>>()
        );
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

    #[test]
    fn a_cuda_bundle_provide_stays_an_unsuffixed_language_identity() {
        let mut parent = bundle();
        parent.versionsuffix = Some("-CUDA-12.8.0".into());
        let expanded = expand_extension_provides(vec![parent]);
        let numpy = expanded
            .iter()
            .find(|candidate| candidate.name == "numpy")
            .expect("numpy provide");
        assert!(
            numpy.versionsuffix.is_none() || numpy.versionsuffix.as_deref() == Some(""),
            "language provide is unsuffixed: {:?}",
            numpy.versionsuffix
        );
        assert_eq!(
            numpy.dependencies[0].versionsuffix.as_deref(),
            Some("-CUDA-12.8.0")
        );
    }

    #[test]
    fn language_provider_index_matches_first_wins() {
        let first = bundle();
        let mut second = bundle();
        second.easyconfig_path = "other-bundle.eb".into();
        second.name = "other-bundle".into();
        let universe = [first.clone(), second];
        let linear = existing_language_provider("numpy", &universe).expect("linear");
        let indexed = existing_language_provider_in("numpy", &language_provider_index(&universe))
            .expect("index");
        assert_eq!(linear.easyconfig_path, first.easyconfig_path);
        assert_eq!(indexed.easyconfig_path, first.easyconfig_path);
    }

    #[test]
    fn language_provider_index_prefers_bundle_over_first_class() {
        let mut numpy = bundle();
        numpy.name = "numpy".into();
        numpy.version = "2.3.1".into();
        numpy.exts_list.clear();
        numpy.easyconfig_path = "numpy-2.3.1-foss-2026.1.eb".into();
        let universe = [numpy, bundle()];
        let linear = existing_language_provider("numpy", &universe).expect("linear");
        let indexed = existing_language_provider_in("numpy", &language_provider_index(&universe))
            .expect("index");
        assert_eq!(linear.easyconfig_path, indexed.easyconfig_path);
        assert_eq!(linear.name, "SciPy-bundle");
        assert_eq!(indexed.name, "SciPy-bundle");
    }

    #[test]
    fn lookup_named_candidates_finds_aliased_provide() {
        let mut parent = bundle();
        parent.name = "Python-bundle-PyPI".into();
        parent.exts_list = vec![ExtEntry {
            name: "poetry-core".into(),
            version: "1.9.0".into(),
        }];
        let expanded = expand_extension_provides(vec![parent]);
        let mut by_name: HashMap<&str, Vec<&Candidate>> = HashMap::new();
        for candidate in &expanded {
            by_name
                .entry(candidate.name.as_str())
                .or_default()
                .push(candidate);
        }
        let (sat_name, named) =
            lookup_named_candidates(&by_name, "poetry-core").expect("aliased provide");
        assert_eq!(sat_name, "poetry");
        assert!(named.iter().any(|candidate| candidate.version == "1.9.0"));
        let poetry = named[0];
        assert!(candidate_answers_name(poetry, "poetry-core"));
        assert!(candidate_answers_name(poetry, "poetry"));
    }

    #[test]
    fn lookup_named_candidates_finds_robot_cased_name() {
        let hdf5 = Candidate {
            name: "HDF5".into(),
            version: "1.16.0".into(),
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2026.1".into(),
            },
            versionsuffix: None,
            easyconfig_path: "HDF5-1.16.0.eb".into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let mut by_name: HashMap<&str, Vec<&Candidate>> = HashMap::new();
        by_name.entry(hdf5.name.as_str()).or_default().push(&hdf5);
        let (sat_name, named) = lookup_named_candidates(&by_name, "hdf5").expect("robot case");
        assert_eq!(sat_name, "HDF5");
        assert_eq!(named[0].version, "1.16.0");
    }
}
