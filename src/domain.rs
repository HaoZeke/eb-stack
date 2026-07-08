//! Domain types for EasyBuild stack selection.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toolchain {
    pub name: String,
    pub version: String,
}

impl Toolchain {
    pub fn label(&self) -> String {
        format!("{}-{}", self.name, self.version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepReq {
    pub name: String,
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
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub name: String,
    pub version: String,
    pub toolchain: Toolchain,
    #[serde(default)]
    pub versionsuffix: Option<String>,
    pub easyconfig_path: String,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Universe {
    pub toolchain: Toolchain,
    #[serde(default)]
    pub generation_label: Option<String>,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub name: String,
    pub version_req: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequireUpgrade {
    pub name: String,
    /// When true, the selected version of `name` must be strictly newer than
    /// the baseline lock's version. When false, construction fails with a
    /// clear error (absolute require_upgrade is not silently ignored).
    #[serde(default)]
    pub relative_to_baseline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub toolchain: Toolchain,
    pub roots: Vec<String>,
    /// Declared priority order over application roots for multi-root
    /// lexicographic newest selection. When omitted or empty, defaults to
    /// [`Self::roots`] list order. Explicit priority is independent of
    /// reordering `roots` in the policy JSON.
    #[serde(default)]
    pub root_priority: Option<Vec<String>>,
    #[serde(default)]
    pub pins: Vec<Pin>,
    #[serde(default)]
    pub forbid: Vec<String>,
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
pub struct LockPackage {
    pub name: String,
    pub version: String,
    pub toolchain: Toolchain,
    #[serde(default)]
    pub versionsuffix: Option<String>,
    pub easyconfig_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverMeta {
    pub engine: String,
    pub engine_version: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackLock {
    pub schema_version: u32,
    pub toolchain: Toolchain,
    #[serde(default)]
    pub generation_label: Option<String>,
    pub packages: Vec<LockPackage>,
    pub solver: SolverMeta,
}

impl StackLock {
    pub fn package(&self, name: &str) -> Option<&LockPackage> {
        self.packages.iter().find(|p| p.name == name)
    }
}
