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
    #[serde(default)]
    pub require_upgrade: Option<RequireUpgrade>,
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
