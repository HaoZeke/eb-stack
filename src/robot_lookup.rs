//! EasyBuild 5.4.2's robot lookup, for planning over the robot path a build
//! will run with.
//!
//! A dependency is found by `robot_find_easyconfig(name, version)`
//! (`easybuild/framework/easyconfig/easyconfig.py`), with `version` the full
//! version `det_full_ec_version` gives: `versionprefix`, the version, then
//! `-<toolchain name>-<toolchain version>` unless the toolchain is `system`,
//! then `versionsuffix`. For each robot path in order it tries the four
//! paths `create_paths` gives, in this order, and takes the first file that
//! exists:
//!
//! 1. `<root>/<name>/<version>.eb`
//! 2. `<root>/<name>/<name>-<version>.eb`
//! 3. `<root>/<letter_dir_for(name)>/<name>/<name>-<version>.eb`
//! 4. `<root>/<name>-<version>.eb`
//!
//! The robot then checks the file it found (`verify_easyconfig_filename`):
//! its name must be `<name>-<version>.eb` and its content must give that
//! name and full version. A file that fails is not skipped for a later
//! layout or root; EasyBuild stops with an error. So a `<root>/<name>/<version>.eb`
//! file is found and always refused, and it hides every later file of that
//! name and version.
//!
//! With `consider_archived_easyconfigs` the paths `<root>/__archive__` of
//! every root are searched after all the roots, the same way. Any other file
//! under a root, however deep and whatever its content, is never a
//! dependency's easyconfig. The path index EasyBuild keeps per root only
//! records which files exist; it adds no candidates.
//!
//! An easyconfig a build list names by file is found by `locate_files`
//! (`easybuild/tools/filetools.py`): the first robot path whose file index
//! has a file of that name, searched recursively with the directories in
//! `ignore_dirs` (and `__archive__`, unless archived easyconfigs are
//! considered) left out. The index is a Python set, so two files of that
//! name under one root leave the choice to set order; that case is refused
//! here instead of guessed.

use crate::domain::{Candidate, Toolchain};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The directory archived easyconfigs live in under a robot path.
pub const ARCHIVE_DIR: &str = "__archive__";

/// EasyBuild's default `ignore_dirs`.
pub const DEFAULT_IGNORE_DIRS: &[&str] = &[".git", ".svn"];

/// Whether the robot also searches `<root>/__archive__`
/// (`--consider-archived-easyconfigs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchivedEasyconfigs {
    /// EasyBuild's default: archived easyconfigs are not searched.
    Ignored,
    /// `<root>/__archive__` of every root is searched after all the roots.
    Considered,
}

/// EasyBuild's `letter_dir_for`: the lowercased first character, or `0`
/// when that is outside `a`-`z`.
pub fn letter_dir_for(name: &str) -> String {
    match name.to_lowercase().chars().next() {
        Some(first) if first.is_ascii_lowercase() => first.to_string(),
        _ => "0".to_string(),
    }
}

/// EasyBuild 5.4.2 `is_system_toolchain`: only the exact name `system`
/// (see `SYSTEM_TOOLCHAIN_NAME`). `dummy`, `System`, and `SYSTEM` are not.
/// Hierarchy matching may still treat `dummy` as an alias of `system` for
/// old robot trees; module naming and robot full versions do not.
pub fn is_module_system_toolchain(toolchain: &Toolchain) -> bool {
    toolchain.name == "system"
}

/// `det_full_ec_version`: `versionprefix`, `version`, the toolchain unless it
/// is `system`, then `versionsuffix`. EasyBuild 5 treats only `system` as the
/// system toolchain.
pub fn full_version(
    version: &str,
    toolchain: &Toolchain,
    versionprefix: &str,
    versionsuffix: &str,
) -> String {
    let main = if is_module_system_toolchain(toolchain) {
        version.to_string()
    } else {
        format!("{version}-{}-{}", toolchain.name, toolchain.version)
    };
    format!("{versionprefix}{main}{versionsuffix}")
}

/// The four paths `create_paths` tries under `root`, in EasyBuild's order.
pub fn candidate_paths(root: &Path, name: &str, full_version: &str) -> [PathBuf; 4] {
    let file = format!("{name}-{full_version}.eb");
    [
        root.join(name).join(format!("{full_version}.eb")),
        root.join(name).join(&file),
        root.join(letter_dir_for(name)).join(name).join(&file),
        root.join(&file),
    ]
}

