//! Toolchain generation hierarchy: sub-toolchain membership for dependency resolve.
//!
//! EasyBuild applications built with a composite toolchain (e.g. `foss-2024a`) pull
//! dependencies from several sub-toolchain levels of the **same generation**
//! (`GCCcore`, `GCC`, `gfbf`, `gompi`, …). Exact top-level toolchain string matching
//! therefore finds none of them.
//!
//! Hierarchy ground truth is EasyBuild's `get_toolchain_hierarchy` (framework).
//! Checked-in JSON fixtures under `fixtures/toolchain_hierarchy/` capture that
//! output so unit tests and runtime resolution do not require EasyBuild.

use crate::domain::{Candidate, Toolchain};
use crate::version::cmp_version;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Minimum share of generation-scoped dependency pins a version must hold to
/// count as a **clear** consensus (modal). Below this, the signal is treated as
/// weak and we fall back to newest-among-used (still ≥ floor).
///
/// Rationale: pure plurality can favor an older back-ported pin that many
/// packages still list (scikit-build-core 0.10.6) while maintainers of key
/// apps (GROMACS) already moved to 0.11.1. Clear majority (CMake 3.29.3 at
/// ~97%) still wins; weak plurality falls through to newest-in-generation.
const CONSENSUS_CLEAR_MAJORITY_NUM: usize = 4;
const CONSENSUS_CLEAR_MAJORITY_DEN: usize = 5; // 4/5 = 80%

/// Ordered sub-toolchain hierarchy for one parent toolchain generation.
///
/// Member order matches EasyBuild: most minimal first, parent last
/// (e.g. `system`, `GCCcore`, `GCC`, `gfbf`, `gompi`, `foss`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainHierarchy {
    pub parent: Toolchain,
    pub members: Vec<Toolchain>,
}

/// On-disk fixture shape (optional metadata fields ignored by consumers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HierarchyFixture {
    parent: Toolchain,
    members: Vec<Toolchain>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    add_system_to_minimal_toolchains: Option<bool>,
}

#[derive(Debug, Error)]
pub enum HierarchyError {
    #[error("IO {0}: {1}")]
    Io(String, #[source] std::io::Error),
    #[error("parse hierarchy fixture {0}: {1}")]
    Parse(String, String),
    #[error("no known hierarchy for toolchain {0}-{1}")]
    UnknownToolchain(String, String),
    #[error("dependency {0} not found in universe under hierarchy of {1}-{2}")]
    MissingDep(String, String, String),
}

impl ToolchainHierarchy {
    /// Labels `name-version` for each member (system empty version → `system`).
    pub fn member_labels(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|t| {
                if is_system_toolchain(t) {
                    "system".into()
                } else if t.version.is_empty() {
                    t.name.clone()
                } else {
                    t.label()
                }
            })
            .collect()
    }

    /// Whether `tc` is a member of this generation's hierarchy.
    pub fn contains(&self, tc: &Toolchain) -> bool {
        self.members.iter().any(|m| toolchains_match(m, tc))
    }
}

/// EasyBuild / parser SYSTEM toolchains: name `system` (any case), version empty or `system`.
pub fn is_system_toolchain(tc: &Toolchain) -> bool {
    tc.name.eq_ignore_ascii_case("system")
}

/// Equality for hierarchy membership, with SYSTEM normalization.
pub fn toolchains_match(a: &Toolchain, b: &Toolchain) -> bool {
    if is_system_toolchain(a) && is_system_toolchain(b) {
        return true;
    }
    a.name == b.name && a.version == b.version
}

/// Load a hierarchy fixture JSON file.
pub fn load_hierarchy_fixture(path: &Path) -> Result<ToolchainHierarchy, HierarchyError> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| HierarchyError::Io(path.display().to_string(), e))?;
    let fix: HierarchyFixture = serde_json::from_str(&s)
        .map_err(|e| HierarchyError::Parse(path.display().to_string(), e.to_string()))?;
    Ok(ToolchainHierarchy {
        parent: fix.parent,
        members: fix.members,
    })
}

/// Built-in hierarchy fixtures embedded at compile time (no EasyBuild at test/runtime).
pub fn known_hierarchy(parent: &Toolchain) -> Option<ToolchainHierarchy> {
    let key = format!("{}-{}", parent.name, parent.version);
    let raw = match key.as_str() {
        "foss-2024a" => Some(include_str!(
            "../fixtures/toolchain_hierarchy/foss-2024a.json"
        )),
        "foss-2023b" => Some(include_str!(
            "../fixtures/toolchain_hierarchy/foss-2023b.json"
        )),
        "foss-2025a" => Some(include_str!(
            "../fixtures/toolchain_hierarchy/foss-2025a.json"
        )),
        _ => None,
    }?;
    let fix: HierarchyFixture = serde_json::from_str(raw).ok()?;
    Some(ToolchainHierarchy {
        parent: fix.parent,
        members: fix.members,
    })
}

/// Resolve hierarchy for `parent`: optional fixture path, else built-in known map.
pub fn hierarchy_for(
    parent: &Toolchain,
    fixture_path: Option<&Path>,
) -> Result<ToolchainHierarchy, HierarchyError> {
    if let Some(p) = fixture_path {
        return load_hierarchy_fixture(p);
    }
    known_hierarchy(parent).ok_or_else(|| {
        HierarchyError::UnknownToolchain(parent.name.clone(), parent.version.clone())
    })
}

