//! Operator-facing reports from a solved stack lock: ordered build list and
//! baseline-vs-solved markdown stack diff.

use crate::domain::{LockPackage, StackLock};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Return co-selected easyconfig paths in dependency order (deps before apps).
///
/// Edges come from `dep_map` (package name or identity → co-stack dependency
/// names or identity keys). Only
/// dependencies that are also co-selected participate. Tie-break is stable by
/// package name so the order is deterministic.
pub fn ordered_build_paths(
    lock: &StackLock,
    dep_map: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    ordered_packages(lock, dep_map)
        .into_iter()
        .map(|p| p.easyconfig_path.clone())
        .collect()
}

/// Format a plain-text build list: one easyconfig path per line, deps first.
pub fn format_build_list(lock: &StackLock, dep_map: &HashMap<String, Vec<String>>) -> String {
    let paths = ordered_build_paths(lock, dep_map);
    if paths.is_empty() {
        return String::new();
    }
    let mut s = paths.join("\n");
    s.push('\n');
    s
}

/// Packages in install order (same topology as [`ordered_build_paths`]).
pub fn ordered_packages<'a>(
    lock: &'a StackLock,
    dep_map: &HashMap<String, Vec<String>>,
) -> Vec<&'a LockPackage> {
    let mut by_name: BTreeMap<&str, Vec<&LockPackage>> = BTreeMap::new();
    for package in &lock.packages {
        by_name
            .entry(package.name.as_str())
            .or_default()
            .push(package);
    }
    let by_key: BTreeMap<String, &LockPackage> = lock
        .packages
        .iter()
        .map(|package| (package_row_key(package), package))
        .collect();
    let by_identity: HashMap<String, &LockPackage> = lock
        .packages
        .iter()
        .map(|package| (crate::sbom::lock_package_key(package), package))
        .collect();

    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for package in &lock.packages {
        let key = package_row_key(package);
        dependents.entry(key.clone()).or_default();
        let mut deg = 0usize;
        if let Some(deps) = dep_map
            .get(&package.name)
            .or_else(|| dep_map.get(&crate::sbom::lock_package_key(package)))
        {
            for dep_name in deps {
                let dep_pkgs: Vec<&LockPackage> = if let Some(dep_pkg) = by_identity.get(dep_name) {
                    vec![*dep_pkg]
                } else if let Some(dep_pkgs) = by_name.get(dep_name.as_str()) {
                    dep_pkgs.clone()
                } else {
                    continue;
                };
                for dep_pkg in dep_pkgs {
                    if package_row_key(dep_pkg) != key {
                        dependents
                            .entry(package_row_key(dep_pkg))
                            .or_default()
                            .push(key.clone());
                        deg += 1;
                    }
                }
            }
        }
        in_degree.insert(key, deg);
    }

    let mut ready: BTreeSet<String> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(key, _)| key.clone())
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(by_key.len());
    while let Some(key) = ready.iter().next().cloned() {
        ready.remove(&key);
        order.push(key.clone());
        if let Some(children) = dependents.get(&key) {
            let mut kids = children.clone();
            kids.sort();
            for child in kids {
                if let Some(deg) = in_degree.get_mut(&child) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.insert(child);
                    }
                }
            }
        }
    }
    if order.len() < by_key.len() {
        for key in by_key.keys() {
            if !order.contains(key) {
                order.push(key.clone());
            }
        }
    }

    order
        .into_iter()
        .filter_map(|key| by_key.get(&key).copied())
        .collect()
}

fn package_row_key(package: &LockPackage) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        package.name,
        package.version,
        package.toolchain.name,
        package.toolchain.version,
        package.versionsuffix.as_deref().unwrap_or("")
    )
}

/// Classification of one logical package between baseline and solved locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageChangeKind {
    /// Present in both locks at the same version.
    Unchanged,
    /// In the solved lock only: the solve pulled it in.
    Added,
    /// In the baseline only: nothing in the solved stack needs it.
    Removed,
    /// In both, at different versions. The direction is not implied; read
    /// `baseline_version` against `solved_version` to see which way it moved.
    VersionBumped,
}

impl PackageChangeKind {
    /// The stable kebab-case name used in reports and machine-read output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Added => "added",
            Self::Removed => "removed",
            Self::VersionBumped => "version-bumped",
        }
    }
}

