//! Operator-facing reports from a solved stack lock: ordered build list and
//! baseline-vs-solved markdown stack diff.

use crate::domain::{LockPackage, StackLock};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Return co-selected easyconfig paths in dependency order (deps before apps).
///
/// Edges come from `dep_map` (package name → co-stack dependency names). Only
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
    let by_name: BTreeMap<&str, &LockPackage> = lock
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    let selected: BTreeSet<&str> = by_name.keys().copied().collect();

    // Kahn: edge dep -> pkg means dep must be installed before pkg.
    // in_degree[pkg] = number of co-selected deps still outstanding.
    let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for name in &selected {
        in_degree.insert(*name, 0);
        dependents.entry(*name).or_default();
    }
    for name in &selected {
        let deps = dep_map
            .get(*name)
            .into_iter()
            .flatten()
            .map(|d| d.as_str())
            .filter(|d| selected.contains(d) && *d != *name);
        let mut co_deps: BTreeSet<&str> = BTreeSet::new();
        for d in deps {
            co_deps.insert(d);
        }
        in_degree.insert(*name, co_deps.len());
        for d in co_deps {
            dependents.entry(d).or_default().push(*name);
        }
    }

    // Stable ready queue: BTreeSet by name.
    let mut ready: BTreeSet<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&n, _)| n)
        .collect();

    let mut order: Vec<&str> = Vec::with_capacity(selected.len());
    while let Some(n) = ready.iter().next().copied() {
        ready.remove(n);
        order.push(n);
        if let Some(children) = dependents.get(n) {
            // Process children in name order for determinism when multiple become ready.
            let mut kids: Vec<&str> = children.clone();
            kids.sort_unstable();
            for child in kids {
                if let Some(deg) = in_degree.get_mut(child) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.insert(child);
                    }
                }
            }
        }
    }

    // Cycles or missing nodes: append remaining names in sorted order so we
    // still emit every co-selected package once.
    if order.len() < selected.len() {
        for n in &selected {
            if !order.contains(n) {
                order.push(*n);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|n| by_name.get(n).copied())
        .collect()
}

/// Classification of one logical package between baseline and solved locks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageChangeKind {
    Unchanged,
    Added,
    Removed,
    VersionBumped,
}

impl PackageChangeKind {
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
    pub name: String,
    pub kind: PackageChangeKind,
    pub baseline_version: Option<String>,
    pub solved_version: Option<String>,
    pub baseline_easyconfig_path: Option<String>,
    pub solved_easyconfig_path: Option<String>,
}

/// Classify every logical package (by name) between baseline and solved locks.
///
/// Result is sorted by package name for stable markdown.
pub fn classify_stack_diff(baseline: &StackLock, solved: &StackLock) -> Vec<PackageChange> {
    let base_by: BTreeMap<&str, &LockPackage> = baseline
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    let sol_by: BTreeMap<&str, &LockPackage> = solved
        .packages
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    let mut names: BTreeSet<&str> = BTreeSet::new();
    names.extend(base_by.keys().copied());
    names.extend(sol_by.keys().copied());

    names
        .into_iter()
        .map(|name| {
            let b = base_by.get(name).copied();
            let s = sol_by.get(name).copied();
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
        })
        .collect()
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