/// Keep candidates whose toolchain is any member of `hierarchy`.
pub fn filter_candidates_in_hierarchy(
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
) -> Vec<Candidate> {
    cands
        .iter()
        .filter(|c| hierarchy.contains(&c.toolchain))
        .cloned()
        .collect()
}

/// Options controlling safe hierarchy dependency resolution.
#[derive(Debug, Clone, Default)]
pub struct ResolveDepOpts<'a> {
    /// Never return a version older than this floor (source recipe version).
    pub floor_version: Option<&'a str>,
    /// When set, only candidates with this versionsuffix match.
    /// When the source pins a non-empty versionsuffix, callers should typically
    /// **not bump** the dep at all (see [`resolve_dep_versions_for_specs`]).
    pub versionsuffix: Option<&'a str>,
    /// When true (generation bump), prefer generation-consensus version selection
    /// among eligible install candidates (see [`resolve_dep_version_in_hierarchy_opts`]).
    /// Default false preserves prefer_newer among ranks when no floor is set.
    pub use_consensus: bool,
}

/// Among hierarchy members for `name`, pick a safe version for the target generation.
///
/// Selection rules (in order):
/// 1. Candidate must be in `hierarchy` and match `name` (strict name+version
///    membership — out-of-generation GCCcore/GCC never qualify).
/// 2. Optional `versionsuffix` must match (empty/`None` matches candidates with no suffix).
/// 3. Optional `floor_version`: never pick a version **older** than the floor.
/// 4. Prefer candidates on the hierarchy **parent** toolchain when present,
///    otherwise the highest (deepest) hierarchy member that has a candidate.
/// 5. Among the same hierarchy rank (or when `use_consensus` / floor is set):
///    - **Generation consensus** (when `use_consensus` or a floor is set):
///      count which version of `name` other recipes in the hierarchy depend on
///      (runtime + build deps). If a version has a clear majority (≥ 80% of
///      pins) and is eligible, pick it. Otherwise fall back to the **newest**
///      eligible version that has at least one generation pin (else newest
///      eligible). This matches maintainer CMake 3.29.3 (clear majority) and
///      scikit-build-core 0.11.1 (weak plurality → newest used).
///    - if no floor and not consensus: pick the **newest** version by [`cmp_version`].
///
/// Returns `None` when no candidate satisfies the filters.
pub fn resolve_dep_version_in_hierarchy(
    name: &str,
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
) -> Option<String> {
    resolve_dep_version_in_hierarchy_opts(name, cands, hierarchy, &ResolveDepOpts::default())
}

/// Rank of `tc` in `hierarchy` (parent last = highest). Uses [`toolchains_match`]
/// so SYSTEM empty/`system` labels compare equal. Out-of-hierarchy → `None`.
pub fn hierarchy_member_rank(hierarchy: &ToolchainHierarchy, tc: &Toolchain) -> Option<usize> {
    hierarchy
        .members
        .iter()
        .enumerate()
        .find(|(_, m)| toolchains_match(m, tc))
        .map(|(i, _)| i)
}

/// Normalize a solver `version_req` to a bare exact version for consensus
/// counting. Accepts EasyBuild pins after [`crate::eb_parse::version_field_to_req`]
/// (`==3.29.3` → `3.29.3`) and bare versions. Returns `None` for ranges / globs.
fn exact_pin_version(version_req: &str) -> Option<&str> {
    let ver = version_req.trim();
    if ver.is_empty() || ver == "*" {
        return None;
    }
    // Exact equality pin from version_field_to_req.
    if let Some(rest) = ver.strip_prefix("==") {
        let rest = rest.trim();
        if rest.is_empty() || rest.contains(',') {
            return None;
        }
        return Some(rest);
    }
    // Loose / range requirements — not consensus-countable.
    if ver.starts_with(">=")
        || ver.starts_with("<=")
        || ver.starts_with("!=")
        || ver.starts_with('>')
        || ver.starts_with('<')
        || ver.starts_with('~')
        || ver.starts_with('^')
        || ver.starts_with('=')
        || ver.contains(',')
    {
        return None;
    }
    Some(ver)
}

/// Count exact version pins of dependency `name` among recipes whose **own**
/// toolchain is in `hierarchy` (generation-scoped reverse-deps).
///
/// Both `dependencies` and `builddependencies` are counted. Only exact version
/// pins are tallied (`3.29.3` or solver form `==3.29.3`); empty/range reqs skip.
///
/// Pure function for unit tests: pass a synthetic `[Candidate]` universe.
pub fn count_generation_dep_versions(
    name: &str,
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for consumer in cands {
        if hierarchy_member_rank(hierarchy, &consumer.toolchain).is_none() {
            continue;
        }
        for dep in consumer.dependencies.iter().chain(consumer.builddependencies.iter()) {
            if dep.name != name {
                continue;
            }
            let Some(ver) = exact_pin_version(&dep.version_req) else {
                continue;
            };
            *counts.entry(ver.to_string()).or_insert(0) += 1;
        }
    }
    counts
}

/// Prefer generation-compiler / composite toolchains over SYSTEM among already
/// hierarchy-eligible install candidates.
///
/// SYSTEM remains a hierarchy member (for rare binary-only build tools), but when
/// a GCCcore/GCC/gfbf/… candidate exists it must win empty-consensus “newest”
/// fallbacks (e.g. CMake 3.29.3 @ GCCcore-13.3.0 over CMake 3.31.8 @ SYSTEM).
///
/// Returns `eligible` unchanged when every candidate is SYSTEM (or the list is empty).
pub fn prefer_non_system_candidates<'a>(eligible: &[&'a Candidate]) -> Vec<&'a Candidate> {
    let non_sys: Vec<&'a Candidate> = eligible
        .iter()
        .copied()
        .filter(|c| !is_system_toolchain(&c.toolchain))
        .collect();
    if non_sys.is_empty() {
        eligible.to_vec()
    } else {
        non_sys
    }
}

