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
/// weak and we fall back to newest-among-used (still at least floor).
///
/// Rationale: pure plurality can favor an older back-ported pin that many
/// packages still list an older release while maintainers of key applications
/// already moved to a newer one. A clear majority at
/// ~97%) still wins; weak plurality falls through to newest-in-generation.
const CONSENSUS_CLEAR_MAJORITY_NUM: usize = 4;
const CONSENSUS_CLEAR_MAJORITY_DEN: usize = 5; // 4/5 = 80%

/// Ordered sub-toolchain hierarchy for one parent toolchain generation.
///
/// Member order matches EasyBuild: most minimal first, parent last
/// (e.g. `system`, `GCCcore`, `GCC`, `gfbf`, `gompi`, `foss`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolchainHierarchy {
    /// The toolchain the members are subtoolchains of.
    pub parent: Toolchain,
    /// Subtoolchains, which a dependency may be built against instead.
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
/// Why a toolchain hierarchy could not be resolved.
pub enum HierarchyError {
    #[error("IO {0}: {1}")]
    /// A hierarchy file could not be read.
    Io(String, #[source] std::io::Error),
    #[error("parse hierarchy fixture {0}: {1}")]
    /// A hierarchy file could not be understood.
    Parse(String, String),
    #[error("no known hierarchy for toolchain {0}-{1}")]
    /// A toolchain named in the hierarchy is not defined.
    UnknownToolchain(String, String),
    #[error("dependency {0} not found in universe under hierarchy of {1}-{2}{3}")]
    /// A dependency could not be placed anywhere in the hierarchy.
    MissingDep(String, String, String, String),
}

/// Short "what DOES exist" suffix for a missing dependency: up to four
/// name-matching candidates across all generations, so the failure reads as a
/// work item (bump/author from one of these) instead of a dead end. Returns
/// an empty-or-parenthesised string safe to append to an error message.
pub fn nearest_candidates_hint(name: &str, cands: &[Candidate]) -> String {
    let mut seen: Vec<String> = cands
        .iter()
        .filter(|c| c.name == name)
        .map(|c| {
            let tc = if is_system_toolchain(&c.toolchain) {
                "SYSTEM".to_string()
            } else {
                c.toolchain.label()
            };
            format!(
                "{}{} @ {}",
                c.version,
                c.versionsuffix.clone().unwrap_or_default(),
                tc
            )
        })
        .collect();
    seen.sort();
    seen.dedup();
    if seen.is_empty() {
        return " (no candidate with this name at any generation)".to_string();
    }
    let extra = seen.len().saturating_sub(4);
    let head = seen.into_iter().take(4).collect::<Vec<_>>().join(", ");
    if extra > 0 {
        format!(" (available at other generations: {head}, +{extra} more)")
    } else {
        format!(" (available at other generations: {head})")
    }
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
    // EasyBuild called this toolchain `dummy` before it was renamed to
    // `system`, and a robot path outlives the rename: a 2019 tree writes 1374
    // of its recipes that way. Reading `dummy` as an ordinary toolchain makes
    // every one of them unsatisfiable, so the two spellings mean the same
    // thing here.
    tc.name.eq_ignore_ascii_case("system") || tc.name.eq_ignore_ascii_case("dummy")
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
    // Bare GCC-family compiler targets arise when a companion recipe is
    // retargeted to a member of a composite hierarchy. GCCcore admits SYSTEM;
    // GCC additionally admits its same-version GCCcore base.
    if matches!(parent.name.as_str(), "GCCcore" | "GCC") && !parent.version.is_empty() {
        let mut members = vec![Toolchain {
            name: "system".into(),
            version: String::new(),
        }];
        if parent.name == "GCC" {
            members.push(Toolchain {
                name: "GCCcore".into(),
                version: parent.version.clone(),
            });
        }
        members.push(parent.clone());
        return Some(ToolchainHierarchy {
            parent: parent.clone(),
            members,
        });
    }
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
        "foss-2025b" => Some(include_str!(
            "../fixtures/toolchain_hierarchy/foss-2025b.json"
        )),
        "foss-2026.1" => Some(include_str!(
            "../fixtures/toolchain_hierarchy/foss-2026.1.json"
        )),
        _ => None,
    }?;
    let fix: HierarchyFixture = serde_json::from_str(raw).ok()?;
    Some(ToolchainHierarchy {
        parent: fix.parent,
        members: fix.members,
    })
}

