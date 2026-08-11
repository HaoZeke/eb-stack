//! Virtual candidates generated from EasyBuild `exts_list` entries.
//!
//! A bundle such as `SciPy-bundle` lists `numpy` in `exts_list`. The solver
//! must treat that as a provide: a requirement for `numpy==2.3.1` is satisfied
//! by selecting the parent bundle, not by inventing a standalone `numpy`
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
}