/// Pick a generation-consensus version of `name` among `eligible` install versions.
///
/// - Clear majority (≥ 80% of generation pins of `name` that land in `eligible`):
///   that modal version.
/// - Else: **newest** among eligible versions that appear in the pin counts
///   (weak signal / no unique consensus).
/// - If no pin counts hit eligible: **newest** eligible (true no-signal fallback).
///
/// `eligible` must already be floor/suffix/hierarchy filtered. Empty → `None`.
/// Callers that hold full [`Candidate`]s should first run
/// [`prefer_non_system_candidates`] so SYSTEM does not beat GCCcore on version alone.
pub fn pick_consensus_version(
    counts: &HashMap<String, usize>,
    eligible: &[String],
) -> Option<String> {
    if eligible.is_empty() {
        return None;
    }
    // Restrict counts to eligible install versions.
    let mut filtered: Vec<(&str, usize)> = eligible
        .iter()
        .filter_map(|v| counts.get(v).map(|c| (v.as_str(), *c)))
        .filter(|(_, c)| *c > 0)
        .collect();
    if filtered.is_empty() {
        // No consensus signal at all → newest eligible.
        return eligible
            .iter()
            .max_by(|a, b| cmp_version(a, b))
            .cloned();
    }
    let total: usize = filtered.iter().map(|(_, c)| c).sum();
    // Modal: highest count; ties broken by newer version.
    filtered.sort_by(|(va, ca), (vb, cb)| {
        cb.cmp(ca).then_with(|| cmp_version(va, vb))
    });
    let (modal_ver, modal_count) = filtered[0];
    let clear = modal_count.saturating_mul(CONSENSUS_CLEAR_MAJORITY_DEN)
        >= total.saturating_mul(CONSENSUS_CLEAR_MAJORITY_NUM);
    if clear {
        return Some(modal_ver.to_string());
    }
    // Weak plurality: newest among versions that have pins.
    filtered
        .into_iter()
        .map(|(v, _)| v)
        .max_by(|a, b| cmp_version(a, b))
        .map(|s| s.to_string())
}

/// Like [`resolve_dep_version_in_hierarchy`] with floor / versionsuffix filters.
///
/// **Strict hierarchy:** only candidates whose toolchain name **and** version
/// are exact members of `hierarchy` (via [`ToolchainHierarchy::contains`] /
/// [`toolchains_match`]) are eligible. A newer package on GCCcore-14.x is
/// **not** valid for foss-2024a (GCCcore-13.3.0 only), even if the name matches.
pub fn resolve_dep_version_in_hierarchy_opts(
    name: &str,
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
    opts: &ResolveDepOpts<'_>,
) -> Option<String> {
    let want_suffix = opts.versionsuffix.unwrap_or("");
    let mut eligible: Vec<&Candidate> = Vec::new();

    for c in cands {
        if c.name != name {
            continue;
        }
        // Strict: name+version membership; out-of-generation GCCcore/GCC excluded.
        if hierarchy_member_rank(hierarchy, &c.toolchain).is_none() {
            continue;
        }
        let got_suffix = c.versionsuffix.as_deref().unwrap_or("");
        if got_suffix != want_suffix {
            continue;
        }
        if let Some(floor) = opts.floor_version {
            if cmp_version(&c.version, floor) == Ordering::Less {
                continue;
            }
        }
        eligible.push(c);
    }
    if eligible.is_empty() {
        return None;
    }

    // Prefer GCCcore/GCC/gfbf/… over SYSTEM before consensus/newest (SYSTEM only
    // when no compiler-toolchain candidate remains).
    let preferred = prefer_non_system_candidates(&eligible);

    let use_consensus = opts.use_consensus || opts.floor_version.is_some();
    if use_consensus {
        let versions: Vec<String> = {
            let mut v: Vec<String> = preferred.iter().map(|c| c.version.clone()).collect();
            v.sort();
            v.dedup();
            v
        };
        let counts = count_generation_dep_versions(name, cands, hierarchy);
        if let Some(picked) = pick_consensus_version(&counts, &versions) {
            return Some(picked);
        }
    }

    // Legacy prefer_newer / rank walk (no floor, no consensus).
    let mut best: Option<&Candidate> = None;
    let mut best_rank: usize = 0;
    for c in preferred {
        let rank = hierarchy_member_rank(hierarchy, &c.toolchain).unwrap_or(0);
        best = match best {
            None => {
                best_rank = rank;
                Some(c)
            }
            Some(prev) => {
                if rank > best_rank {
                    best_rank = rank;
                    Some(c)
                } else if rank < best_rank {
                    Some(prev)
                } else if cmp_version(&c.version, &prev.version) == Ordering::Greater {
                    Some(c)
                } else {
                    Some(prev)
                }
            }
        };
    }
    best.map(|c| c.version.clone())
}