/// Derive a GCC-family generation hierarchy from the parsed easyconfig universe.
///
/// The robot tree itself defines each generation: the `foss-<gen>` (or
/// `gompi-<gen>` / `gfbf-<gen>`) toolchain-definition easyconfig pins the
/// generation's `GCC` version, and the intermediate composite definitions
/// exist as sibling recipes. Deriving from the tree makes any generation
/// present in the robot tree work with no fixture (the annual-bump case:
/// a brand-new `foss-2026.1` must not require shipping a new fixture).
///
/// Members mirror EasyBuild's `get_toolchain_hierarchy` order for foss-family
/// generations: `system < GCCcore < GCC < gompi < gfbf < parent`. Intermediate
/// composites are included only when their definition recipe is in the tree.
/// GCCcore is assumed version-paired with GCC (true for all modern
/// generations, 2020a+). Non-GCC-family parents return `None`.
/// A hierarchy read off the tree, for a toolchain no family rule covers.
///
/// A toolchain is defined by a recipe, and that recipe's dependencies say what
/// it is made of: `iimpi` names `intel-compilers` and `impi`, and
/// `intel-compilers` names the `GCCcore` it was built on. Any dependency that
/// other recipes in the tree use *as* a toolchain is a subtoolchain, so the
/// chain can be walked without knowing the family: an unfamiliar composite
/// stops being unplannable, which for a site is the difference between a
/// generation it can bump and one it cannot.
fn derive_hierarchy_by_walking(
    parent: &Toolchain,
    cands: &[Candidate],
) -> Option<ToolchainHierarchy> {
    let used_as_toolchain: std::collections::HashSet<&str> = cands
        .iter()
        .map(|candidate| candidate.toolchain.name.as_str())
        .collect();
    let mut members: Vec<Toolchain> = vec![Toolchain {
        name: "system".into(),
        version: String::new(),
    }];
    let mut pending = vec![parent.clone()];
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Breadth-first from the parent down; each level found is inserted before
    // the toolchain that named it, so the result stays lowest-first.
    while let Some(current) = pending.pop() {
        if !seen.insert(format!("{}-{}", current.name, current.version)) {
            continue;
        }
        let Some(definition) = cands
            .iter()
            .find(|c| c.name == current.name && c.version == current.version)
        else {
            continue;
        };
        for dependency in definition
            .dependencies
            .iter()
            .chain(definition.builddependencies.iter())
        {
            if !used_as_toolchain.contains(dependency.name.as_str()) {
                continue;
            }
            let Some(version) = exact_pin_version(&dependency.version_req) else {
                continue;
            };
            let found = Toolchain {
                name: dependency.name.clone(),
                version: version.to_string(),
            };
            if !members.iter().any(|m| toolchains_match(m, &found)) {
                members.push(found.clone());
            }
            pending.push(found);
        }
    }
    if members.len() < 2 {
        return None;
    }
    members.push(parent.clone());
    Some(ToolchainHierarchy {
        parent: parent.clone(),
        members,
    })
}

/// The hierarchy for a toolchain, read from the tree that defines it.
///
/// Each family knows its own shape: the GCC composites reach GCCcore through
/// their GCC pin, the NVIDIA ones walk their own chain, and a compiler-only
/// toolchain states the GCCcore it was built on. Anything else is walked
/// generically.
pub fn derive_hierarchy_from_candidates(
    parent: &Toolchain,
    cands: &[Candidate],
) -> Option<ToolchainHierarchy> {
    const COMPOSITES: [&str; 3] = ["gompi", "gfbf", "foss"];
    if NVIDIA_COMPOSITES.contains(&parent.name.as_str()) {
        // NVHPC-family composite (NVHPC, nvompi, nvofbf): a different chain
        // from the GCC family, walked in its own function.
        //
        // The fallback covers a pre-5.2.0 `NVHPC` generation, whose defining
        // recipe pins GCCcore directly and names no compiler level, so it
        // derives as the compiler-only toolchain it was back then.
        return derive_nvidia_family_hierarchy(parent, cands)
            .or_else(|| derive_compiler_toolchain_hierarchy(parent, cands))
            .or_else(|| derive_hierarchy_by_walking(parent, cands));
    }
    if !COMPOSITES.contains(&parent.name.as_str()) || parent.version.is_empty() {
        // Not a GCC-family composite. A compiler-only toolchain
        // (intel-compilers, nvidia-compilers, rocm-compilers, ...) still has a
        // derivable hierarchy: its own defining recipe pins the GCCcore
        // generation it is built on, so companion recipes on this toolchain
        // resolve their dependencies at [SYSTEM, GCCcore-<gen>, <toolchain>].
        return derive_compiler_toolchain_hierarchy(parent, cands)
            .or_else(|| derive_hierarchy_by_walking(parent, cands));
    }
    // The parent generation's own toolchain-definition recipe.
    let def = cands
        .iter()
        .find(|c| c.name == parent.name && c.version == parent.version)?;
    let Some(gcc_ver) = def
        .dependencies
        .iter()
        .chain(def.builddependencies.iter())
        .find(|d| d.name == "GCC")
        .and_then(|d| exact_pin_version(&d.version_req))
        .map(str::to_string)
    else {
        // A composite that reaches GCC through another composite rather than
        // naming it: foss lists gompi, not GCC.
        return derive_hierarchy_by_walking(parent, cands);
    };
    let mut members = vec![
        Toolchain {
            name: "system".into(),
            version: String::new(),
        },
        Toolchain {
            name: "GCCcore".into(),
            version: gcc_ver.clone(),
        },
        Toolchain {
            name: "GCC".into(),
            version: gcc_ver,
        },
    ];
    for comp in COMPOSITES {
        if comp == parent.name {
            break; // parent itself is appended last (highest rank)
        }
        let defined = cands
            .iter()
            .any(|c| c.name == comp && c.version == parent.version);
        if defined {
            members.push(Toolchain {
                name: comp.into(),
                version: parent.version.clone(),
            });
        }
    }
    members.push(parent.clone());
    Some(ToolchainHierarchy {
        parent: parent.clone(),
        members,
    })
}

/// NVHPC-family composites, lowest rank first, mirroring the GCC family's
/// `gompi < gfbf < foss`. `nvompi` is NVHPC + OpenMPI; `nvofbf` adds FlexiBLAS,
/// FFTW and ScaLAPACK on top.
/// Whether `version` names the same release as `spelled`, allowing `spelled` to
/// carry a versionsuffix the candidate states separately.
///
/// `25.3` answers for `25.3-CUDA-12.8.0`, and `25.11` for itself. The separator
/// requirement is what stops `25.1` from answering for `25.11-CUDA-12.9.1`: a
/// bare `starts_with` accepts it and then reads that release's GCCcore, which
/// silently places the whole hierarchy on the wrong compiler generation.
fn version_prefix_of(version: &str, spelled: &str) -> bool {
    if version.is_empty() || !spelled.starts_with(version) {
        return false;
    }
    let rest = &spelled[version.len()..];
    rest.is_empty() || rest.starts_with('-')
}

