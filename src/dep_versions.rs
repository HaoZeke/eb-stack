//! Versions of a dependency that exist in a local easyconfigs tree.
//!
//! Review fixes often need a dependency version for a given toolchain
//! generation (`('Cython', '<which?>')` on GCCcore-13.3.0). Guessing gives
//! versions that do not exist. This scans `.eb` files named `<name>-*.eb`
//! under the given roots and reports each one's `version`, toolchain, and
//! `versionsuffix`, so the caller can pick a version that exists.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

/// Upper bound on directory entries visited, so a huge root cannot stall.
const MAX_ENTRIES: usize = 400_000;

/// One easyconfig of the requested software.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DepVersion {
    /// `version` from the easyconfig.
    pub version: String,
    /// `NAME-VERSION`, or `system` for `toolchain = SYSTEM`.
    pub toolchain: String,
    /// `versionsuffix` as written (may contain templates), empty when unset.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub versionsuffix: String,
    /// Path of the easyconfig.
    pub path: String,
}

/// Result of [`dep_versions`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DepVersions {
    /// Software name searched for (case-sensitive, as in `('Cython', ...)`).
    pub name: String,
    /// Toolchain filter, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    /// Easyconfigs for exactly that toolchain (or all, without a filter).
    pub matching: Vec<DepVersion>,
    /// Easyconfigs on a subtoolchain of the requested toolchain (for
    /// `foss-2024a`: `GCCcore-13.3.0`, `GCC-13.3.0`, `gompi-2024a`, ...). A
    /// dependency of a foss recipe may use any of these; robot finds it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtoolchain_matching: Vec<DepVersion>,
    /// Easyconfigs of the same software for other toolchains.
    pub other_toolchains: Vec<DepVersion>,
    /// Number of `.eb` files seen under the roots.
    pub easyconfigs_scanned: usize,
    /// Roots that were searched.
    pub roots: Vec<String>,
    /// One-line guidance for the caller.
    pub message: String,
}

static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?m)^version\s*=\s*['"]([^'"]*)['"]"#).expect("version regex"));
static SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^versionsuffix\s*=\s*['"]([^'"]*)['"]"#).expect("versionsuffix regex")
});
static TOOLCHAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^toolchain\s*=\s*\{\s*['"]name['"]\s*:\s*['"]([^'"]+)['"]\s*,\s*['"]version['"]\s*:\s*['"]([^'"]+)['"]"#,
    )
    .expect("toolchain regex")
});
static SYSTEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^toolchain\s*=\s*SYSTEM\b").expect("system regex"));

/// Normalize a toolchain spec: `GCCcore-13.3.0`, `GCCcore/13.3.0`,
/// `('GCCcore', '13.3.0')`-style `GCCcore 13.3.0`, or `system`.
pub fn normalize_toolchain(spec: &str) -> String {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("system") {
        return "system".into();
    }
    let cleaned: String = spec
        .chars()
        .filter(|c| !matches!(c, '(' | ')' | '\'' | '"'))
        .collect();
    let parts: Vec<&str> = cleaned
        .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    match parts.as_slice() {
        [name, version] => format!("{name}-{version}"),
        _ => cleaned,
    }
}

