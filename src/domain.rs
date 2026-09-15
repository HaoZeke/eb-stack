//! Domain types for EasyBuild stack selection.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

    /// EasyBuild `SYSTEM` / historical `dummy`, regardless of version spelling.
    pub fn is_system(&self) -> bool {
        self.name.eq_ignore_ascii_case("system") || self.name.eq_ignore_ascii_case("dummy")
    }

    /// Identity label: `system` for SYSTEM/dummy, otherwise [`Self::label`].
    pub fn identity_label(&self) -> String {
        if self.is_system() {
            "system".into()
        } else {
            self.label()
        }
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

/// The four fields that make two candidates the same module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CandidateKey {
    /// Package name.
    pub name: String,
    /// Package version.
    pub version: String,
    /// [`Toolchain::identity_label`].
    pub toolchain: String,
    /// Versionsuffix, empty when the recipe has none.
    pub versionsuffix: String,
}

impl Candidate {
    /// Identity for walks and maps: SYSTEM/dummy collapse, empty suffix is none.
    pub fn identity_key(&self) -> CandidateKey {
        CandidateKey {
            name: self.name.clone(),
            version: self.version.clone(),
            toolchain: self.toolchain.identity_label(),
            versionsuffix: self.versionsuffix.clone().unwrap_or_default(),
        }
    }
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
#[serde(deny_unknown_fields)]
/// A constraint fixing one package to a version or range.
pub struct Pin {
    /// Package the pin applies to.
    pub name: String,
    /// Version requirement the selection must satisfy, same spelling rules as
    /// [`DepReq::version_req`].
    pub version_req: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
    #[serde(
        default = "default_objective",
        deserialize_with = "deserialize_objective"
    )]
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

fn deserialize_objective<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value == "prefer_newer" {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported policy objective {value}; only prefer_newer is implemented \
             (set prefer_installed for the installed-version preference)"
        )))
    }
}

impl Policy {
    /// Effective root priority: explicit `root_priority` when non-empty,
    /// otherwise `roots` order. Any root missing from the priority list is
    /// appended in `roots` order so every application root is optimized.
    pub fn effective_root_priority(&self) -> Result<Vec<String>, String> {
        let mut order: Vec<String> = match &self.root_priority {
            Some(p) if !p.is_empty() => p.clone(),
            _ => self.roots.clone(),
        };
        let mut pin_names = std::collections::BTreeSet::new();
        for pin in &self.pins {
            if !pin_names.insert(pin.name.as_str()) {
                return Err(format!("pins lists {} more than once", pin.name));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for name in &order {
            if !self.roots.iter().any(|root| root == name) {
                return Err(format!("root_priority references unknown root {name}"));
            }
            if !seen.insert(name) {
                return Err(format!("root_priority lists {name} more than once"));
            }
        }
        for r in &self.roots {
            if !order.iter().any(|x| x == r) {
                order.push(r.clone());
            }
        }
        Ok(order)
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

impl LockPackage {
    /// The same four-field identity a [`Candidate`] uses.
    pub fn identity_key(&self) -> CandidateKey {
        CandidateKey {
            name: self.name.clone(),
            version: self.version.clone(),
            toolchain: self.toolchain.identity_label(),
            versionsuffix: self.versionsuffix.clone().unwrap_or_default(),
        }
    }
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
    /// The unique locked entry for `name`.
    ///
    /// A stack can carry the same name at more than one toolchain (SYSTEM
    /// bootstrap next to a generation build). First-match would hide the
    /// later one; if the name is not unique this returns `None`.
    pub fn package(&self, name: &str) -> Option<&LockPackage> {
        let mut found = None;
        for package in &self.packages {
            if package.name != name {
                continue;
            }
            if found.is_some() {
                return None;
            }
            found = Some(package);
        }
        found
    }

    /// Reject a lock whose schema this reader does not know.
    pub fn validate_schema(&self) -> Result<(), String> {
        if self.schema_version != STACK_LOCK_SCHEMA_VERSION {
            return Err(format!(
                "unsupported stack lock schema version {}",
                self.schema_version
            ));
        }
        Ok(())
    }
}

/// Schema version written into every [`StackLock`].
pub const STACK_LOCK_SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(packages: Vec<LockPackage>, schema_version: u32) -> StackLock {
        StackLock {
            schema_version,
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2026.1".into(),
            },
            generation_label: None,
            packages,
            solver: SolverMeta {
                engine: "test".into(),
                engine_version: "test".into(),
                timestamp: "STABLE".into(),
            },
        }
    }

    fn pkg(name: &str, version: &str, toolchain: &str) -> LockPackage {
        LockPackage {
            name: name.into(),
            version: version.into(),
            toolchain: Toolchain {
                name: toolchain.into(),
                version: "system".into(),
            },
            versionsuffix: None,
            easyconfig_path: format!("{name}-{version}.eb"),
        }
    }

    #[test]
    fn package_lookup_is_none_when_the_name_is_not_unique() {
        let stack = lock(
            vec![
                pkg("Python", "3.12.3", "system"),
                pkg("Python", "3.13.1", "GCCcore"),
            ],
            STACK_LOCK_SCHEMA_VERSION,
        );
        assert!(stack.package("Python").is_none());
        assert_eq!(stack.package("missing"), None);
    }

    #[test]
    fn package_lookup_returns_the_unique_name() {
        let stack = lock(
            vec![pkg("Python", "3.12.3", "GCCcore")],
            STACK_LOCK_SCHEMA_VERSION,
        );
        assert_eq!(stack.package("Python").unwrap().version, "3.12.3");
    }

    #[test]
    fn system_and_dummy_share_an_identity_label() {
        let system = Toolchain {
            name: "system".into(),
            version: "system".into(),
        };
        let dummy = Toolchain {
            name: "dummy".into(),
            version: String::new(),
        };
        assert_eq!(system.identity_label(), dummy.identity_label());
        assert_eq!(system.identity_label(), "system");
        assert_ne!(system.label(), dummy.label());
    }

    #[test]
    fn empty_suffix_and_none_share_a_candidate_key() {
        let toolchain = Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        };
        let plain = Candidate {
            name: "Lib".into(),
            version: "1.0".into(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            easyconfig_path: "a.eb".into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        };
        let mut empty = plain.clone();
        empty.versionsuffix = Some(String::new());
        empty.easyconfig_path = "b.eb".into();
        empty.moduleclass = Some("lib".into());
        assert_eq!(plain.identity_key(), empty.identity_key());
        assert_ne!(plain, empty);
    }

    #[test]
    fn unknown_stack_lock_schema_is_rejected() {
        let stack = lock(Vec::new(), 99);
        assert!(stack.validate_schema().is_err());
        assert!(lock(Vec::new(), STACK_LOCK_SCHEMA_VERSION)
            .validate_schema()
            .is_ok());
    }

    fn policy(roots: &[&str], priority: Option<&[&str]>) -> Policy {
        Policy {
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2026.1".into(),
            },
            roots: roots.iter().map(|name| (*name).to_string()).collect(),
            root_priority: priority
                .map(|names| names.iter().map(|name| (*name).to_string()).collect()),
            prefer_installed: false,
            pins: Vec::new(),
            forbid: Vec::new(),
            objective: default_objective(),
            require_upgrade: Vec::new(),
        }
    }

    #[test]
    fn unknown_policy_objective_is_rejected() {
        let json = r#"{
            "toolchain": {"name": "foss", "version": "2026.1"},
            "roots": ["App"],
            "objective": "prefer_installed"
        }"#;
        let error = serde_json::from_str::<Policy>(json).expect_err("objective typo");
        assert!(
            error.to_string().contains("prefer_installed")
                || error.to_string().contains("objective"),
            "{error}"
        );
    }