/// One logical package's baseline-vs-solved change for human review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageChange {
    /// Logical package name, the key the two locks are matched on.
    pub name: String,
    /// Which side of the comparison this package fell on.
    pub kind: PackageChangeKind,
    /// Version in the baseline lock. `None` when the package is `Added`.
    pub baseline_version: Option<String>,
    /// Version in the solved lock. `None` when the package is `Removed`.
    pub solved_version: Option<String>,
    /// Easyconfig backing the baseline version, when that lock recorded one.
    /// Absent for an `Added` package and for a lock written without paths.
    pub baseline_easyconfig_path: Option<String>,
    /// Easyconfig backing the solved version, on the same terms.
    pub solved_easyconfig_path: Option<String>,
}

/// Classify every logical package (by name) between baseline and solved locks.
///
/// Result is sorted by package name for stable markdown.
pub fn classify_stack_diff(baseline: &StackLock, solved: &StackLock) -> Vec<PackageChange> {
    let mut base_by: BTreeMap<&str, Vec<&LockPackage>> = BTreeMap::new();
    for package in &baseline.packages {
        base_by
            .entry(package.name.as_str())
            .or_default()
            .push(package);
    }
    let mut sol_by: BTreeMap<&str, Vec<&LockPackage>> = BTreeMap::new();
    for package in &solved.packages {
        sol_by
            .entry(package.name.as_str())
            .or_default()
            .push(package);
    }
    let mut names: BTreeSet<&str> = BTreeSet::new();
    names.extend(base_by.keys().copied());
    names.extend(sol_by.keys().copied());

    let mut changes = Vec::new();
    for name in names {
        let base = base_by.get(name).cloned().unwrap_or_default();
        let sol = sol_by.get(name).cloned().unwrap_or_default();
        if base.len() <= 1 && sol.len() <= 1 {
            changes.push(diff_pair(name, base.first().copied(), sol.first().copied()));
            continue;
        }
        let mut base_id: BTreeMap<String, &LockPackage> = base
            .iter()
            .map(|package| (package_row_key(package), *package))
            .collect();
        let mut sol_id: BTreeMap<String, &LockPackage> = sol
            .iter()
            .map(|package| (package_row_key(package), *package))
            .collect();
        let mut keys: BTreeSet<String> = BTreeSet::new();
        keys.extend(base_id.keys().cloned());
        keys.extend(sol_id.keys().cloned());
        for key in keys {
            changes.push(diff_pair(name, base_id.remove(&key), sol_id.remove(&key)));
        }
    }
    changes
}

fn diff_pair(name: &str, b: Option<&LockPackage>, s: Option<&LockPackage>) -> PackageChange {
    match (b, s) {
        (None, Some(s)) => PackageChange {
            name: name.to_string(),
            kind: PackageChangeKind::Added,
            baseline_version: None,
            solved_version: Some(s.version.clone()),
            baseline_easyconfig_path: None,
            solved_easyconfig_path: Some(s.easyconfig_path.clone()),
        },
        (Some(b), None) => PackageChange {
            name: name.to_string(),
            kind: PackageChangeKind::Removed,
            baseline_version: Some(b.version.clone()),
            solved_version: None,
            baseline_easyconfig_path: Some(b.easyconfig_path.clone()),
            solved_easyconfig_path: None,
        },
        (Some(b), Some(s)) if b.version == s.version => PackageChange {
            name: name.to_string(),
            kind: PackageChangeKind::Unchanged,
            baseline_version: Some(b.version.clone()),
            solved_version: Some(s.version.clone()),
            baseline_easyconfig_path: Some(b.easyconfig_path.clone()),
            solved_easyconfig_path: Some(s.easyconfig_path.clone()),
        },
        (Some(b), Some(s)) => PackageChange {
            name: name.to_string(),
            kind: PackageChangeKind::VersionBumped,
            baseline_version: Some(b.version.clone()),
            solved_version: Some(s.version.clone()),
            baseline_easyconfig_path: Some(b.easyconfig_path.clone()),
            solved_easyconfig_path: Some(s.easyconfig_path.clone()),
        },
        (None, None) => unreachable!("name always from one side"),
    }
}

