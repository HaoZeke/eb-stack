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