/// Resolve versions for many dependency names. Names missing from the universe
/// are omitted from the map (callers may leave source versions unchanged).
pub fn resolve_dep_versions_in_hierarchy(
    names: impl IntoIterator<Item = impl AsRef<str>>,
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for n in names {
        let name = n.as_ref();
        if let Some(ver) = resolve_dep_version_in_hierarchy(name, cands, hierarchy) {
            out.insert(name.to_string(), ver);
        }
    }
    out
}

/// Like [`resolve_dep_versions_in_hierarchy`] but errors if any name is unresolved.
pub fn resolve_dep_versions_in_hierarchy_strict(
    names: impl IntoIterator<Item = impl AsRef<str>>,
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
) -> Result<HashMap<String, String>, HierarchyError> {
    let mut out = HashMap::new();
    for n in names {
        let name = n.as_ref().to_string();
        match resolve_dep_version_in_hierarchy(&name, cands, hierarchy) {
            Some(ver) => {
                out.insert(name, ver);
            }
            None => {
                return Err(HierarchyError::MissingDep(
                    name,
                    hierarchy.parent.name.clone(),
                    hierarchy.parent.version.clone(),
                ));
            }
        }
    }
    Ok(out)
}

/// One dependency as scraped from a source recipe (name + version + optional suffix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDepSpec {
    pub name: String,
    pub version: String,
    pub versionsuffix: Option<String>,
    /// 4th tuple element is EasyBuild `SYSTEM` (pseudo-external / binary pin).
    pub system_toolchain: bool,
    /// Trailing same-line comment marks an optional extra (e.g. `# optional`).
    /// Soft-unresolved only — does **not** freeze the pin when candidates exist.
    pub optional: bool,
}

impl SourceDepSpec {
    /// Convenience constructor for tests / simple name+version pins.
    pub fn plain(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            versionsuffix: None,
            system_toolchain: false,
            optional: false,
        }
    }

    /// Whether auto-resolve must leave the source version untouched without
    /// attempting hierarchy resolve: SYSTEM 4th-tuple or non-empty versionsuffix.
    ///
    /// Bare `# optional` is **not** a freeze pin — optional deps still bump when
    /// hierarchy candidates exist (soft-keep only if unresolved).
    pub fn is_preserve_pin(&self) -> bool {
        self.system_toolchain
            || self
                .versionsuffix
                .as_deref()
                .is_some_and(|vs| !vs.is_empty())
    }
}