    #[test]
    fn unknown_nested_policy_fields_are_rejected() {
        let toolchain = r#"{
            "toolchain": {"name": "foss", "version": "2026.1", "verison": "2025b"},
            "roots": ["App"]
        }"#;
        let error = serde_json::from_str::<Policy>(toolchain).expect_err("toolchain typo");
        assert!(
            error.to_string().contains("unknown field") || error.to_string().contains("verison"),
            "{error}"
        );
        let pin = r#"{
            "toolchain": {"name": "foss", "version": "2026.1"},
            "roots": ["App"],
            "pins": [{"name": "Lib", "version_req": "==1.0", "version": "==2.0"}]
        }"#;
        let error = serde_json::from_str::<Policy>(pin).expect_err("pin typo");
        assert!(
            error.to_string().contains("unknown field") || error.to_string().contains("version"),
            "{error}"
        );
        let upgrade = r#"{
            "toolchain": {"name": "foss", "version": "2026.1"},
            "roots": ["App"],
            "require_upgrade": [{"name": "App", "relative_to_basline": true}]
        }"#;
        let error = serde_json::from_str::<Policy>(upgrade).expect_err("upgrade typo");
        assert!(
            error.to_string().contains("unknown field")
                || error.to_string().contains("relative_to_basline")
                || error.to_string().contains("untagged"),
            "{error}"
        );
    }

    #[test]
    fn duplicate_pin_names_are_an_error() {
        let mut policy = policy(&["App"], None);
        policy.pins = vec![
            Pin {
                name: "App".into(),
                version_req: "==2.0".into(),
            },
            Pin {
                name: "App".into(),
                version_req: "==1.0".into(),
            },
        ];
        let error = policy.effective_root_priority().expect_err("duplicate pin");
        assert!(error.contains("App") && error.contains("pins"), "{error}");
    }

    #[test]
    fn unknown_policy_field_is_rejected() {
        let json = r#"{
            "toolchain": {"name": "foss", "version": "2026.1"},
            "roots": ["App"],
            "prefer_instaleld": true
        }"#;
        let error = serde_json::from_str::<Policy>(json).expect_err("field typo");
        assert!(
            error.to_string().contains("unknown field")
                || error.to_string().contains("prefer_instaleld"),
            "{error}"
        );
    }

    #[test]
    fn unknown_root_priority_name_is_an_error() {
        let policy = policy(&["GROMACS", "LAMMPS"], Some(&["Gromacs", "LAMMPS"]));
        let error = policy.effective_root_priority().expect_err("typo");
        assert!(
            error.contains("Gromacs"),
            "unknown priority name must be reported: {error}"
        );
    }

    #[test]
    fn duplicate_root_priority_name_is_an_error() {
        let policy = policy(
            &["LAMMPS", "GROMACS"],
            Some(&["LAMMPS", "GROMACS", "LAMMPS"]),
        );
        let error = policy.effective_root_priority().expect_err("duplicate");
        assert!(
            error.contains("LAMMPS"),
            "duplicate priority name must be reported: {error}"
        );
    }
}