/// `NVHPC` is a composite in its own right since EasyBuild 5.2.0:
/// `NVHPC(NvidiaCompilersToolchain, NVHPCX, NVBLAS, NVScaLAPACK)`, i.e. the
/// compilers plus the SDK's own MPI and math libraries. A recipe whose
/// toolchain is `NVHPC-<ver>` therefore has a real hierarchy to derive, and it
/// is ordered below nvompi and nvofbf, which compose on top of it.
const NVIDIA_COMPOSITES: [&str; 3] = ["NVHPC", "nvompi", "nvofbf"];

/// Derive an NVHPC-family generation hierarchy (`nvompi-<gen>`, `nvofbf-<gen>`).
///
/// The chain differs from the GCC family in shape, so it cannot reuse
/// [`derive_hierarchy_from_candidates`]'s GCC walk. EasyBuild composes it as
/// `Nvompi.SUBTOOLCHAIN = ['NVHPC', 'nvidia-compilers']` and
/// `NvidiaCompilersToolchain.SUBTOOLCHAIN = [GCCcore.NAME, SYSTEM_TOOLCHAIN_NAME]`
/// (framework `easybuild/toolchains/nvompi.py` and `nvidia_compilers.py`), giving
/// `system < GCCcore < nvidia-compilers < NVHPC < nvompi < nvofbf`.
///
/// GCCcore matters here rather than being cosmetic: without that level a recipe
/// on `nvofbf` cannot see `Szip-2.1.1-GCCcore-<gen>` or
/// `cryptography-<v>-GCCcore-<gen>`, which are exactly the dependencies real
/// NVHPC-toolchain recipes reach for, and the solve fails as unsatisfiable.
///
/// The walk is three hops through the tree, each pinned by a real recipe:
/// the composite definition pins `NVHPC`, whose recipe pins `nvidia-compilers`,
/// whose recipe pins `GCCcore`. Returns `None` when the composite has no
/// defining recipe in the tree or that recipe pins no NVHPC.
fn derive_nvidia_family_hierarchy(
    parent: &Toolchain,
    cands: &[Candidate],
) -> Option<ToolchainHierarchy> {
    if parent.version.is_empty() {
        return None;
    }
    // A dependent recipe spells its toolchain with version and versionsuffix
    // joined (`NVHPC-25.11-CUDA-12.9.1`), while the defining recipe splits them
    // into version `25.11` plus versionsuffix `-CUDA-12.9.1`. Match either, or
    // a versionsuffixed parent never finds its own definition.
    let joined = |c: &Candidate| {
        format!(
            "{}{}",
            c.version,
            c.versionsuffix.as_deref().unwrap_or_default()
        )
    };
    // The last fallback carries the weight in practice: NVHPC and
    // nvidia-compilers spell their versionsuffix as a template
    // (`-CUDA-%(cudaver)s`), which textual parsing cannot expand, so the joined
    // form reads `25.11-CUDA-%(cudaver)s` and never equals `25.11-CUDA-12.9.1`.
    // Match on the version prefix instead, requiring the remainder to begin a
    // versionsuffix so that `25.1` does not answer for `25.11-CUDA-12.9.1`.
    let suffixed_prefix = |c: &Candidate| version_prefix_of(&c.version, &parent.version);
    let def = cands
        .iter()
        .find(|c| c.name == parent.name && c.version == parent.version)
        .or_else(|| {
            cands
                .iter()
                .find(|c| c.name == parent.name && joined(c) == parent.version)
        })
        .or_else(|| {
            cands
                .iter()
                .find(|c| c.name == parent.name && suffixed_prefix(c))
        })?;
    // The composite pins its compiler as a whole toolchain string, e.g.
    // ('NVHPC', '25.3-CUDA-12.8.0'): version and versionsuffix already joined,
    // which is precisely the form a dependent recipe's `toolchain` uses.
    // The compiler level has two spellings. `nvidia-compilers` is the current
    // one: EasyBuild 5.2.0 replaced NVHPCToolchain with NvidiaCompilersToolchain,
    // so a composite built against a current framework pins that. `NVHPC` is the
    // deprecated spelling, still what older generations in the wild carry. Accept
    // either, preferring the current one, or a correctly-built new generation
    // reads as an unknown hierarchy.
    let nvhpc_ver = ["nvidia-compilers", "NVHPC"]
        .iter()
        .find_map(|compiler| {
            def.dependencies
                .iter()
                .chain(def.builddependencies.iter())
                .find(|d| d.name == *compiler)
                .and_then(|d| exact_pin_version(&d.version_req))
        })?
        .to_string();

    let mut members = vec![Toolchain {
        name: "system".into(),
        version: String::new(),
    }];
    if let Some(gcccore_ver) = nvidia_compilers_gcccore(&nvhpc_ver, cands) {
        members.push(Toolchain {
            name: "GCCcore".into(),
            version: gcccore_ver,
        });
    }
    // Both spellings of the compiler level are members: older recipes in the
    // wild use `NVHPC` as their toolchain, while `nvidia-compilers` is the
    // framework's own name for the same level.
    members.push(Toolchain {
        name: "nvidia-compilers".into(),
        version: nvhpc_ver.clone(),
    });
    // When NVHPC is itself the parent it is appended last, at its own version,
    // and must not also appear here at the compiler version.
    if parent.name != "NVHPC" {
        members.push(Toolchain {
            name: "NVHPC".into(),
            version: nvhpc_ver,
        });
    }
    for comp in NVIDIA_COMPOSITES {
        if comp == parent.name {
            break; // parent itself is appended last (highest rank)
        }
        if cands
            .iter()
            .any(|c| c.name == comp && c.version == parent.version)
        {
            members.push(Toolchain {
                name: comp.into(),
                version: parent.version.clone(),
            });
        }
    }
    members.push(parent.clone());
    Some(ToolchainHierarchy {
        parent: parent.clone(),
        members,
    })
}