/// Human-reviewable markdown comparing baseline lock to solved lock.
///
/// Pasteable into a pull request: per-package status, versions, and easyconfig
/// paths on each side that exists.
pub fn format_stack_diff_markdown(baseline: &StackLock, solved: &StackLock) -> String {
    let changes = classify_stack_diff(baseline, solved);
    let mut unchanged = 0usize;
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut bumped = 0usize;
    for c in &changes {
        match c.kind {
            PackageChangeKind::Unchanged => unchanged += 1,
            PackageChangeKind::Added => added += 1,
            PackageChangeKind::Removed => removed += 1,
            PackageChangeKind::VersionBumped => bumped += 1,
        }
    }

    let base_label = baseline
        .generation_label
        .clone()
        .unwrap_or_else(|| baseline.toolchain.label());
    let sol_label = solved
        .generation_label
        .clone()
        .unwrap_or_else(|| solved.toolchain.label());

    let mut out = String::new();
    out.push_str("# Stack diff\n\n");
    out.push_str(&format!(
        "Baseline (`{base_label}`) → solved (`{sol_label}`).\n\n"
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- **unchanged**: {unchanged}\n- **added**: {added}\n- **removed**: {removed}\n- **version-bumped**: {bumped}\n\n"
    ));
    out.push_str("## Packages\n\n");

    for c in &changes {
        out.push_str(&format!("### {} — {}\n\n", c.name, c.kind.as_str()));
        match c.kind {
            PackageChangeKind::Unchanged => {
                out.push_str(&format!(
                    "- Baseline: `{}` — `{}`\n- Solved: `{}` — `{}`\n\n",
                    c.baseline_version.as_deref().unwrap_or("—"),
                    c.baseline_easyconfig_path.as_deref().unwrap_or("—"),
                    c.solved_version.as_deref().unwrap_or("—"),
                    c.solved_easyconfig_path.as_deref().unwrap_or("—"),
                ));
            }
            PackageChangeKind::Added => {
                out.push_str(&format!(
                    "- Baseline: *(not present)*\n- Solved: `{}` — `{}`\n\n",
                    c.solved_version.as_deref().unwrap_or("—"),
                    c.solved_easyconfig_path.as_deref().unwrap_or("—"),
                ));
            }
            PackageChangeKind::Removed => {
                out.push_str(&format!(
                    "- Baseline: `{}` — `{}`\n- Solved: *(removed)*\n\n",
                    c.baseline_version.as_deref().unwrap_or("—"),
                    c.baseline_easyconfig_path.as_deref().unwrap_or("—"),
                ));
            }
            PackageChangeKind::VersionBumped => {
                out.push_str(&format!(
                    "- Baseline: `{}` — `{}`\n- Solved: `{}` — `{}`\n- Change: `{}` → `{}`\n\n",
                    c.baseline_version.as_deref().unwrap_or("—"),
                    c.baseline_easyconfig_path.as_deref().unwrap_or("—"),
                    c.solved_version.as_deref().unwrap_or("—"),
                    c.solved_easyconfig_path.as_deref().unwrap_or("—"),
                    c.baseline_version.as_deref().unwrap_or("—"),
                    c.solved_version.as_deref().unwrap_or("—"),
                ));
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;
    use crate::sbom::dep_map_from_universe;
    use crate::select::select_stack;
    use std::path::PathBuf;

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next")
    }

    fn load_json<T: serde::de::DeserializeOwned>(name: &str) -> T {
        let p = fixture_dir().join(name);
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    fn tc(name: &str, ver: &str) -> Toolchain {
        Toolchain {
            name: name.into(),
            version: ver.into(),
        }
    }

    fn pkg(name: &str, ver: &str, path: &str) -> LockPackage {
        LockPackage {
            name: name.into(),
            version: ver.into(),
            toolchain: tc("foss", "2025b"),
            versionsuffix: None,
            easyconfig_path: path.into(),
        }
    }

    fn lock_of(packages: Vec<LockPackage>) -> StackLock {
        StackLock {
            schema_version: 1,
            toolchain: tc("foss", "2025b"),
            generation_label: Some("test".into()),
            packages,
            solver: SolverMeta {
                engine: "test".into(),
                engine_version: "test".into(),
                timestamp: "STABLE".into(),
            },
        }
    }

    #[test]
    fn build_list_is_newline_paths_deps_before_app() {
        let baseline: StackLock = load_json("baseline.lock.json");
        let universe: Universe = load_json("universe_next.json");
        let policy: Policy = load_json("policy_prefer_newer.json");
        let lock = select_stack(&universe, &policy, Some(&baseline)).unwrap();
        let dep_map = dep_map_from_universe(&lock, &universe);
        let text = format_build_list(&lock, &dep_map);

        // Plain paths only: no blank lines, one path per line, trailing newline.
        assert!(text.ends_with('\n'), "build list should end with newline");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), lock.packages.len());
        for line in &lines {
            assert!(!line.is_empty());
            assert!(
                line.ends_with(".eb"),
                "expected easyconfig path, got {line:?}"
            );
            assert!(
                !line.contains(' '),
                "build list lines should be paths only: {line:?}"
            );
        }

        // Every selected package appears exactly once.
        let mut seen = BTreeSet::new();
        for p in &lock.packages {
            assert!(
                lines.iter().any(|l| *l == p.easyconfig_path),
                "missing {}",
                p.easyconfig_path
            );
            seen.insert(p.name.as_str());
        }
        assert_eq!(seen.len(), lock.packages.len());

        // GROMACS depends on Python, OpenBLAS, OpenMPI, FFTW — application last.
        let idx = |name: &str| {
            let path = &lock.package(name).unwrap().easyconfig_path;
            lines.iter().position(|l| *l == path).unwrap()
        };
        let g = idx("GROMACS");
        assert!(idx("OpenBLAS") < g, "OpenBLAS before GROMACS");
        assert!(idx("OpenMPI") < g, "OpenMPI before GROMACS");
        assert!(idx("FFTW") < g, "FFTW before GROMACS");
        assert!(idx("Python") < g, "Python before GROMACS");
    }

    #[test]
    fn stack_diff_classifies_fixture_prefer_newer() {
        let baseline: StackLock = load_json("baseline.lock.json");
        let universe: Universe = load_json("universe_next.json");
        let policy: Policy = load_json("policy_prefer_newer.json");
        let lock = select_stack(&universe, &policy, Some(&baseline)).unwrap();
        let changes = classify_stack_diff(&baseline, &lock);
        let by: BTreeMap<_, _> = changes.iter().map(|c| (c.name.as_str(), c)).collect();

        assert_eq!(by["FFTW"].kind, PackageChangeKind::Unchanged);
        assert_eq!(by["FFTW"].baseline_version.as_deref(), Some("3.3.10"));
        assert_eq!(by["FFTW"].solved_version.as_deref(), Some("3.3.10"));
        assert!(by["FFTW"].baseline_easyconfig_path.is_some());
        assert!(by["FFTW"].solved_easyconfig_path.is_some());

        assert_eq!(by["GROMACS"].kind, PackageChangeKind::VersionBumped);
        assert_eq!(by["GROMACS"].baseline_version.as_deref(), Some("2024.1"));
        assert_eq!(by["GROMACS"].solved_version.as_deref(), Some("2025.0"));

        assert_eq!(by["OpenBLAS"].kind, PackageChangeKind::VersionBumped);
        assert_eq!(by["OpenMPI"].kind, PackageChangeKind::VersionBumped);

        let md = format_stack_diff_markdown(&baseline, &lock);
        assert!(md.contains("version-bumped"), "{md}");
        assert!(md.contains("unchanged"), "{md}");
        assert!(md.contains("GROMACS"), "{md}");
        assert!(md.contains("2024.1") && md.contains("2025.0"), "{md}");
        assert!(
            md.contains(by["GROMACS"].baseline_easyconfig_path.as_deref().unwrap()),
            "{md}"
        );
        assert!(
            md.contains(by["GROMACS"].solved_easyconfig_path.as_deref().unwrap()),
            "{md}"
        );
    }

    #[test]
    fn stack_diff_added_and_removed() {
        let baseline = lock_of(vec![
            pkg("OpenBLAS", "0.3.23", "old/OpenBLAS.eb"),
            pkg("Legacy", "1.0", "old/Legacy.eb"),
        ]);
        let solved = lock_of(vec![
            pkg("OpenBLAS", "0.3.27", "new/OpenBLAS.eb"),
            pkg("NewPkg", "2.0", "new/NewPkg.eb"),
        ]);
        let changes = classify_stack_diff(&baseline, &solved);
        let by: BTreeMap<_, _> = changes.iter().map(|c| (c.name.as_str(), c)).collect();

        assert_eq!(by["Legacy"].kind, PackageChangeKind::Removed);
        assert_eq!(
            by["Legacy"].baseline_easyconfig_path.as_deref(),
            Some("old/Legacy.eb")
        );
        assert!(by["Legacy"].solved_easyconfig_path.is_none());

        assert_eq!(by["NewPkg"].kind, PackageChangeKind::Added);
        assert_eq!(
            by["NewPkg"].solved_easyconfig_path.as_deref(),
            Some("new/NewPkg.eb")
        );
        assert!(by["NewPkg"].baseline_easyconfig_path.is_none());

        assert_eq!(by["OpenBLAS"].kind, PackageChangeKind::VersionBumped);

        let md = format_stack_diff_markdown(&baseline, &solved);
        assert!(md.contains("**added**: 1"), "{md}");
        assert!(md.contains("**removed**: 1"), "{md}");
        assert!(md.contains("**version-bumped**: 1"), "{md}");
        assert!(md.contains("*(not present)*"), "{md}");
        assert!(md.contains("*(removed)*"), "{md}");
        assert!(md.contains("`0.3.23` → `0.3.27`"), "{md}");
    }

    #[test]
    fn build_list_empty_lock() {
        let lock = lock_of(vec![]);
        let map = HashMap::new();
        assert_eq!(format_build_list(&lock, &map), "");
    }
}