/// The `versionprefix` an easyconfig sets, from its text: `""` when it sets
/// none. Only a plain string literal is understood; any other top-level
/// assignment is an error, because the full version cannot be told.
pub fn versionprefix_of(text: &str) -> Result<String, String> {
    use regex::Regex;
    use std::sync::LazyLock;
    static ASSIGNED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^versionprefix\s*=").expect("versionprefix regex"));
    static LITERAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?m)^versionprefix\s*=\s*(?:'([^'\\]*)'|"([^"\\]*)")\s*(?:#.*)?$"#)
            .expect("versionprefix literal regex")
    });
    let assignments = ASSIGNED.find_iter(text).count();
    if assignments == 0 {
        return Ok(String::new());
    }
    let literals: Vec<String> = LITERAL
        .captures_iter(text)
        .map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .collect();
    match literals.as_slice() {
        [one] if assignments == 1 => Ok(one.clone()),
        _ => Err("versionprefix is not one plain string literal".to_string()),
    }
}

/// The full version of a parsed candidate, as EasyBuild's robot looks it up.
/// Uses [`Candidate::versionprefix`] (parsed once at the easyconfig boundary);
/// does not re-read the file. `_text` is retained so existing call sites keep
/// compiling; it is not consulted. A part that still holds a `%(...)s`
/// template is an error: the file name cannot be told.
pub fn candidate_full_version(candidate: &Candidate, _text: &str) -> Result<String, String> {
    let prefix = candidate.versionprefix.as_deref().unwrap_or("");
    let suffix = candidate.versionsuffix.clone().unwrap_or_default();
    let parts = [
        candidate.name.as_str(),
        candidate.version.as_str(),
        candidate.toolchain.name.as_str(),
        candidate.toolchain.version.as_str(),
        prefix,
        suffix.as_str(),
    ];
    if let Some(part) = parts.iter().find(|part| part.contains("%(")) {
        return Err(format!(
            "{part:?} keeps a template eb-stack does not resolve"
        ));
    }
    Ok(full_version(
        &candidate.version,
        &candidate.toolchain,
        prefix,
        &suffix,
    ))
}

/// A robot path: its roots in EasyBuild's order, first searched first, and
/// whether archived easyconfigs are searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RobotPath {
    roots: Vec<PathBuf>,
    archived: ArchivedEasyconfigs,
}

/// What EasyBuild's robot does for one dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The robot uses this file.
    Found(PathBuf),
    /// No candidate path exists: the dependency is missing.
    Missing,
    /// The robot finds this file and then stops with an error, because its
    /// name or content is not the name and full version it looked for.
    Refused {
        /// The file found.
        path: PathBuf,
        /// What `verify_easyconfig_filename` would say.
        reason: String,
    },
}

/// Which files of a parsed robot tree EasyBuild's robot would use.
#[derive(Debug, Clone, Default)]
pub struct EffectiveCandidates {
    /// The candidates the robot finds for their own name and full version,
    /// highest precedence first: by search root, then by `create_paths` order.
    pub kept: Vec<Candidate>,
    /// Paths of candidates the robot never reads: another file wins for
    /// their name and full version, or no lookup reaches them.
    pub unreachable: Vec<String>,
    /// Paths whose full version cannot be told, with the reason.
    pub undecidable: Vec<(String, String)>,
    /// Paths the robot finds for their own name and full version and then
    /// refuses, with the reason: their name and version are not usable.
    pub refused: Vec<(String, String)>,
}

impl RobotPath {
    /// The robot path `roots`, first searched first.
    pub fn new(roots: Vec<PathBuf>, archived: ArchivedEasyconfigs) -> Self {
        Self { roots, archived }
    }

    /// The roots, first searched first.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Every root a dependency lookup searches, in order: the roots, then
    /// `<root>/__archive__` of each when archived easyconfigs are considered.
    pub fn search_roots(&self) -> Vec<PathBuf> {
        let mut out = self.roots.clone();
        if self.archived == ArchivedEasyconfigs::Considered {
            out.extend(self.roots.iter().map(|root| root.join(ARCHIVE_DIR)));
        }
        out
    }

    /// A root that is not a directory, which EasyBuild would skip without a
    /// word; planning over it cannot match the build.
    pub fn require_directories(&self) -> Result<(), String> {
        match self.roots.iter().find(|root| !root.is_dir()) {
            Some(root) => Err(format!("robot path {} is not a directory", root.display())),
            None => Ok(()),
        }
    }

    /// `robot_find_easyconfig(name, full_version)` with `exists` telling
    /// which files exist: the first candidate path, over [`Self::search_roots`],
    /// that does. Returns the path and its index in `search_roots`.
    pub fn find_with(
        &self,
        name: &str,
        full_version: &str,
        exists: impl Fn(&Path) -> bool,
    ) -> Option<(usize, PathBuf)> {
        self.search_roots()
            .iter()
            .enumerate()
            .find_map(|(index, root)| {
                candidate_paths(root, name, full_version)
                    .into_iter()
                    .find(|path| exists(path))
                    .map(|path| (index, path))
            })
    }

