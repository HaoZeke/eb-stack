//! Domain types for EasyBuild stack selection.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// An EasyBuild toolchain: the compiler and library generation a build targets.
pub struct Toolchain {
    /// Toolchain name as EasyBuild spells it, e.g. `foss`, `GCCcore`. The
    /// EasyBuild `SYSTEM` toolchain appears here as `system`.
    pub name: String,
    /// Generation string, e.g. `2026.1`. `system` for the system toolchain.
    pub version: String,
}

impl Toolchain {
    /// `name-version`, the form used in easyconfig filenames and messages.
    pub fn label(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One dependency an easyconfig declares, after template resolution.
pub struct DepReq {
    /// Package name required.
    pub name: String,
    /// Version field as written: an exact version, or a range such as `>=1.2`.
    /// Not normalised, because the easyconfig's own spelling is what a
    /// maintainer will look for.
    pub version_req: String,
    /// Optional versionsuffix on this dependency (e.g. `-CUDA-%(cudaver)s` after resolve).
    /// When set, selection treats it as part of the requirement identity.
    #[serde(default)]
    pub versionsuffix: Option<String>,
    /// Per-dependency toolchain override (`None` = inherit the dependent's toolchain).
    /// Includes EasyBuild `SYSTEM` → `{name: "system", version: "system"}`.
    #[serde(default)]
    pub toolchain: Option<Toolchain>,
}

/// One bundled extension entry from an easyconfig `exts_list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtEntry {
    /// Extension name as listed in `exts_list`.
    pub name: String,
    /// Extension version. Empty when the entry gave none.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One installable variant the solver may choose: a parsed easyconfig.
///
/// Identity is name, version, toolchain and versionsuffix together; two
/// easyconfigs differing in any of those are separate candidates.
pub struct Candidate {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// Toolchain this variant builds against.
    pub toolchain: Toolchain,
    /// Suffix distinguishing variants of one version, e.g. `-CUDA-12.6.0`.
    /// `None` when the easyconfig sets none.
    #[serde(default)]
    pub versionsuffix: Option<String>,
    /// Path the candidate was parsed from. Empty for an in-memory parse.
    pub easyconfig_path: String,
    /// Runtime requirements, which must also be installed.
    #[serde(default)]
    pub dependencies: Vec<DepReq>,
    /// Build-time-only requirements (`builddependencies` in the easyconfig).
    /// Same `DepReq` semantics as runtime `dependencies`; kept separate so
    /// lock/SBOM/serialized outputs can distinguish build vs runtime roles.
    #[serde(default)]
    pub builddependencies: Vec<DepReq>,
    /// Bundled extensions (`exts_list`) resolved from the easyconfig.
    #[serde(default)]
    pub exts_list: Vec<ExtEntry>,
    /// What the recipe says the module is for. Carried so a regenerated or
    /// retargeted recipe can keep the class the tree already gives a package,
    /// which upstream metadata cannot tell you: archspec and cppy state no
    /// topic at all and upstream classes both as `tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moduleclass: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// The candidate set a solve draws from, scoped to one toolchain generation.
pub struct Universe {
    /// Toolchain the solve targets.
    pub toolchain: Toolchain,
    /// Human label for the generation, when the caller supplied one. Carried
    /// into the lock for provenance; it does not affect selection.
    #[serde(default)]
    pub generation_label: Option<String>,
    /// Every variant available to choose from.
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A constraint fixing one package to a version or range.
pub struct Pin {
    /// Package the pin applies to.
    pub name: String,
    /// Version requirement the selection must satisfy, same spelling rules as
    /// [`DepReq::version_req`].
    pub version_req: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A demand that a package move forward relative to the baseline lock.
pub struct RequireUpgrade {
    /// Package that must advance.
    pub name: String,
    /// When true, the selected version of `name` must be strictly newer than
    /// the baseline lock's version. When false, construction fails with a
    /// clear error (absolute require_upgrade is not silently ignored).
    #[serde(default)]
    pub relative_to_baseline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// What a solve is asked to produce: target toolchain, roots, and constraints.
pub struct Policy {
    /// Toolchain generation to select for.
    pub toolchain: Toolchain,
    /// Application roots the stack exists to provide. Everything else is
    /// pulled in only because a root needs it.
    pub roots: Vec<String>,
    /// Declared priority order over application roots for multi-root
    /// lexicographic newest selection. When omitted or empty, defaults to
    /// [`Self::roots`] list order. Explicit priority is independent of
    /// reordering `roots` in the policy JSON.
    #[serde(default)]
    pub root_priority: Option<Vec<String>>,
    /// Keep what the baseline already installed when nothing requires moving.
    ///
    /// The default objective is newest-wins, which is right for planning a new
    /// generation and wrong for maintaining one: on a site where a rebuild
    /// costs hours of a GPU partition, a solve that moves a package nobody
    /// asked to move spends that time for nothing. With this set, a package
    /// present in the baseline lock is preferred at the version it already
    /// has, unless a pin, an exclusion or a require_upgrade says otherwise.
    /// Those all remain hard constraints; this only decides between candidates
    /// that were all valid anyway.
    #[serde(default)]
    pub prefer_installed: bool,
    /// Version constraints applied on top of what the candidates allow.
    #[serde(default)]
    pub pins: Vec<Pin>,
    /// Package names the solve must not select at any version.
    #[serde(default)]
    pub forbid: Vec<String>,
    /// Optimisation objective. `prefer_newer` when unset, which is the only
    /// value the shipped solver implements.
    #[serde(default = "default_objective")]
    pub objective: String,
    /// Packages that must be strictly newer than baseline (when
    /// `relative_to_baseline` is true). Accepts a single object or an array
    /// in JSON for backward compatibility.
    #[serde(default, deserialize_with = "deserialize_require_upgrades")]
    pub require_upgrade: Vec<RequireUpgrade>,
}

/// Accept `null`, a single `RequireUpgrade` object, or an array of them.
fn deserialize_require_upgrades<'de, D>(deserializer: D) -> Result<Vec<RequireUpgrade>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Helper {
        One(RequireUpgrade),
        Many(Vec<RequireUpgrade>),
    }
    Ok(match Option::<Helper>::deserialize(deserializer)? {
        None => Vec::new(),
        Some(Helper::One(one)) => vec![one],
        Some(Helper::Many(many)) => many,
    })
}

fn default_objective() -> String {
    "prefer_newer".into()
}

impl Policy {
    /// Effective root priority: explicit `root_priority` when non-empty,
    /// otherwise `roots` order. Any root missing from the priority list is
    /// appended in `roots` order so every application root is optimized.
    pub fn effective_root_priority(&self) -> Vec<String> {
        let mut order: Vec<String> = match &self.root_priority {
            Some(p) if !p.is_empty() => p.clone(),
            _ => self.roots.clone(),
        };
        // Only roots participate in the objective.
        order.retain(|r| self.roots.iter().any(|root| root == r));
        for r in &self.roots {
            if !order.iter().any(|x| x == r) {
                order.push(r.clone());
            }
        }
        order
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// One selected variant as recorded in a lock.
pub struct LockPackage {
    /// Package name.
    pub name: String,
    /// Version selected.
    pub version: String,
    /// Toolchain the selected easyconfig builds against.
    pub toolchain: Toolchain,
    /// Versionsuffix of the selected variant, when it has one.
    #[serde(default)]
    pub versionsuffix: Option<String>,
    /// Easyconfig the selection came from, so a lock can be traced back to a
    /// file. Empty when the candidate was parsed from memory.
    pub easyconfig_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Provenance of a solve, so a lock says what produced it.
pub struct SolverMeta {
    /// Solver that produced the lock, e.g. `resolvo`.
    pub engine: String,
    /// Version of that solver.
    pub engine_version: String,
    /// When the solve ran, as an RFC 3339 timestamp.
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A complete, reproducible selection: every package a stack installs.
pub struct StackLock {
    /// Schema version of this lock document. Readers reject what they do not
    /// know rather than guessing at an unfamiliar shape.
    pub schema_version: u32,
    /// Toolchain the stack targets.
    pub toolchain: Toolchain,
    /// Generation label carried from the universe, when one was given.
    #[serde(default)]
    pub generation_label: Option<String>,
    /// Selected packages, sorted by name for a stable diff.
    pub packages: Vec<LockPackage>,
    /// What produced this lock.
    pub solver: SolverMeta,
}

impl StackLock {
    /// The locked entry for `name`, or `None` when the stack has no such
    /// package.
    pub fn package(&self, name: &str) -> Option<&LockPackage> {
        self.packages.iter().find(|p| p.name == name)
    }
}