/// GCCcore generation underneath an NVHPC toolchain string, via the
/// `nvidia-compilers` recipe that NVHPC pins.
///
/// `nvhpc_ver` is a joined toolchain string (`25.3-CUDA-12.8.0`), while the
/// `nvidia-compilers` recipe splits the same identity into `version` (`25.3`)
/// plus `versionsuffix` (`-CUDA-12.8.0`). Prefer the variant whose joined form
/// matches exactly; fall back to a version-prefix match so a CUDA-less
/// `nvidia-compilers-25.3` still answers for `25.3-CUDA-12.8.0` (both pin the
/// same GCCcore in practice).
fn nvidia_compilers_gcccore(nvhpc_ver: &str, cands: &[Candidate]) -> Option<String> {
    let is_nvc = |c: &Candidate| c.name == "nvidia-compilers";
    let joined = |c: &Candidate| {
        format!(
            "{}{}",
            c.version,
            c.versionsuffix.as_deref().unwrap_or_default()
        )
    };
    let chosen = cands
        .iter()
        .find(|c| is_nvc(c) && joined(c) == nvhpc_ver)
        .or_else(|| {
            cands
                .iter()
                .find(|c| is_nvc(c) && version_prefix_of(&c.version, nvhpc_ver))
        })?;
    chosen
        .dependencies
        .iter()
        .chain(chosen.builddependencies.iter())
        .find(|d| d.name == "GCCcore")
        .and_then(|d| exact_pin_version(&d.version_req))
        .map(|s| s.to_string())
}

/// Derive the hierarchy for a compiler-only toolchain (intel-compilers,
/// nvidia-compilers, rocm-compilers, ...) from the tree.
///
/// These sit directly on a GCCcore generation rather than composing gompi/gfbf.
/// The toolchain's own defining recipe (`name == parent.name`, `version ==
/// parent.version`) pins that GCCcore version as a dependency, so a companion
/// recipe built on the toolchain (e.g. `OpenMPI-<v>-nvidia-compilers-25.11`)
/// resolves its dependency versions against the GCCcore generation. The
/// hierarchy is `[SYSTEM, GCCcore-<gen>, <toolchain>]`, mirroring how
/// [`known_hierarchy`] treats a bare `GCCcore` parent. Returns `None` when the
/// toolchain has no defining recipe in the tree or that recipe pins no GCCcore.
///
/// A toolchain used with a versionsuffix is spelled two ways: a dependent
/// recipe writes one joined string (`NVHPC-25.3-CUDA-12.8.0`), while the
/// defining recipe splits it into `version` (`25.3`) plus `versionsuffix`
/// (`-CUDA-12.8.0`). Both spellings resolve here, so a recipe on such a
/// toolchain is not reported as an unknown generation.
fn derive_compiler_toolchain_hierarchy(
    parent: &Toolchain,
    cands: &[Candidate],
) -> Option<ToolchainHierarchy> {
    if parent.version.is_empty() || is_system_toolchain(parent) {
        return None;
    }
    let def = cands
        .iter()
        .find(|c| c.name == parent.name && c.version == parent.version)
        .or_else(|| {
            cands.iter().find(|c| {
                c.name == parent.name
                    && format!(
                        "{}{}",
                        c.version,
                        c.versionsuffix.as_deref().unwrap_or_default()
                    ) == parent.version
            })
        })?;
    let gcccore_ver = def
        .dependencies
        .iter()
        .chain(def.builddependencies.iter())
        .find(|d| d.name == "GCCcore")
        .and_then(|d| exact_pin_version(&d.version_req))
        .map(|s| s.to_string())
        // NVHPC pins CUDA and nvidia-compilers rather than GCCcore itself, so
        // the GCCcore generation sits one hop further down.
        .or_else(|| nvidia_compilers_gcccore(&parent.version, cands))?;
    Some(ToolchainHierarchy {
        parent: parent.clone(),
        members: vec![
            Toolchain {
                name: "system".into(),
                version: String::new(),
            },
            Toolchain {
                name: "GCCcore".into(),
                version: gcccore_ver,
            },
            parent.clone(),
        ],
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

/// Like [`hierarchy_for`], with the parsed easyconfig universe as a final
/// fallback: fixture path, else built-in, else derived from the tree
/// ([`derive_hierarchy_from_candidates`]).
pub fn hierarchy_for_with_tree(
    parent: &Toolchain,
    fixture_path: Option<&Path>,
    cands: &[Candidate],
) -> Result<ToolchainHierarchy, HierarchyError> {
    // The system toolchain is the bottom of every hierarchy and has nothing
    // under it, so it is its own. Without this a site cannot plan the recipes
    // that sit there at all, and those are the compilers: GCCcore,
    // intel-compilers and nvidia-compilers are all built at SYSTEM.
    if is_system_toolchain(parent) {
        return Ok(ToolchainHierarchy {
            parent: parent.clone(),
            members: vec![parent.clone()],
        });
    }
    match hierarchy_for(parent, fixture_path) {
        Ok(h) => Ok(h),
        Err(HierarchyError::UnknownToolchain(..)) if fixture_path.is_none() => {
            derive_hierarchy_from_candidates(parent, cands).ok_or_else(|| {
                HierarchyError::UnknownToolchain(parent.name.clone(), parent.version.clone())
            })
        }
        Err(e) => Err(e),
    }
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
/// Among hierarchy members for name, pick a safe version for the target
/// generation.
///
/// Selection rules, in order: (1) candidate must be in the hierarchy and match
/// name (strict name+version membership; out-of-generation GCCcore/GCC never
/// qualify); (2) optional versionsuffix must match (empty/None matches
/// candidates with no suffix); (3) optional floor_version never picks a version
/// older than the floor; (4) prefer candidates on the hierarchy parent
/// toolchain when present, otherwise the highest hierarchy member that has a
/// candidate; (5) among the same hierarchy rank (or when use_consensus / floor
/// is set), use generation consensus when available: if a version has a clear
/// majority (at least 80 percent of pins) and is eligible, pick it, else the
/// newest eligible version that has at least one generation pin (else newest
/// eligible). Without floor and without consensus, pick the newest version by
/// cmp_version. Returns None when no candidate satisfies the filters.
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
        for dep in consumer
            .dependencies
            .iter()
            .chain(consumer.builddependencies.iter())
        {
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
/// Clear majority (at least 80 percent of generation pins of name that land in
/// eligible) yields that modal version. Else pick the newest among eligible
/// versions that appear in the pin counts (weak signal / no unique consensus).
/// If no pin counts hit eligible, pick the newest eligible (true no-signal
/// fallback).
///
/// eligible must already be floor/suffix/hierarchy filtered; empty yields None.
/// Callers that hold full Candidate values should first run
/// prefer_non_system_candidates so SYSTEM does not beat GCCcore on version alone.
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
        return eligible.iter().max_by(|a, b| cmp_version(a, b)).cloned();
    }
    let total: usize = filtered.iter().map(|(_, c)| c).sum();
    // Modal: highest count; ties broken by newer version.
    filtered.sort_by(|(va, ca), (vb, cb)| cb.cmp(ca).then_with(|| cmp_version(va, vb)));
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
            // The floor (source recipe's dep version) guards against accidental
            // downgrades on a forward generation bump. It must not exclude a
            // candidate on the target hierarchy's own parent toolchain: that
            // version is the generation-authoritative pick, not a downgrade,
            // even when a newer-generation source is retargeted onto an older
            // generation (e.g. binutils-2.42-GCCcore-13.3.0 resolved from a
            // GCCcore-15.2.0 source whose binutils floor is 2.45).
            let is_parent_toolchain = toolchains_match(&hierarchy.parent, &c.toolchain);
            if !is_parent_toolchain && cmp_version(&c.version, floor) == Ordering::Less {
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
                let hint = nearest_candidates_hint(&name, cands);
                return Err(HierarchyError::MissingDep(
                    name,
                    hierarchy.parent.name.clone(),
                    hierarchy.parent.version.clone(),
                    hint,
                ));
            }
        }
    }
    Ok(out)
}

/// One dependency as scraped from a source recipe (name + version + optional suffix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDepSpec {
    /// Dependency name.
    pub name: String,
    /// Version required.
    pub version: String,
    /// Versionsuffix required, when the entry names one.
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

    /// Whether package bump planning must leave the source version untouched:
    /// SYSTEM 4th-tuple or non-empty versionsuffix.
    ///
    /// A `# optional` comment marks the dependency optional **to include**,
    /// not frozen. Optional deps still bump when hierarchy candidates exist
    /// (soft-keep the source pin only if unresolved — no hard ERROR).
    pub fn is_preserve_pin(&self) -> bool {
        self.system_toolchain
            || self
                .versionsuffix
                .as_deref()
                .is_some_and(|vs| !vs.is_empty())
    }
}