/// `name-version` (or `system`) of every subtoolchain of `toolchain`, the
/// toolchain itself included, when eb-stack knows its hierarchy.
fn hierarchy_members(toolchain: &str) -> Vec<String> {
    let Some((name, version)) = toolchain.rsplit_once('-') else {
        return Vec::new();
    };
    let parent = crate::domain::Toolchain {
        name: name.to_string(),
        version: version.to_string(),
    };
    crate::hierarchy::known_hierarchy(&parent)
        .map(|hierarchy| {
            hierarchy
                .members
                .iter()
                .map(|member| {
                    if member.name == "system" {
                        "system".to_string()
                    } else {
                        format!("{}-{}", member.name, member.version)
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_easyconfig(path: &Path, text: &str) -> Option<DepVersion> {
    let version = VERSION.captures(text)?.get(1)?.as_str().to_string();
    let toolchain = if let Some(captures) = TOOLCHAIN.captures(text) {
        format!("{}-{}", &captures[1], &captures[2])
    } else if SYSTEM.is_match(text) {
        "system".to_string()
    } else {
        return None;
    };
    let versionsuffix = SUFFIX
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map(|suffix| suffix.as_str().to_string())
        .unwrap_or_default();
    Some(DepVersion {
        version,
        toolchain,
        versionsuffix,
        path: path.display().to_string(),
    })
}

fn walk(
    root: &Path,
    prefix: &str,
    found: &mut Vec<PathBuf>,
    scanned: &mut usize,
    visited: &mut usize,
) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            *visited += 1;
            if *visited > MAX_ENTRIES {
                return;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if name != ".git" {
                    stack.push(entry.path());
                }
            } else if kind.is_file() && name.ends_with(".eb") {
                *scanned += 1;
                if name.starts_with(prefix) {
                    found.push(entry.path());
                }
            }
        }
    }
}

/// List the easyconfigs of `name` under `roots` (directories, or `.eb`
/// files whose directory is searched). Errors when the roots hold no `.eb`
/// file at all, because then nothing can be looked up.
pub fn dep_versions(
    roots: &[PathBuf],
    name: &str,
    toolchain: Option<&str>,
) -> Result<DepVersions, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name is empty".into());
    }
    let prefix = format!("{name}-");
    let mut files = Vec::new();
    let mut scanned = 0usize;
    let mut visited = 0usize;
    let mut searched = Vec::new();
    for root in roots {
        let dir = if root.is_file() {
            root.parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            root.clone()
        };
        if !dir.is_dir() {
            continue;
        }
        searched.push(dir.display().to_string());
        walk(&dir, &prefix, &mut files, &mut scanned, &mut visited);
    }
    if scanned == 0 {
        return Err(format!(
            "no easyconfigs tree to look {name} up in: found no .eb files under {}. Pass easyconfigs with a robot tree (for example a checkout's easybuild/easyconfigs). Do not guess a version; leave the dependency as it was and say the version could not be checked",
            if searched.is_empty() {
                "the given paths".to_string()
            } else {
                searched.join(", ")
            }
        ));
    }
    let wanted = toolchain.map(normalize_toolchain);
    let members = wanted.as_deref().map(hierarchy_members).unwrap_or_default();
    let mut matching = BTreeSet::new();
    let mut sub = BTreeSet::new();
    let mut others = BTreeSet::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("name") && line.contains(&format!("'{name}'"))
                || line.starts_with("name") && line.contains(&format!("\"{name}\""))
        }) {
            // `Foo-bar-1.0.eb` matched the `Foo-` prefix but is another package.
            continue;
        }
        let Some(found) = parse_easyconfig(&path, &text) else {
            continue;
        };
        match &wanted {
            Some(wanted) if found.toolchain != *wanted && members.contains(&found.toolchain) => {
                sub.insert(found);
            }
            Some(wanted) if found.toolchain != *wanted => {
                others.insert(found);
            }
            _ => {
                matching.insert(found);
            }
        }
    }
    let matching: Vec<DepVersion> = matching.into_iter().collect();
    let subtoolchain_matching: Vec<DepVersion> = sub.into_iter().collect();
    let other_toolchains: Vec<DepVersion> = others.into_iter().collect();
    let message = match (&wanted, matching.is_empty(), other_toolchains.is_empty()) {
        (Some(toolchain), true, _) if !subtoolchain_matching.is_empty() => format!(
            "no {name} easyconfig for {toolchain} itself, but {} exist on its subtoolchains (subtoolchain_matching: {}). A dependency of a {toolchain} recipe may use one of these versions: write the plain ('{name}', '<version>') and robot finds it through the subtoolchain",
            subtoolchain_matching.len(),
            subtoolchain_matching
                .iter()
                .map(|found| format!("{} on {}", found.version, found.toolchain))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        (_, false, _) => format!(
            "{} easyconfig(s) of {name} exist{}; use one of these versions",
            matching.len(),
            wanted
                .as_ref()
                .map(|toolchain| format!(" for {toolchain}"))
                .unwrap_or_default()
        ),
        (Some(toolchain), true, false) => format!(
            "no {name} easyconfig for {toolchain}; versions exist only for other toolchains (listed). A dependency needs an easyconfig for the same toolchain generation; do not guess a version"
        ),
        _ => format!(
            "no easyconfig of {name} in this tree; do not guess a version. Leave the dependency as it was and say it could not be checked, or add the missing easyconfig"
        ),
    };
    Ok(DepVersions {
        name: name.to_string(),
        toolchain: wanted,
        matching,
        subtoolchain_matching,
        other_toolchains,
        easyconfigs_scanned: scanned,
        roots: searched,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn tree() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let ec = "easybuild/easyconfigs/c/Cython";
        write(
            temp.path(),
            &format!("{ec}/Cython-0.29.37-GCCcore-13.3.0.eb"),
            "easyblock = 'PythonPackage'\nname = 'Cython'\nversion = '0.29.37'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n",
        );
        write(
            temp.path(),
            &format!("{ec}/Cython-3.0.10-GCCcore-13.2.0.eb"),
            "name = 'Cython'\nversion = '3.0.10'\ntoolchain = {'name': 'GCCcore', 'version': '13.2.0'}\n",
        );
        write(
            temp.path(),
            "easybuild/easyconfigs/c/Cython-bundle/Cython-bundle-1.0-GCCcore-13.3.0.eb",
            "name = 'Cython-bundle'\nversion = '1.0'\ntoolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n",
        );
        write(
            temp.path(),
            "easybuild/easyconfigs/m/make/make-4.4.1.eb",
            "name = 'make'\nversion = '4.4.1'\ntoolchain = SYSTEM\n",
        );
        temp
    }

    #[test]
    fn lists_versions_for_the_toolchain_and_others_separately() {
        let temp = tree();
        let found = dep_versions(
            &[temp.path().to_path_buf()],
            "Cython",
            Some("GCCcore-13.3.0"),
        )
        .unwrap();
        let versions: Vec<_> = found.matching.iter().map(|v| v.version.as_str()).collect();
        assert_eq!(versions, ["0.29.37"]);
        assert_eq!(found.other_toolchains.len(), 1);
        assert_eq!(found.other_toolchains[0].toolchain, "GCCcore-13.2.0");
        assert_eq!(found.easyconfigs_scanned, 4);
        assert!(
            found.message.contains("use one of these versions"),
            "{}",
            found.message
        );
    }

    #[test]
    fn subtoolchain_versions_count_for_a_composite_toolchain() {
        // Proof v5, PR 24538: Cython for a foss-2024a recipe lives on
        // GCCcore-13.3.0; the old answer was "no Cython easyconfig for
        // foss-2024a ... do not guess a version".
        let temp = tree();
        let found =
            dep_versions(&[temp.path().to_path_buf()], "Cython", Some("foss/2024a")).unwrap();
        assert!(found.matching.is_empty());
        let versions: Vec<_> = found
            .subtoolchain_matching
            .iter()
            .map(|v| (v.version.as_str(), v.toolchain.as_str()))
            .collect();
        assert_eq!(versions, [("0.29.37", "GCCcore-13.3.0")]);
        assert_eq!(found.other_toolchains.len(), 1);
        assert!(found.message.contains("subtoolchain"), "{}", found.message);
        assert!(!found.message.contains("do not guess"), "{}", found.message);
    }

    #[test]
    fn toolchain_spellings_normalize() {
        assert_eq!(normalize_toolchain("GCCcore/13.3.0"), "GCCcore-13.3.0");
        assert_eq!(
            normalize_toolchain("('GCCcore', '13.3.0')"),
            "GCCcore-13.3.0"
        );
        assert_eq!(normalize_toolchain("SYSTEM"), "system");
        let temp = tree();
        let make = dep_versions(&[temp.path().to_path_buf()], "make", Some("system")).unwrap();
        assert_eq!(make.matching.len(), 1);
    }

    #[test]
    fn says_so_when_the_toolchain_has_no_version() {
        let temp = tree();
        let found = dep_versions(
            &[temp.path().to_path_buf()],
            "Cython",
            Some("GCCcore-14.2.0"),
        )
        .unwrap();
        assert!(found.matching.is_empty());
        assert_eq!(found.other_toolchains.len(), 2);
        assert!(found.message.contains("do not guess"), "{}", found.message);
        let missing = dep_versions(&[temp.path().to_path_buf()], "Nope", None).unwrap();
        assert!(missing.matching.is_empty() && missing.other_toolchains.is_empty());
        assert!(missing.message.contains("do not guess"));
    }

    #[test]
    fn errors_clearly_without_an_easyconfigs_tree() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("notes.txt"), "x").unwrap();
        let error = dep_versions(&[temp.path().to_path_buf()], "Cython", None).unwrap_err();
        assert!(error.contains("no .eb files"), "{error}");
        assert!(error.contains("Do not guess"), "{error}");
    }
}