    /// [`Self::find_with`] on the file system.
    pub fn find(&self, name: &str, full_version: &str) -> Option<PathBuf> {
        self.find_with(name, full_version, |path| path.is_file())
            .map(|(_, path)| path)
    }

    /// The robot's answer for a dependency: [`Self::find`], then the file
    /// check. `identity` gives the name and full version a file's content
    /// states, or why that cannot be told.
    pub fn resolve(
        &self,
        name: &str,
        full_version: &str,
        identity: impl Fn(&Path) -> Result<(String, String), String>,
    ) -> Resolution {
        let Some(path) = self.find(name, full_version) else {
            return Resolution::Missing;
        };
        match verify_file(&path, name, full_version, identity) {
            Ok(()) => Resolution::Found(path),
            Err(reason) => Resolution::Refused { path, reason },
        }
    }

    /// `locate_files` for one file name: the file of that name under the
    /// first root whose tree has one, leaving out `ignore_dirs` and, unless
    /// archived easyconfigs are considered, `__archive__`. Two files of that
    /// name under the first root that has any is an error.
    pub fn locate_named(
        &self,
        file_name: &str,
        ignore_dirs: &[&str],
    ) -> Result<Option<PathBuf>, String> {
        let mut ignored: BTreeSet<&str> = ignore_dirs.iter().copied().collect();
        if self.archived == ArchivedEasyconfigs::Ignored {
            ignored.insert(ARCHIVE_DIR);
        }
        for root in &self.roots {
            if !root.exists() {
                continue;
            }
            let found = files_named(root, file_name, &ignored);
            match found.as_slice() {
                [] => continue,
                [one] => return Ok(Some(one.clone())),
                many => {
                    return Err(format!(
                        "{} files named {file_name} under robot path {}: {}; EasyBuild's pick among them is not defined",
                        many.len(),
                        root.display(),
                        many.iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
        }
        Ok(None)
    }

    /// The candidates of a parsed tree EasyBuild's robot would use: each
    /// candidate whose own name and full version lead [`Self::find`] to its
    /// own file. Full versions come from candidate fields (no text re-read).
    /// `read` is retained for call-site compatibility and is unused.
    pub fn effective(
        &self,
        candidates: Vec<Candidate>,
        _read: impl Fn(&Path) -> Option<String>,
    ) -> EffectiveCandidates {
        let mut out = EffectiveCandidates::default();
        let mut ranked: Vec<((usize, usize), Candidate)> = Vec::new();
        let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
        for candidate in candidates {
            let path = PathBuf::from(&candidate.easyconfig_path);
            let version = match candidate_full_version(&candidate, "") {
                Ok(version) => version,
                Err(why) => {
                    out.undecidable
                        .push((candidate.easyconfig_path.clone(), why));
                    continue;
                }
            };
            let same = |found: &Path| same_file(found, &path);
            let found = self.find_with(&candidate.name, &version, |p| p.is_file());
            match found {
                Some((_, found))
                    if same(&found) && !file_name_matches(&found, &candidate.name, &version) =>
                {
                    out.refused.push((
                        candidate.easyconfig_path.clone(),
                        expected_name_reason(&found, &candidate.name, &version),
                    ));
                }
                Some((root, found)) if same(&found) => {
                    let canonical = std::fs::canonicalize(&found).unwrap_or(found.clone());
                    if !seen.insert(canonical) {
                        out.unreachable.push(candidate.easyconfig_path.clone());
                        continue;
                    }
                    let layout =
                        candidate_paths(&self.search_roots()[root], &candidate.name, &version)
                            .iter()
                            .position(|p| same_file(p, &found))
                            .unwrap_or(0);
                    ranked.push(((root, layout), candidate));
                }
                _ => out.unreachable.push(candidate.easyconfig_path.clone()),
            }
        }
        ranked.sort_by_key(|(rank, _)| *rank);
        out.kept = ranked.into_iter().map(|(_, candidate)| candidate).collect();
        out.unreachable.sort();
        out.undecidable.sort();
        out.refused.sort();
        out
    }
}

/// `verify_easyconfig_filename`: the file is named `<name>-<version>.eb`
/// and its content states that name and full version.
fn verify_file(
    path: &Path,
    name: &str,
    full_version: &str,
    identity: impl Fn(&Path) -> Result<(String, String), String>,
) -> Result<(), String> {
    if !file_name_matches(path, name, full_version) {
        return Err(expected_name_reason(path, name, full_version));
    }
    let (found_name, found_version) = identity(path)?;
    if found_name != name || found_version != full_version {
        return Err(format!(
            "{} states {found_name} {found_version}, not {name} {full_version}",
            path.display()
        ));
    }
    Ok(())
}

fn file_name_matches(path: &Path, name: &str, full_version: &str) -> bool {
    path.file_name().and_then(|f| f.to_str()) == Some(format!("{name}-{full_version}.eb").as_str())
}

fn expected_name_reason(path: &Path, name: &str, full_version: &str) -> String {
    format!(
        "EasyBuild's robot finds {} for {name} {full_version} and refuses it: the file name is not {name}-{full_version}.eb",
        path.display()
    )
}

/// Whether two paths name one file: equal, or the same file once resolved.
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Files named `file_name` under `root`, as `os.walk(followlinks=True)`
/// sees them with the directories named in `ignored` left out below `root`.
fn files_named(root: &Path, file_name: &str, ignored: &BTreeSet<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if let Ok(real) = std::fs::canonicalize(&dir) {
            if !visited.insert(real) {
                continue;
            }
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !ignored.contains(name.as_str()) {
                    stack.push(path);
                }
            } else if name == file_name {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(name: &str, version: &str) -> Toolchain {
        Toolchain {
            name: name.into(),
            version: version.into(),
        }
    }

    #[test]
    fn letter_dirs_match_easybuild() {
        assert_eq!(letter_dir_for("GCC"), "g");
        assert_eq!(letter_dir_for("zlib"), "z");
        assert_eq!(letter_dir_for("4ti2"), "0");
        assert_eq!(letter_dir_for("_x"), "0");
    }

    #[test]
    fn full_versions_follow_det_full_ec_version() {
        assert_eq!(full_version("1.3", &tc("system", "system"), "", ""), "1.3");
        assert_eq!(
            full_version("1.3", &tc("GCCcore", "15.2.0"), "", "-x"),
            "1.3-GCCcore-15.2.0-x"
        );
        assert_eq!(
            full_version("2024.1.0", &tc("system", "system"), "mpp-", ".linux"),
            "mpp-2024.1.0.linux"
        );
        // EasyBuild 5 has no `dummy` alias for the system toolchain.
        assert_eq!(
            full_version("1", &tc("dummy", "dummy"), "", ""),
            "1-dummy-dummy"
        );
    }

    #[test]
    fn create_paths_order_is_easybuilds() {
        let paths = candidate_paths(Path::new("/r"), "GCC", "13.2.0");
        assert_eq!(
            paths.map(|p| p.display().to_string()),
            [
                "/r/GCC/13.2.0.eb",
                "/r/GCC/GCC-13.2.0.eb",
                "/r/g/GCC/GCC-13.2.0.eb",
                "/r/GCC-13.2.0.eb"
            ]
        );
    }

    #[test]
    fn versionprefix_is_read_only_from_a_plain_literal() {
        assert_eq!(versionprefix_of("name = 'x'\n").unwrap(), "");
        assert_eq!(
            versionprefix_of("versionprefix = 'mpp-'\n").unwrap(),
            "mpp-"
        );
        assert_eq!(
            versionprefix_of("versionprefix = \"v\"  # tag\n").unwrap(),
            "v"
        );
        assert!(versionprefix_of("versionprefix = local_p\n").is_err());
        assert!(versionprefix_of("versionprefix = 'a'\nversionprefix = 'b'\n").is_err());
    }

    #[test]
    fn the_first_root_then_the_first_layout_wins_and_archives_come_last() {
        let present: BTreeSet<PathBuf> = [
            "/b/Dep/Dep-1.0.eb",
            "/b/d/Dep/Dep-1.0.eb",
            "/a/d/Dep/Dep-1.0.eb",
            "/a/__archive__/o/Old/Old-0.1.eb",
            "/b/Old-0.1.eb",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let exists = |p: &Path| present.contains(p);
        let robot = RobotPath::new(vec!["/a".into(), "/b".into()], ArchivedEasyconfigs::Ignored);
        assert_eq!(
            robot.find_with("Dep", "1.0", exists).map(|(_, p)| p),
            Some(PathBuf::from("/a/d/Dep/Dep-1.0.eb"))
        );
        let robot_b = RobotPath::new(vec!["/b".into(), "/a".into()], ArchivedEasyconfigs::Ignored);
        assert_eq!(
            robot_b.find_with("Dep", "1.0", exists).map(|(_, p)| p),
            Some(PathBuf::from("/b/Dep/Dep-1.0.eb"))
        );
        // A later plain root beats an earlier root's archive.
        let archived = RobotPath::new(
            vec!["/a".into(), "/b".into()],
            ArchivedEasyconfigs::Considered,
        );
        assert_eq!(
            archived.find_with("Old", "0.1", exists).map(|(_, p)| p),
            Some(PathBuf::from("/b/Old-0.1.eb"))
        );
        assert_eq!(
            archived.search_roots(),
            [
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/a/__archive__"),
                PathBuf::from("/b/__archive__")
            ]
        );
        assert_eq!(robot.find_with("Missing", "1", exists), None);
    }
}
