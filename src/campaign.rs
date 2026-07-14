//! Persisted build-evaluation campaigns with typed failure findings.

use crate::target::{BuildTarget, CommandPlan, TargetError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const CAMPAIGN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct CampaignRequest {
    pub bundle: PathBuf,
    pub target: BuildTarget,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CampaignStatus {
    Planned,
    Running,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildFindingClass {
    Transport,
    Executor,
    Runtime,
    Checksum,
    Patch,
    DependencyMissing,
    Configure,
    Compile,
    Link,
    Test,
    Install,
    Sanity,
    Resource,
    Timeout,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingDisposition {
    Mechanical,
    Retryable,
    RequiresJudgment,
    TargetRepair,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildFinding {
    pub id: String,
    pub class: BuildFindingClass,
    pub disposition: FindingDisposition,
    pub stage: String,
    pub recipe: String,
    pub target: String,
    pub summary: String,
    pub evidence: String,
    pub command: CommandPlan,
    pub exit_code: Option<i32>,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ClaimLadder {
    pub resolves: bool,
    pub builds: bool,
    pub binary_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignEvent {
    pub attempt: u32,
    pub status: CampaignStatus,
    pub recipe: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignState {
    pub schema_version: u32,
    pub package: String,
    pub version: String,
    pub bundle: String,
    pub target: String,
    pub status: CampaignStatus,
    pub attempts: u32,
    pub claims: ClaimLadder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_recipe: Option<String>,
    #[serde(default)]
    pub findings: Vec<BuildFinding>,
    #[serde(default)]
    pub history: Vec<CampaignEvent>,
}

pub fn run_campaign(request: &CampaignRequest) -> Result<CampaignState, CampaignError> {
    let manifest_path = request.bundle.join("package.plan.json");
    let manifest: Value = read_json(&manifest_path)?;
    let package = manifest
        .pointer("/package/name")
        .and_then(Value::as_str)
        .ok_or_else(|| CampaignError::InvalidBundle("manifest has no package.name".into()))?;
    let version = manifest
        .pointer("/package/version")
        .and_then(Value::as_str)
        .ok_or_else(|| CampaignError::InvalidBundle("manifest has no package.version".into()))?;
    let recipes = discover_files(&request.bundle.join("easyconfigs"), "eb")?;
    if recipes.is_empty() {
        return Err(CampaignError::InvalidBundle(
            "bundle has no EasyBuild recipes".into(),
        ));
    }
    let locks = discover_files(&request.bundle.join("locks"), "json")?;
    if locks.is_empty() {
        return Err(CampaignError::InvalidBundle(
            "bundle has no Resolvo profile locks".into(),
        ));
    }

    let mut state = if request.state_path.is_file() {
        let state: CampaignState = read_json(&request.state_path)?;
        if state.schema_version != CAMPAIGN_SCHEMA_VERSION {
            return Err(CampaignError::UnsupportedSchema(state.schema_version));
        }
        if state.package != package || state.version != version {
            return Err(CampaignError::StateIdentity);
        }
        state
    } else {
        CampaignState {
            schema_version: CAMPAIGN_SCHEMA_VERSION,
            package: package.into(),
            version: version.into(),
            bundle: request.bundle.display().to_string(),
            target: request.target.name.clone(),
            status: CampaignStatus::Planned,
            attempts: 0,
            claims: ClaimLadder {
                resolves: true,
                builds: false,
                binary_verified: false,
            },
            current_recipe: None,
            findings: Vec::new(),
            history: Vec::new(),
        }
    };

    state.attempts += 1;
    state.target = request.target.name.clone();
    state.status = CampaignStatus::Running;
    state.claims.builds = false;
    state.history.push(CampaignEvent {
        attempt: state.attempts,
        status: CampaignStatus::Running,
        recipe: None,
        detail: format!("build evaluation on {}", request.target.name),
    });
    write_state(&request.state_path, &state)?;
    let staged_bundle = request.target.stage_bundle(&request.bundle)?;

    for recipe in recipes {
        let relative_recipe = recipe
            .strip_prefix(&request.bundle)
            .map_err(|_| CampaignError::InvalidBundle("recipe is outside bundle".into()))?;
        let recipe_text = relative_recipe.display().to_string();
        let staged_recipe = Path::new(&staged_bundle).join(relative_recipe);
        state.current_recipe = Some(recipe_text.clone());
        write_state(&request.state_path, &state)?;
        let command = request
            .target
            .build_command(&staged_recipe.display().to_string());
        let output = command.execute().map_err(CampaignError::Target)?;
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let class = classify_build_failure("build", &stdout, &stderr, output.status.code());
            let evidence = compact_evidence(&stdout, &stderr);
            state.findings.push(BuildFinding {
                id: format!(
                    "attempt:{}:finding:{}",
                    state.attempts,
                    state.findings.len() + 1
                ),
                class,
                disposition: disposition(class),
                stage: "build".into(),
                recipe: recipe_text.clone(),
                target: request.target.name.clone(),
                summary: finding_summary(class, output.status.code()),
                evidence,
                command,
                exit_code: output.status.code(),
                attempt: state.attempts,
            });
            state.status = CampaignStatus::Failed;
            state.current_recipe = None;
            state.history.push(CampaignEvent {
                attempt: state.attempts,
                status: CampaignStatus::Failed,
                recipe: Some(recipe_text),
                detail: format!("classified build failure as {class:?}"),
            });
            write_state(&request.state_path, &state)?;
            return Ok(state);
        }
        state.history.push(CampaignEvent {
            attempt: state.attempts,
            status: CampaignStatus::Running,
            recipe: Some(recipe_text),
            detail: "EasyBuild command succeeded".into(),
        });
    }

    state.status = CampaignStatus::Completed;
    state.current_recipe = None;
    state.claims.builds = true;
    state.history.push(CampaignEvent {
        attempt: state.attempts,
        status: CampaignStatus::Completed,
        recipe: None,
        detail: "all EasyBuild commands succeeded".into(),
    });
    write_state(&request.state_path, &state)?;
    Ok(state)
}

pub fn classify_build_failure(
    stage: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> BuildFindingClass {
    let text = format!("{stage}\n{stdout}\n{stderr}").to_ascii_lowercase();
    if text.contains("ssh:")
        || text.contains("connection refused")
        || text.contains("connection timed out")
    {
        BuildFindingClass::Transport
    } else if text.contains("slurm") && (text.contains("error") || text.contains("invalid")) {
        if text.contains("oom") || text.contains("out of memory") {
            BuildFindingClass::Resource
        } else {
            BuildFindingClass::Executor
        }
    } else if text.contains("oom-kill") || text.contains("out of memory") {
        BuildFindingClass::Resource
    } else if text.contains("checksum") && (text.contains("failed") || text.contains("mismatch")) {
        BuildFindingClass::Checksum
    } else if text.contains("patch") && (text.contains("failed") || text.contains("reject")) {
        BuildFindingClass::Patch
    } else if text.contains("no such file or directory")
        && (text.contains("fatal error") || text.contains("header"))
    {
        BuildFindingClass::DependencyMissing
    } else if text.contains("cmake error")
        || text.contains("configure: error")
        || text.contains("meson.build:")
    {
        BuildFindingClass::Configure
    } else if text.contains("undefined reference") || text.contains("ld: cannot find") {
        BuildFindingClass::Link
    } else if text.contains("tests failed")
        || text.contains("test failed")
        || text.contains("ctest") && text.contains("failed")
    {
        BuildFindingClass::Test
    } else if text.contains("sanity check failed") {
        BuildFindingClass::Sanity
    } else if text.contains("install") && text.contains("failed") {
        BuildFindingClass::Install
    } else if text.contains("timed out") || exit_code == Some(124) {
        BuildFindingClass::Timeout
    } else if text.contains("error:") || text.contains("compilation terminated") {
        BuildFindingClass::Compile
    } else {
        BuildFindingClass::Unknown
    }
}

fn disposition(class: BuildFindingClass) -> FindingDisposition {
    match class {
        BuildFindingClass::Transport | BuildFindingClass::Executor | BuildFindingClass::Runtime => {
            FindingDisposition::TargetRepair
        }
        BuildFindingClass::Resource | BuildFindingClass::Timeout => FindingDisposition::Retryable,
        BuildFindingClass::Checksum => FindingDisposition::Mechanical,
        _ => FindingDisposition::RequiresJudgment,
    }
}

fn finding_summary(class: BuildFindingClass, exit_code: Option<i32>) -> String {
    format!("{class:?} failure from EasyBuild command (exit {exit_code:?})")
}

fn compact_evidence(stdout: &str, stderr: &str) -> String {
    let combined = format!("stdout:\n{stdout}\nstderr:\n{stderr}");
    let lines = combined.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(200);
    let mut compact = lines[start..].join("\n");
    if compact.len() > 64 * 1024 {
        compact = compact
            .chars()
            .rev()
            .take(64 * 1024)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
    }
    compact
}

fn discover_files(root: &Path, extension: &str) -> Result<Vec<PathBuf>, CampaignError> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| CampaignError::Io(directory.clone(), error))?
        {
            let entry = entry.map_err(|error| CampaignError::Io(directory.clone(), error))?;
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, CampaignError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CampaignError::Io(path.to_path_buf(), error))?;
    serde_json::from_str(&text).map_err(|error| CampaignError::Json(path.to_path_buf(), error))
}

fn write_state(path: &Path, state: &CampaignState) -> Result<(), CampaignError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CampaignError::Io(parent.to_path_buf(), error))?;
    }
    let temporary = path.with_extension("tmp");
    let mut text = serde_json::to_string_pretty(state)
        .map_err(|error| CampaignError::Json(path.to_path_buf(), error))?;
    text.push('\n');
    std::fs::write(&temporary, text)
        .map_err(|error| CampaignError::Io(temporary.clone(), error))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| CampaignError::Io(path.to_path_buf(), error))?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum CampaignError {
    #[error("invalid package bundle: {0}")]
    InvalidBundle(String),
    #[error("unsupported campaign schema version {0}")]
    UnsupportedSchema(u32),
    #[error("campaign state package identity does not match the bundle")]
    StateIdentity,
    #[error("target command: {0}")]
    Target(#[from] TargetError),
    #[error("read or write {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("JSON {0}: {1}")]
    Json(PathBuf, serde_json::Error),
}