/// Resolve many source dependency specifications with floor and versionsuffix safety.
///
/// - Deps with a **non-empty versionsuffix** are not bumped (caller keeps source).
/// - Deps with **SYSTEM** 4th-tuple toolchain are not bumped (keep source pin).
/// - Deps marked **optional** (`# optional` on that line) resolve/bump like any
///   other dep when hierarchy candidates exist (`# optional` = optional *to
///   include*, not frozen). If unresolved they soft-keep the source pin (no
///   hard ERROR). The emitter preserves the comment text; only the version
///   token changes.
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
        // `# optional` marks the dep optional-to-include, not frozen: it
        // resolves/bumps like any other dep below (comment text is preserved
        // by the emitter regardless of whether the version changes).
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
                    let hint = nearest_candidates_hint(&spec.name, cands);
                    return Err(HierarchyError::MissingDep(
                        spec.name.clone(),
                        hierarchy.parent.name.clone(),
                        hierarchy.parent.version.clone(),
                        hint,
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
    fn known_foss_2026_1_includes_required_levels() {
        // An embedded generation lets `plan` resolve dependency versions when
        // the supplied robot lacks the matching toolchain-definition recipe.
        let h = known_hierarchy(&foss("2026.1")).expect("embedded foss-2026.1");
        let labels = h.member_labels();
        for need in [
            "system",
            "GCCcore-15.2.0",
            "GCC-15.2.0",
            "gfbf-2026.1",
            "gompi-2026.1",
            "foss-2026.1",
        ] {
            assert!(
                labels.iter().any(|l| l == need),
                "missing {need} in {labels:?}"
            );
        }
        assert_eq!(h.parent, foss("2026.1"));
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
        let cands = parse_easyconfig_tree(&root)
            .expect("parse universe")
            .candidates;
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
            moduleclass: None,
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

    /// The real nvofbf-2025.10 chain as the trees actually spell it:
    /// nvofbf pins NVHPC as one joined toolchain string, NVHPC pins
    /// nvidia-compilers split into version + versionsuffix, and
    /// nvidia-compilers pins GCCcore.
    fn nvidia_family_tree() -> Vec<Candidate> {
        let mut nvofbf = cand("nvofbf", "2025.10", "system", "", None);
        nvofbf.dependencies = vec![
            dep_pin("NVHPC", "25.3-CUDA-12.8.0"),
            dep_pin("OpenMPI", "5.0.7"),
            dep_pin("FlexiBLAS", "3.4.5"),
        ];
        let mut nvompi = cand("nvompi", "2025.10", "system", "", None);
        nvompi.dependencies = vec![dep_pin("NVHPC", "25.3-CUDA-12.8.0")];
        let mut nvhpc = cand("NVHPC", "25.3", "system", "", Some("-CUDA-12.8.0"));
        nvhpc.dependencies = vec![
            dep_pin("CUDA", "12.8.0"),
            dep_pin("nvidia-compilers", "25.3"),
        ];
        let mut nvc = cand(
            "nvidia-compilers",
            "25.3",
            "system",
            "",
            Some("-CUDA-12.8.0"),
        );
        nvc.dependencies = vec![dep_pin("GCCcore", "14.2.0")];
        vec![nvofbf, nvompi, nvhpc, nvc]
    }

    #[test]
    fn nvofbf_hierarchy_derives_from_tree_without_a_fixture() {
        let parent = Toolchain {
            name: "nvofbf".into(),
            version: "2025.10".into(),
        };
        let h = derive_hierarchy_from_candidates(&parent, &nvidia_family_tree())
            .expect("nvofbf-2025.10 should derive from the tree");
        let names: Vec<&str> = h.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "system",
                "GCCcore",
                "nvidia-compilers",
                "NVHPC",
                "nvompi",
                "nvofbf"
            ]
        );
        assert_eq!(h.parent, parent);
    }

    /// EasyBuild 5.2.0 replaced NVHPCToolchain with NvidiaCompilersToolchain, so
    /// a composite toolchain built against a current framework pins
    /// `nvidia-compilers` where an older one pinned `NVHPC`. Deriving only from
    /// the deprecated spelling makes every correctly-built new generation an
    /// unknown hierarchy, which downgrades its dependency matching to a
    /// toolchain-blind one.
    #[test]
    fn nvofbf_hierarchy_derives_from_the_nvidia_compilers_spelling() {
        let mut tree = nvidia_family_tree();
        for tc in tree.iter_mut() {
            if tc.name == "nvofbf" || tc.name == "nvompi" {
                // Same generation, built the way EasyBuild 5.2.0 onwards wants:
                // the composite pins the compiler-only module, not the SDK.
                for dep in tc.dependencies.iter_mut() {
                    if dep.name == "NVHPC" {
                        dep.name = "nvidia-compilers".into();
                    }
                }
            }
        }
        let parent = Toolchain {
            name: "nvofbf".into(),
            version: "2025.10".into(),
        };
        let h = derive_hierarchy_from_candidates(&parent, &tree)
            .expect("a composite pinning nvidia-compilers must still derive");
        let names: Vec<&str> = h.members.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"GCCcore"), "got {names:?}");
        assert!(names.contains(&"nvidia-compilers"), "got {names:?}");
        assert_eq!(names.last(), Some(&"nvofbf"));
    }

    /// GCCcore is the level that lets a recipe on nvofbf reach
    /// Szip-GCCcore-14.2.0 and cryptography-GCCcore-14.2.0. Without it the
    /// solve is unsatisfiable, so its exact version is load-bearing.
    #[test]
    fn nvofbf_hierarchy_pins_gcccore_through_nvidia_compilers() {
        let parent = Toolchain {
            name: "nvofbf".into(),
            version: "2025.10".into(),
        };
        let h = derive_hierarchy_from_candidates(&parent, &nvidia_family_tree()).expect("derive");
        let gcccore = h
            .members
            .iter()
            .find(|m| m.name == "GCCcore")
            .expect("GCCcore member");
        assert_eq!(gcccore.version, "14.2.0");
        // The compiler level carries the joined toolchain string, which is what a
        // dependent recipe's own `toolchain` field spells.
        let nvhpc = h
            .members
            .iter()
            .find(|m| m.name == "NVHPC")
            .expect("NVHPC member");
        assert_eq!(nvhpc.version, "25.3-CUDA-12.8.0");
    }

    #[test]
    fn nvompi_hierarchy_stops_below_nvofbf() {
        let parent = Toolchain {
            name: "nvompi".into(),
            version: "2025.10".into(),
        };
        let h = derive_hierarchy_from_candidates(&parent, &nvidia_family_tree()).expect("derive");
        let names: Vec<&str> = h.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["system", "GCCcore", "nvidia-compilers", "NVHPC", "nvompi"]
        );
    }

    /// A version prefix only answers at a versionsuffix boundary. Without that
    /// rule `25.1` matches `25.11-CUDA-12.9.1`, and the hierarchy silently
    /// takes GCCcore from the wrong release: checking a real NVHPC-25.11 recipe
    /// put the tree on GCCcore-13.3.0 instead of 14.2.0, which then lost
    /// cryptography-44.0.2.
    #[test]
    fn a_shorter_release_does_not_answer_for_a_longer_one() {
        assert!(version_prefix_of("25.3", "25.3-CUDA-12.8.0"));
        assert!(version_prefix_of("25.11", "25.11"));
        assert!(version_prefix_of("25.11", "25.11-CUDA-12.9.1"));
        assert!(!version_prefix_of("25.1", "25.11-CUDA-12.9.1"));
        assert!(!version_prefix_of("25.1", "25.11"));
        assert!(!version_prefix_of("", "25.11"));

        let mut older = cand("nvidia-compilers", "25.1", "system", "", None);
        older.dependencies = vec![dep_pin("GCCcore", "13.3.0")];
        let mut newer = cand("nvidia-compilers", "25.11", "system", "", None);
        newer.dependencies = vec![dep_pin("GCCcore", "14.2.0")];
        assert_eq!(
            nvidia_compilers_gcccore("25.11", &[older, newer]),
            Some("14.2.0".to_string())
        );
    }

    /// A CUDA-less nvidia-compilers still answers for a CUDA-suffixed NVHPC pin;
    /// both variants pin the same GCCcore in the real tree.
    #[test]
    fn nvidia_compilers_gcccore_falls_back_to_version_prefix() {
        let mut nvc = cand("nvidia-compilers", "25.3", "system", "", None);
        nvc.dependencies = vec![dep_pin("GCCcore", "14.2.0")];
        assert_eq!(
            nvidia_compilers_gcccore("25.3-CUDA-12.8.0", &[nvc]),
            Some("14.2.0".to_string())
        );
    }

    /// FlexiBLAS-3.4.5-NVHPC-25.3-CUDA-12.8.0.eb writes its toolchain as one
    /// joined string, while NVHPC's own recipe splits it. Both must resolve.
    ///
    /// `nvidia-compilers` is a member, not noise: since EasyBuild 5.2.0 NVHPC
    /// composes the compilers with the SDK's NVHPCX, NVBLAS and NVScaLAPACK, so
    /// a recipe on NVHPC can legitimately reach a dependency built on the
    /// compiler-only level below it.
    #[test]
    fn versionsuffixed_compiler_toolchain_resolves_from_joined_spelling() {
        let parent = Toolchain {
            name: "NVHPC".into(),
            version: "25.3-CUDA-12.8.0".into(),
        };
        let h = derive_hierarchy_from_candidates(&parent, &nvidia_family_tree())
            .expect("NVHPC-25.3-CUDA-12.8.0 should resolve from the split recipe");
        let names: Vec<&str> = h.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["system", "GCCcore", "nvidia-compilers", "NVHPC"]
        );
        assert_eq!(
            h.members
                .iter()
                .find(|m| m.name == "GCCcore")
                .map(|m| m.version.as_str()),
            Some("14.2.0")
        );
        // The parent ranks last, at its own version, and appears exactly once.
        assert_eq!(
            h.members.last().map(|m| m.version.as_str()),
            Some(parent.version.as_str())
        );
        assert_eq!(h.members.iter().filter(|m| m.name == "NVHPC").count(), 1);
    }

    #[test]
    fn nvofbf_without_a_defining_recipe_does_not_derive() {
        let parent = Toolchain {
            name: "nvofbf".into(),
            version: "2099.99".into(),
        };
        assert!(derive_hierarchy_from_candidates(&parent, &nvidia_family_tree()).is_none());
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
        assert!(resolve_dep_version_in_hierarchy_opts("Cython", &only_old, &h, &opts).is_none());
    }

    #[test]
    fn derive_hierarchy_from_tree_for_unknown_generation() {
        // A generation with no shipped fixture (foss-2026.1 graduated to a
        // known fixture; 2099a stands in for the next brand-new generation)
        // must derive its hierarchy from the robot tree: the foss definition
        // pins GCC 15.2.0, and gompi/gfbf definitions exist as recipes.
        let parent = Toolchain {
            name: "foss".into(),
            version: "2099a".into(),
        };
        let mut foss_def = cand("foss", "2099a", "system", "", None);
        foss_def.dependencies = vec![dep_pin("GCC", "15.2.0")];
        let cands = vec![
            foss_def,
            cand("gompi", "2099a", "system", "", None),
            cand("gfbf", "2099a", "system", "", None),
            cand("binutils", "2.42", "GCCcore", "15.2.0", None),
        ];
        let h = derive_hierarchy_from_candidates(&parent, &cands).expect("derived");
        assert_eq!(
            h.member_labels(),
            vec![
                "system",
                "GCCcore-15.2.0",
                "GCC-15.2.0",
                "gompi-2099a",
                "gfbf-2099a",
                "foss-2099a",
            ]
        );
        // hierarchy_for_with_tree falls back to derivation for unknown gens...
        let via_tree = hierarchy_for_with_tree(&parent, None, &cands).expect("with tree");
        assert_eq!(via_tree, h);
        // ...but a known generation still uses the shipped fixture.
        let known = hierarchy_for_with_tree(&foss("2024a"), None, &cands).expect("known");
        assert_eq!(known, known_hierarchy(&foss("2024a")).unwrap());
        // foss-2026.1 is now a shipped fixture (no tree derivation needed).
        assert!(known_hierarchy(&foss("2026.1")).is_some());
        // Missing composite definitions are simply omitted from members.
        let mut foss_only = cand("foss", "2099a", "system", "", None);
        foss_only.dependencies = vec![dep_pin("GCC", "15.2.0")];
        let sparse = vec![foss_only];
        let h2 = derive_hierarchy_from_candidates(&parent, &sparse).expect("derived sparse");
        assert_eq!(
            h2.member_labels(),
            vec!["system", "GCCcore-15.2.0", "GCC-15.2.0", "foss-2099a"]
        );
        // A compiler-only toolchain with no defining recipe in the tree is not
        // derivable (nothing pins its GCCcore generation).
        let intel = Toolchain {
            name: "intel".into(),
            version: "2026a".into(),
        };
        assert!(derive_hierarchy_from_candidates(&intel, &cands).is_none());
    }

    #[test]
    fn compiler_only_toolchain_hierarchy_derived_from_its_gcccore_dep() {
        // nvidia-compilers-25.11 sits directly on GCCcore-14.3.0 (declared in
        // its own recipe). A companion recipe on this toolchain must resolve
        // dependencies at [SYSTEM, GCCcore-14.3.0, nvidia-compilers-25.11].
        let mut def = cand("nvidia-compilers", "25.11", "system", "", None);
        def.dependencies = vec![dep_pin("GCCcore", "14.3.0"), dep_pin("binutils", "2.44")];
        let parent = Toolchain {
            name: "nvidia-compilers".into(),
            version: "25.11".into(),
        };
        let h = derive_hierarchy_from_candidates(&parent, &[def.clone()]).expect("derived");
        assert_eq!(
            h.member_labels(),
            vec!["system", "GCCcore-14.3.0", "nvidia-compilers-25.11"]
        );
        // A GCCcore-14.3.0 dependency candidate is in-hierarchy; an intel-gen
        // one on a different GCCcore is not.
        let ucx_143 = cand("UCX", "1.19.0", "GCCcore", "14.3.0", None);
        let ucx_133 = cand("UCX", "1.18.0", "GCCcore", "13.3.0", None);
        assert!(h.contains(&ucx_143.toolchain));
        assert!(!h.contains(&ucx_133.toolchain));
        // No defining recipe -> not derivable.
        assert!(derive_hierarchy_from_candidates(&parent, &[]).is_none());
    }

    #[test]
    fn missing_dep_error_carries_nearest_generation_hint() {
        // A miss must read as a work item: name the generations that DO have
        // the package so the operator knows what to bump from.
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![cand("xtb", "6.7.1", "gfbf", "2023b", None)];
        let err = resolve_dep_versions_in_hierarchy_strict(["xtb"], &cands, &h).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("available at other generations") && msg.contains("6.7.1 @ gfbf-2023b"),
            "got {msg}"
        );
    }

    #[test]
    fn floor_does_not_exclude_parent_generation_on_backward_retarget() {
        // Retargeting a GCCcore-15.2.0 companion onto GCCcore-13.3.0: the source
        // binutils floor (2.45) must not exclude the generation-authoritative
        // binutils-2.42-GCCcore-13.3.0 (the hierarchy parent), even though a
        // higher SYSTEM binutils (2.46.1) is present. Without the parent-toolchain
        // exemption the floor snaps to the SYSTEM 2.46.1 -- the wrong binutils for
        // a GCCcore-13.3.0 build.
        let gcccore = Toolchain {
            name: "GCCcore".into(),
            version: "13.3.0".into(),
        };
        let h = known_hierarchy(&gcccore).expect("built-in GCCcore hierarchy");
        let cands = vec![
            cand("binutils", "2.42", "GCCcore", "13.3.0", None),
            cand("binutils", "2.46.1", "system", "", None),
        ];
        let opts = ResolveDepOpts {
            floor_version: Some("2.45"),
            versionsuffix: None,
            use_consensus: false,
        };
        assert_eq!(
            resolve_dep_version_in_hierarchy_opts("binutils", &cands, &h, &opts).as_deref(),
            Some("2.42"),
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
        assert!(
            !map.contains_key("LLVM"),
            "suffix-pinned LLVM must not bump"
        );
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
            matches!(err, HierarchyError::MissingDep(ref n, _, _, _) if n == "MissingPkg"),
            "{err}"
        );
        let (map, kept) = resolve_dep_versions_for_specs(&specs, &cands, &h, true).unwrap();
        assert!(map.is_empty());
        assert!(kept
            .iter()
            .any(|k| k.contains("MissingPkg") && k.contains("keeping")));
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
        cands.push(consumer_pinning(
            "NewApp", "1.0", "foss", "2024a", "CMake", "3.31.8",
        ));
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
        sk.push(consumer_pinning(
            "SciNew",
            "1.0",
            "gfbf",
            "2024a",
            "scikit-learn",
            "1.6.1",
        ));
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
        assert!(
            !counts.contains_key("3.31.8"),
            "out-of-gen pins must not count: {counts:?}"
        );
    }

    #[test]
    fn resolve_specs_preserve_system_pin_but_bump_optional() {
        let h = known_hierarchy(&foss("2024a")).unwrap();
        let cands = vec![
            cand("Python", "3.12.3", "GCCcore", "13.3.0", None),
            // In-hierarchy newer ASE — optional bumps like a normal dep.
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
            "optional dep must resolve to the in-hierarchy generation version: {map:?}"
        );
        assert_eq!(map.get("networkx").map(String::as_str), Some("3.4.2"));
        assert_eq!(map.get("PyTables").map(String::as_str), Some("3.10.2"));
        assert_eq!(map.get("Python").map(String::as_str), Some("3.12.3"));
        assert!(kept
            .iter()
            .any(|k| k.contains("USEARCH") && k.contains("SYSTEM")));
        assert!(
            !kept.iter().any(|k| k.contains("ASE")),
            "optional dep with a candidate must not be listed as kept-old: {kept:?}"
        );
        assert!(SourceDepSpec {
            name: "USEARCH".into(),
            version: "11.0.667-i86linux32".into(),
            versionsuffix: None,
            system_toolchain: true,
            optional: false,
        }
        .is_preserve_pin());
        assert!(
            !SourceDepSpec {
                name: "ASE".into(),
                version: "3.23.0".into(),
                versionsuffix: None,
                system_toolchain: false,
                optional: true,
            }
            .is_preserve_pin(),
            "optional alone must not count as a pin"
        );
        // Missing non-optional still errors.
        let bad = vec![SourceDepSpec::plain("MissingPkg", "1.0")];
        assert!(resolve_dep_versions_for_specs(&bad, &cands, &h, false).is_err());
        // Missing optional soft-keeps (no hard ERROR); keep_old path also soft-keeps.
        let opt_missing = vec![SourceDepSpec {
            name: "OptionalGhost".into(),
            version: "0.1".into(),
            versionsuffix: None,
            system_toolchain: false,
            optional: true,
        }];
        let (m2, k2) = resolve_dep_versions_for_specs(&opt_missing, &cands, &h, false).unwrap();
        assert!(m2.is_empty());
        assert!(
            k2.iter()
                .any(|k| k.contains("OptionalGhost") && k.contains("optional")),
            "kept notes: {k2:?}"
        );
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
        assert_eq!(
            hierarchy_member_rank(&h, &sys_empty),
            hierarchy_member_rank(&h, &sys_sys)
        );
        assert!(
            hierarchy_member_rank(&h, &gcc).unwrap() > hierarchy_member_rank(&h, &sys_sys).unwrap()
        );
        assert!(hierarchy_member_rank(&h, &gcc14).is_none());
    }
}