/// Resolve many [`SourceDepSpec`]s with floor + versionsuffix safety.
///
/// - Deps with a **non-empty versionsuffix** are not bumped (caller keeps source).
/// - Deps with **SYSTEM** 4th-tuple toolchain are not bumped (keep source pin).
/// - Deps marked **optional** (`# optional` on that line) still resolve/bump when
///   hierarchy candidates exist; if none exist they soft-keep the source pin
///   (no hard ERROR) — optional is not a freeze, only soft-unresolved.
/// - Resolved versions are never older than the source version.
/// - Missing **non-optional** candidates yield [`HierarchyError::MissingDep`]
///   unless `keep_old` is true (then the source version is kept and listed in
///   `kept_old`).
pub fn resolve_dep_versions_for_specs(
    specs: &[SourceDepSpec],
    cands: &[Candidate],
    hierarchy: &ToolchainHierarchy,
    keep_old: bool,
) -> Result<(HashMap<String, String>, Vec<String>), HierarchyError> {
    let mut out = HashMap::new();
    let mut kept_old = Vec::new();
    for spec in specs {
        // versionsuffix-qualified deps stay at the source pin (do not bump).
        if let Some(vs) = spec.versionsuffix.as_deref() {
            if !vs.is_empty() {
                kept_old.push(format!(
                    "{} (versionsuffix {vs} pinned; not bumped)",
                    spec.name
                ));
                continue;
            }
        }
        // SYSTEM 4th-tuple (e.g. USEARCH): external/binary pin, not a generation bump.
        if spec.system_toolchain {
            kept_old.push(format!(
                "{} (SYSTEM toolchain pin {}; not bumped)",
                spec.name, spec.version
            ));
            continue;
        }
        let opts = ResolveDepOpts {
            floor_version: Some(spec.version.as_str()),
            versionsuffix: None,
            use_consensus: true,
        };
        match resolve_dep_version_in_hierarchy_opts(&spec.name, cands, hierarchy, &opts) {
            Some(ver) => {
                out.insert(spec.name.clone(), ver);
            }
            None => {
                // Optional deps soft-keep when unresolved (no hard ERROR).
                // Non-optional: keep_old or hard fail.
                if spec.optional || keep_old {
                    let reason = if spec.optional {
                        format!(
                            "{} (optional; no candidate under {}-{}; keeping source {})",
                            spec.name,
                            hierarchy.parent.name,
                            hierarchy.parent.version,
                            spec.version
                        )
                    } else {
                        format!(
                            "{} (no candidate under {}-{}; keeping source {})",
                            spec.name,
                            hierarchy.parent.name,
                            hierarchy.parent.version,
                            spec.version
                        )
                    };
                    kept_old.push(reason);
                } else {
                    return Err(HierarchyError::MissingDep(
                        spec.name.clone(),
                        hierarchy.parent.name.clone(),
                        hierarchy.parent.version.clone(),
                    ));
                }
            }
        }
    }
    Ok((out, kept_old))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eb_parse::parse_easyconfig_tree;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn foss(ver: &str) -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: ver.into(),
        }
    }

    #[test]
    fn known_foss_2024a_includes_required_levels() {
        let h = known_hierarchy(&foss("2024a")).expect("embedded foss-2024a");
        let labels = h.member_labels();
        for need in [
            "system",
            "GCCcore-13.3.0",
            "GCC-13.3.0",
            "gfbf-2024a",
            "gompi-2024a",
            "foss-2024a",
        ] {
            assert!(
                labels.iter().any(|l| l == need),
                "missing {need} in {labels:?}"
            );
        }
        assert_eq!(h.parent, foss("2024a"));
    }

    #[test]
    fn load_fixture_matches_known() {
        let path = fixture_root().join("fixtures/toolchain_hierarchy/foss-2024a.json");
        let loaded = load_hierarchy_fixture(&path).expect("load");
        let known = known_hierarchy(&foss("2024a")).unwrap();
        assert_eq!(loaded, known);
    }

    #[test]
    fn system_membership_normalizes_empty_and_system_version() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        assert!(h.contains(&Toolchain {
            name: "system".into(),
            version: "".into(),
        }));
        assert!(h.contains(&Toolchain {
            name: "system".into(),
            version: "system".into(),
        }));
        assert!(h.contains(&Toolchain {
            name: "SYSTEM".into(),
            version: "SYSTEM".into(),
        }));
    }

    #[test]
    fn resolve_multi_subtoolchain_universe_picks_generation_versions() {
        let root = fixture_root().join("fixtures/hierarchy_resolve/easyconfigs");
        let cands = parse_easyconfig_tree(&root).expect("parse universe").candidates;
        let h = known_hierarchy(&foss("2024a")).unwrap();

        // Exact-toolchain filter would find zero of these under foss-2024a alone.
        let exact_foss: Vec<_> = cands
            .iter()
            .filter(|c| c.toolchain.name == "foss" && c.toolchain.version == "2024a")
            .map(|c| c.name.as_str())
            .collect();
        assert!(
            !exact_foss.contains(&"Python"),
            "Python must not live at foss-2024a in this universe"
        );

        assert_eq!(
            resolve_dep_version_in_hierarchy("Python", &cands, &h).as_deref(),
            Some("3.12.3"),
            "must pick GCCcore-13.3.0 Python, not GCCcore-14.2.0 decoy 3.13.1"
        );
        assert_eq!(
            resolve_dep_version_in_hierarchy("CMake", &cands, &h).as_deref(),
            Some("3.30.0"),
            "prefer_newer among hierarchy: foss-level 3.30.0 over GCCcore 3.29.3"
        );
        assert_eq!(
            resolve_dep_version_in_hierarchy("SciPy-bundle", &cands, &h).as_deref(),
            Some("2024.05")
        );
        assert_eq!(
            resolve_dep_version_in_hierarchy("mpi4py", &cands, &h).as_deref(),
            Some("4.0.1")
        );
        assert_eq!(
            resolve_dep_version_in_hierarchy("networkx", &cands, &h).as_deref(),
            Some("3.4.2")
        );
        assert_eq!(
            resolve_dep_version_in_hierarchy("scikit-build-core", &cands, &h).as_deref(),
            Some("0.11.1")
        );
    }

    #[test]
    fn filter_candidates_in_hierarchy_excludes_other_generations() {
        let root = fixture_root().join("fixtures/hierarchy_resolve/easyconfigs");
        let cands = parse_easyconfig_tree(&root).expect("parse").candidates;
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let filtered = filter_candidates_in_hierarchy(&cands, &h);
        assert!(filtered.iter().all(|c| h.contains(&c.toolchain)));
        assert!(!filtered.iter().any(|c| c.version == "3.13.1"));
        assert!(!filtered.iter().any(|c| c.version == "2025.06"));
    }

    fn cand(name: &str, ver: &str, tc_name: &str, tc_ver: &str, vs: Option<&str>) -> Candidate {
        Candidate {
            name: name.into(),
            version: ver.into(),
            toolchain: Toolchain {
                name: tc_name.into(),
                version: tc_ver.into(),
            },
            versionsuffix: vs.map(str::to_string),
            easyconfig_path: format!("{name}-{ver}.eb"),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        }
    }

    fn dep_pin(name: &str, ver: &str) -> crate::domain::DepReq {
        crate::domain::DepReq {
            name: name.into(),
            // Mirror version_field_to_req: exact pins become ==V in Candidates.
            version_req: format!("=={ver}"),
            versionsuffix: None,
            toolchain: None,
        }
    }

    #[test]
    fn exact_pin_version_strips_double_equals() {
        assert_eq!(exact_pin_version("==3.29.3"), Some("3.29.3"));
        assert_eq!(exact_pin_version("3.29.3"), Some("3.29.3"));
        assert_eq!(exact_pin_version(">=3.29"), None);
        assert_eq!(exact_pin_version("*"), None);
    }

    /// Consumer recipe in the generation that pins `dep_name` at `dep_ver` (builddep).
    fn consumer_pinning(
        pkg: &str,
        pkg_ver: &str,
        tc_name: &str,
        tc_ver: &str,
        dep_name: &str,
        dep_ver: &str,
    ) -> Candidate {
        let mut c = cand(pkg, pkg_ver, tc_name, tc_ver, None);
        c.builddependencies = vec![dep_pin(dep_name, dep_ver)];
        c
    }

    #[test]
    fn resolve_never_returns_older_than_floor() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        // Legacy Cython 0.29 and modern 3.0.10 both on GCCcore-13.3.0.
        let cands = vec![
            cand("Cython", "0.29.37", "GCCcore", "13.3.0", None),
            cand("Cython", "3.0.10", "GCCcore", "13.3.0", None),
        ];
        // Floor at 3.0.0: must not snap to 0.29.37.
        let opts = ResolveDepOpts {
            floor_version: Some("3.0.0"),
            versionsuffix: None,
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("Cython", &cands, &h, &opts).as_deref(),
            Some("3.0.10")
        );
        // Floor at 3.0.10 with only older available → None.
        let only_old = vec![cand("Cython", "0.29.37", "GCCcore", "13.3.0", None)];
        assert!(
            resolve_dep_version_in_hierarchy_opts("Cython", &only_old, &h, &opts).is_none()
        );
    }

    #[test]
    fn resolve_versionsuffix_match_and_specs_do_not_bump_suffix_pins() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![
            cand("LLVM", "14.0.6", "GCCcore", "13.3.0", Some("-llvmlite")),
            cand("LLVM", "18.1.0", "GCCcore", "13.3.0", None),
            cand("ASE", "3.22.1", "foss", "2024a", Some("-Python-3.12")),
            cand("ASE", "3.23.0", "foss", "2024a", None),
        ];
        // When looking for plain LLVM, do not pick the -llvmlite build.
        let plain = ResolveDepOpts {
            floor_version: Some("14.0.0"),
            versionsuffix: None,
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("LLVM", &cands, &h, &plain).as_deref(),
            Some("18.1.0")
        );
        // When looking for -llvmlite, only that suffix matches.
        let llvmlite = ResolveDepOpts {
            floor_version: Some("14.0.0"),
            versionsuffix: Some("-llvmlite"),
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("LLVM", &cands, &h, &llvmlite).as_deref(),
            Some("14.0.6")
        );

        // Specs: versionsuffix-pinned deps are not rewritten.
        let specs = vec![
            SourceDepSpec {
                name: "LLVM".into(),
                version: "14.0.6".into(),
                versionsuffix: Some("-llvmlite".into()),
                system_toolchain: false,
                optional: false,
            },
            SourceDepSpec {
                name: "ASE".into(),
                version: "3.22.1".into(),
                versionsuffix: Some("-Python-3.12".into()),
                system_toolchain: false,
                optional: false,
            },
            SourceDepSpec::plain("Python", "3.12.0"),
        ];
        let cands2 = {
            let mut v = cands;
            v.push(cand("Python", "3.12.3", "GCCcore", "13.3.0", None));
            v
        };
        let (map, kept) = resolve_dep_versions_for_specs(&specs, &cands2, &h, false).unwrap();
        assert!(!map.contains_key("LLVM"), "suffix-pinned LLVM must not bump");
        assert!(!map.contains_key("ASE"), "suffix-pinned ASE must not bump");
        assert_eq!(map.get("Python").map(String::as_str), Some("3.12.3"));
        assert!(kept.iter().any(|k| k.contains("LLVM")));
        assert!(kept.iter().any(|k| k.contains("ASE")));
    }

    #[test]
    fn resolve_missing_is_error_unless_keep_old() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![cand("Python", "3.12.3", "GCCcore", "13.3.0", None)];
        let specs = vec![SourceDepSpec::plain("MissingPkg", "1.0")];
        let err = resolve_dep_versions_for_specs(&specs, &cands, &h, false).unwrap_err();
        assert!(
            matches!(err, HierarchyError::MissingDep(ref n, _, _) if n == "MissingPkg"),
            "{err}"
        );
        let (map, kept) = resolve_dep_versions_for_specs(&specs, &cands, &h, true).unwrap();
        assert!(map.is_empty());
        assert!(kept.iter().any(|k| k.contains("MissingPkg") && k.contains("keeping")));
    }

    #[test]
    fn resolve_prefers_target_generation_member_over_global_legacy() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        // Cython on older GCCcore is NOT in foss-2024a hierarchy; only 13.3.0 is.
        let cands = vec![
            cand("Cython", "0.29.37", "GCCcore", "12.3.0", None),
            cand("Cython", "3.0.10", "GCCcore", "13.3.0", None),
        ];
        assert_eq!(
            resolve_dep_version_in_hierarchy("Cython", &cands, &h).as_deref(),
            Some("3.0.10"),
            "must pick target-generation Cython, not legacy GCCcore-12.3.0"
        );
    }

    #[test]
    fn resolve_excludes_newer_gcccore_outside_generation_even_if_globally_newest() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        // Scale-study failure mode: CMake 3.31.8 only on GCCcore-14.x must not win
        // over hierarchy-native CMake 3.29.3 at GCCcore-13.3.0 for foss-2024a.
        let cands = vec![
            cand("CMake", "3.29.3", "GCCcore", "13.3.0", None),
            cand("CMake", "3.31.8", "GCCcore", "14.3.0", None),
            cand("CMake", "3.31.11", "GCCcore", "15.2.0", None),
            cand("CMake", "3.31.8", "system", "system", None),
        ];
        // SYSTEM is in hierarchy (rank 0); GCCcore-13.3.0 ranks higher → 3.29.3
        // beats SYSTEM 3.31.8. Out-of-gen 14.x/15.x must never be selected.
        assert_eq!(
            resolve_dep_version_in_hierarchy("CMake", &cands, &h).as_deref(),
            Some("3.29.3"),
            "must pick GCCcore-13.3.0 CMake, not GCCcore-14+/SYSTEM global newest"
        );
        // If only out-of-generation candidates exist, resolve returns None.
        let only_new = vec![
            cand("CMake", "3.31.8", "GCCcore", "14.3.0", None),
            cand("CMake", "4.0.3", "GCCcore", "15.2.0", None),
        ];
        assert!(
            resolve_dep_version_in_hierarchy("CMake", &only_new, &h).is_none(),
            "no in-hierarchy CMake → None (not a silent global pick)"
        );
    }

    #[test]
    fn resolve_with_floor_uses_generation_consensus_not_blind_newest() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        // Real tree: both CMake 3.29.3 and 3.31.8 ship on GCCcore-13.3.0.
        // Clear majority of generation consumers pin 3.29.3 → consensus wins.
        let mut cands = vec![
            cand("CMake", "3.29.3", "GCCcore", "13.3.0", None),
            cand("CMake", "3.31.8", "GCCcore", "13.3.0", None),
            cand("CMake", "3.31.8", "GCCcore", "14.3.0", None),
        ];
        for i in 0..10 {
            cands.push(consumer_pinning(
                &format!("App{i}"),
                "1.0",
                "foss",
                "2024a",
                "CMake",
                "3.29.3",
            ));
        }
        cands.push(consumer_pinning("NewApp", "1.0", "foss", "2024a", "CMake", "3.31.8"));
        let opts = ResolveDepOpts {
            floor_version: Some("3.27.6"),
            versionsuffix: None,
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("CMake", &cands, &h, &opts).as_deref(),
            Some("3.29.3")
        );
        // scikit-learn: clear majority pins 1.5.2 → not newest 1.6.1.
        let mut sk = vec![
            cand("scikit-learn", "1.5.2", "gfbf", "2024a", None),
            cand("scikit-learn", "1.6.1", "gfbf", "2024a", None),
            cand("scikit-learn", "1.7.0", "gfbf", "2025a", None),
        ];
        for i in 0..8 {
            sk.push(consumer_pinning(
                &format!("Sci{i}"),
                "1.0",
                "gfbf",
                "2024a",
                "scikit-learn",
                "1.5.2",
            ));
        }
        sk.push(consumer_pinning("SciNew", "1.0", "gfbf", "2024a", "scikit-learn", "1.6.1"));
        let opts_sk = ResolveDepOpts {
            floor_version: Some("1.4.0"),
            versionsuffix: None,
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("scikit-learn", &sk, &h, &opts_sk).as_deref(),
            Some("1.5.2")
        );
    }

    #[test]
    fn consensus_modal_clear_majority_wins() {
        let mut counts = HashMap::new();
        counts.insert("3.29.3".into(), 20);
        counts.insert("3.31.8".into(), 2);
        let eligible = vec!["3.29.3".into(), "3.31.8".into()];
        assert_eq!(
            pick_consensus_version(&counts, &eligible).as_deref(),
            Some("3.29.3")
        );
    }

    #[test]
    fn consensus_weak_plurality_falls_back_to_newest_used() {
        // scikit-build-core style: 17 vs 5 is only ~77% — not a clear majority.
        let mut counts = HashMap::new();
        counts.insert("0.10.6".into(), 17);
        counts.insert("0.11.1".into(), 5);
        let eligible = vec!["0.10.6".into(), "0.11.1".into()];
        assert_eq!(
            pick_consensus_version(&counts, &eligible).as_deref(),
            Some("0.11.1"),
            "weak plurality → newest among used"
        );
    }

    #[test]
    fn consensus_empty_signal_falls_back_to_newest_eligible() {
        let counts = HashMap::new();
        let eligible = vec!["0.10.6".into(), "0.11.1".into()];
        assert_eq!(
            pick_consensus_version(&counts, &eligible).as_deref(),
            Some("0.11.1")
        );
    }

    #[test]
    fn consensus_respects_floor_via_eligible_filter() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let mut cands = vec![
            cand("CMake", "3.29.3", "GCCcore", "13.3.0", None),
            cand("CMake", "3.31.8", "GCCcore", "13.3.0", None),
        ];
        // Majority pins old 3.29.3, but floor at 3.30.0 excludes it.
        for i in 0..10 {
            cands.push(consumer_pinning(
                &format!("App{i}"),
                "1.0",
                "foss",
                "2024a",
                "CMake",
                "3.29.3",
            ));
        }
        let opts = ResolveDepOpts {
            floor_version: Some("3.30.0"),
            versionsuffix: None,
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("CMake", &cands, &h, &opts).as_deref(),
            Some("3.31.8"),
            "floor must exclude older consensus pin"
        );
    }

    #[test]
    fn empty_consensus_prefers_gcccore_over_system_newest() {
        // Mirrors auto_resolve_cmake_ignores_out_of_generation_gcccore universe:
        // no reverse-dep pins → empty consensus → must not pick SYSTEM 3.31.8.
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![
            cand("CMake", "3.29.3", "GCCcore", "13.3.0", None),
            cand("CMake", "3.31.8", "GCCcore", "14.3.0", None), // out of gen
            cand("CMake", "3.31.8", "system", "system", None),
        ];
        let opts = ResolveDepOpts {
            floor_version: Some("3.27.6"),
            versionsuffix: None,
            use_consensus: true,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("CMake", &cands, &h, &opts).as_deref(),
            Some("3.29.3"),
            "empty consensus must prefer GCCcore-13.3.0 over SYSTEM 3.31.8"
        );
        // SYSTEM-only eligible still resolves.
        let only_sys = vec![cand("CMake", "3.31.8", "system", "system", None)];
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("CMake", &only_sys, &h, &opts).as_deref(),
            Some("3.31.8")
        );
    }

    #[test]
    fn prefer_non_system_candidates_keeps_system_when_alone() {
        let gcc = cand("CMake", "3.29.3", "GCCcore", "13.3.0", None);
        let sys = cand("CMake", "3.31.8", "system", "system", None);
        let both = prefer_non_system_candidates(&[&gcc, &sys]);
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].version, "3.29.3");
        let only = prefer_non_system_candidates(&[&sys]);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].version, "3.31.8");
    }

    #[test]
    fn count_generation_dep_versions_scopes_to_hierarchy_only() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![
            cand("CMake", "3.29.3", "GCCcore", "13.3.0", None),
            consumer_pinning("InGen", "1.0", "foss", "2024a", "CMake", "3.29.3"),
            // Out-of-generation consumer must NOT count.
            consumer_pinning("OutGen", "1.0", "foss", "2025a", "CMake", "3.31.8"),
            consumer_pinning("InGen2", "1.0", "gfbf", "2024a", "CMake", "3.29.3"),
        ];
        let counts = count_generation_dep_versions("CMake", &cands, &h);
        assert_eq!(counts.get("3.29.3").copied(), Some(2));
        assert!(!counts.contains_key("3.31.8"), "out-of-gen pins must not count: {counts:?}");
    }

    #[test]
    fn resolve_specs_system_preserve_optional_bumps_when_candidates_exist() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![
            cand("Python", "3.12.3", "GCCcore", "13.3.0", None),
            // In-hierarchy newer ASE — optional must still bump (not freeze).
            cand("ASE", "3.24.0", "foss", "2024a", None),
            cand("USEARCH", "12.0", "GCCcore", "13.3.0", None), // decoy — must not bump SYSTEM pin
            cand("networkx", "3.4.2", "foss", "2024a", None),
            cand("PyTables", "3.10.2", "foss", "2024a", None),
        ];
        let specs = vec![
            SourceDepSpec {
                name: "USEARCH".into(),
                version: "11.0.667-i86linux32".into(),
                versionsuffix: None,
                system_toolchain: true,
                optional: false,
            },
            SourceDepSpec {
                name: "ASE".into(),
                version: "3.23.0".into(),
                versionsuffix: None,
                system_toolchain: false,
                optional: true,
            },
            // MDTraj-style optional extras: must bump when candidates exist.
            SourceDepSpec {
                name: "networkx".into(),
                version: "3.2.1".into(),
                versionsuffix: None,
                system_toolchain: false,
                optional: true,
            },
            SourceDepSpec {
                name: "PyTables".into(),
                version: "3.9.2".into(),
                versionsuffix: None,
                system_toolchain: false,
                optional: true,
            },
            SourceDepSpec::plain("Python", "3.12.0"),
        ];
        let (map, kept) = resolve_dep_versions_for_specs(&specs, &cands, &h, false).unwrap();
        assert!(
            !map.contains_key("USEARCH"),
            "SYSTEM pin must not be bumped: {map:?}"
        );
        assert_eq!(
            map.get("ASE").map(String::as_str),
            Some("3.24.0"),
            "optional ASE with hierarchy candidates must bump: {map:?}"
        );
        assert_eq!(map.get("networkx").map(String::as_str), Some("3.4.2"));
        assert_eq!(map.get("PyTables").map(String::as_str), Some("3.10.2"));
        assert_eq!(map.get("Python").map(String::as_str), Some("3.12.3"));
        assert!(kept.iter().any(|k| k.contains("USEARCH") && k.contains("SYSTEM")));
        assert!(!SourceDepSpec {
            name: "ASE".into(),
            version: "3.23.0".into(),
            versionsuffix: None,
            system_toolchain: false,
            optional: true,
        }
        .is_preserve_pin());
        assert!(SourceDepSpec {
            name: "USEARCH".into(),
            version: "11.0".into(),
            versionsuffix: None,
            system_toolchain: true,
            optional: false,
        }
        .is_preserve_pin());
        // Missing non-optional still errors.
        let bad = vec![SourceDepSpec::plain("MissingPkg", "1.0")];
        assert!(resolve_dep_versions_for_specs(&bad, &cands, &h, false).is_err());
        // Missing optional never hard-fails (soft-keep).
        let opt_missing = vec![SourceDepSpec {
            name: "OptionalGhost".into(),
            version: "0.1".into(),
            versionsuffix: None,
            system_toolchain: false,
            optional: true,
        }];
        let (m2, k2) = resolve_dep_versions_for_specs(&opt_missing, &cands, &h, false).unwrap();
        assert!(m2.is_empty());
        assert!(k2.iter().any(|k| k.contains("OptionalGhost") && k.contains("optional")));
    }

    #[test]
    fn hierarchy_member_rank_matches_system_normalization() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let sys_empty = Toolchain {
            name: "system".into(),
            version: "".into(),
        };
        let sys_sys = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let gcc = Toolchain {
            name: "GCCcore".into(),
            version: "13.3.0".into(),
        };
        let gcc14 = Toolchain {
            name: "GCCcore".into(),
            version: "14.3.0".into(),
        };
        assert_eq!(hierarchy_member_rank(&h, &sys_empty), hierarchy_member_rank(&h, &sys_sys));
        assert!(hierarchy_member_rank(&h, &gcc).unwrap() > hierarchy_member_rank(&h, &sys_sys).unwrap());
        assert!(hierarchy_member_rank(&h, &gcc14).is_none());
    }
}
