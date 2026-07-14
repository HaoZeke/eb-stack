//! Persisted build-evaluation campaigns with typed failure findings.

use crate::package::ProductProfile;
use crate::target::{
    BuildTarget, CommandPlan, TargetError, TargetExecutor, TargetRuntime, TargetTransport,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::OpenOptions;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum FindingStatus {
    #[default]
    Open,
    InProgress,
    Resolved,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingResolution {
    pub action: String,
    pub evidence: String,
    #[serde(default)]
    pub changes: Vec<String>,
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
    #[serde(default)]
    pub status: FindingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<FindingResolution>,
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
    let _lock = CampaignLock::acquire(&request.state_path)?;
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
    state.claims.binary_verified = false;
    state.history.push(CampaignEvent {
        attempt: state.attempts,
        status: CampaignStatus::Running,
        recipe: None,
        detail: format!("build evaluation on {}", request.target.name),
    });
    write_state(&request.state_path, &state)?;
    let staged_bundle = match request.target.stage_bundle(&request.bundle) {
        Ok(path) => path,
        Err(error) => {
            let evidence = error.to_string();
            state.findings.push(BuildFinding {
                id: format!(
                    "attempt:{}:finding:{}",
                    state.attempts,
                    state.findings.len() + 1
                ),
                class: BuildFindingClass::Transport,
                disposition: FindingDisposition::TargetRepair,
                stage: "stage".into(),
                recipe: String::new(),
                target: request.target.name.clone(),
                summary: "package bundle staging failed".into(),
                evidence,
                command: CommandPlan {
                    program: "stage-bundle".into(),
                    args: vec![request.bundle.display().to_string()],
                },
                exit_code: None,
                attempt: state.attempts,
                status: FindingStatus::Open,
                owner: None,
                resolution: None,
            });
            state.status = CampaignStatus::Failed;
            state.current_recipe = None;
            state.history.push(CampaignEvent {
                attempt: state.attempts,
                status: CampaignStatus::Failed,
                recipe: None,
                detail: "classified bundle staging failure as Transport".into(),
            });
            write_state(&request.state_path, &state)?;
            return Ok(state);
        }
    };

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
        let output = match command.execute() {
            Ok(output) => output,
            Err(error) => {
                record_target_command_failure(
                    &mut state,
                    &request.target,
                    "build",
                    &recipe_text,
                    command,
                    &error,
                );
                state.current_recipe = None;
                write_state(&request.state_path, &state)?;
                return Ok(state);
            }
        };
        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let evidence = build_failure_evidence(&stdout, &stderr);
            let class = classify_build_failure("build", &evidence, "", output.status.code());
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
                status: FindingStatus::Open,
                owner: None,
                resolution: None,
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
            recipe: Some(recipe_text.clone()),
            detail: "EasyBuild command succeeded".into(),
        });
        supersede_findings(&mut state, "build", &recipe_text);
    }

    state.current_recipe = None;
    state.claims.builds = true;
    write_state(&request.state_path, &state)?;

    let verification_profiles = verification_profiles(&manifest)?;
    let verification_count = verification_profiles
        .iter()
        .map(|profile| profile.verification_commands.len())
        .sum::<usize>();
    for profile in verification_profiles {
        let module = module_name(&manifest, package, version, &profile)?;
        for verification in &profile.verification_commands {
            let program = expand_verification_token(
                &verification.program,
                &module,
                package,
                version,
                &profile,
            );
            let args = verification
                .args
                .iter()
                .map(|argument| {
                    expand_verification_token(argument, &module, package, version, &profile)
                })
                .collect::<Vec<_>>();
            let command = request.target.verification_command(&program, &args);
            let output = match command.execute() {
                Ok(output) => output,
                Err(error) => {
                    record_target_command_failure(
                        &mut state,
                        &request.target,
                        "verify",
                        &format!("profile:{}", profile.name),
                        command,
                        &error,
                    );
                    write_state(&request.state_path, &state)?;
                    return Ok(state);
                }
            };
            if !output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                let class =
                    classify_build_failure("verify", &stdout, &stderr, output.status.code());
                state.findings.push(BuildFinding {
                    id: format!(
                        "attempt:{}:finding:{}",
                        state.attempts,
                        state.findings.len() + 1
                    ),
                    class,
                    disposition: disposition(class),
                    stage: "verify".into(),
                    recipe: format!("profile:{}", profile.name),
                    target: request.target.name.clone(),
                    summary: format!(
                        "binary verification failed for profile {} (exit {:?})",
                        profile.name,
                        output.status.code()
                    ),
                    evidence: format!("module={module}\n{}", compact_evidence(&stdout, &stderr)),
                    command,
                    exit_code: output.status.code(),
                    attempt: state.attempts,
                    status: FindingStatus::Open,
                    owner: None,
                    resolution: None,
                });
                state.status = CampaignStatus::Failed;
                state.history.push(CampaignEvent {
                    attempt: state.attempts,
                    status: CampaignStatus::Failed,
                    recipe: Some(format!("profile:{}", profile.name)),
                    detail: format!("classified binary verification failure as {class:?}"),
                });
                write_state(&request.state_path, &state)?;
                return Ok(state);
            }
            state.history.push(CampaignEvent {
                attempt: state.attempts,
                status: CampaignStatus::Running,
                recipe: Some(format!("profile:{}", profile.name)),
                detail: format!("binary verification succeeded for module {module}"),
            });
            supersede_findings(&mut state, "verify", &format!("profile:{}", profile.name));
            write_state(&request.state_path, &state)?;
        }
    }

    state.status = CampaignStatus::Completed;
    state.claims.binary_verified = verification_count > 0;
    state.history.push(CampaignEvent {
        attempt: state.attempts,
        status: CampaignStatus::Completed,
        recipe: None,
        detail: if verification_count > 0 {
            "all EasyBuild and binary verification commands succeeded".into()
        } else {
            "all EasyBuild commands succeeded; no binary verification commands declared".into()
        },
    });
    write_state(&request.state_path, &state)?;
    Ok(state)
}

pub fn claim_finding(
    state_path: &Path,
    finding_id: &str,
    owner: &str,
) -> Result<CampaignState, CampaignError> {
    let _lock = CampaignLock::acquire(state_path)?;
    let mut state: CampaignState = read_json(state_path)?;
    let finding = state
        .findings
        .iter_mut()
        .find(|finding| finding.id == finding_id)
        .ok_or_else(|| CampaignError::FindingNotFound(finding_id.into()))?;
    match finding.status {
        FindingStatus::Open => {
            finding.status = FindingStatus::InProgress;
            finding.owner = Some(owner.into());
        }
        FindingStatus::InProgress if finding.owner.as_deref() == Some(owner) => {}
        FindingStatus::InProgress => {
            return Err(CampaignError::FindingOwned {
                id: finding_id.into(),
                owner: finding.owner.clone().unwrap_or_else(|| "unknown".into()),
            });
        }
        status => {
            return Err(CampaignError::FindingState {
                id: finding_id.into(),
                status,
            });
        }
    }
    write_state(state_path, &state)?;
    Ok(state)
}

pub fn resolve_finding(
    state_path: &Path,
    finding_id: &str,
    owner: &str,
    resolution: FindingResolution,
) -> Result<CampaignState, CampaignError> {
    let _lock = CampaignLock::acquire(state_path)?;
    let mut state: CampaignState = read_json(state_path)?;
    let finding = state
        .findings
        .iter_mut()
        .find(|finding| finding.id == finding_id)
        .ok_or_else(|| CampaignError::FindingNotFound(finding_id.into()))?;
    if finding.status != FindingStatus::InProgress {
        return Err(CampaignError::FindingState {
            id: finding_id.into(),
            status: finding.status,
        });
    }
    if finding.owner.as_deref() != Some(owner) {
        return Err(CampaignError::FindingOwned {
            id: finding_id.into(),
            owner: finding.owner.clone().unwrap_or_else(|| "unknown".into()),
        });
    }
    finding.status = FindingStatus::Resolved;
    finding.resolution = Some(resolution);
    write_state(state_path, &state)?;
    Ok(state)
}

fn supersede_findings(state: &mut CampaignState, stage: &str, recipe: &str) {
    for finding in &mut state.findings {
        if finding.stage == stage
            && finding.recipe == recipe
            && matches!(
                finding.status,
                FindingStatus::Open | FindingStatus::InProgress
            )
        {
            finding.status = FindingStatus::Superseded;
            finding.resolution.get_or_insert_with(|| FindingResolution {
                action: "successful campaign retry superseded this finding".into(),
                evidence: format!("attempt {} succeeded at stage {stage}", state.attempts),
                changes: Vec::new(),
            });
        }
    }
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
    } else if text.contains("glibc_") && text.contains("not found") {
        BuildFindingClass::Runtime
    } else if text.contains("checksum") && (text.contains("failed") || text.contains("mismatch")) {
        BuildFindingClass::Checksum
    } else if text.contains("patch") && (text.contains("failed") || text.contains("reject")) {
        BuildFindingClass::Patch
    } else if text.contains("no such file or directory")
        && (text.contains("env:") || exit_code == Some(127))
    {
        BuildFindingClass::Runtime
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
    } else if stage.eq_ignore_ascii_case("verify") {
        BuildFindingClass::Sanity
    } else {
        BuildFindingClass::Unknown
    }
}

fn record_target_command_failure(
    state: &mut CampaignState,
    target: &BuildTarget,
    stage: &str,
    recipe: &str,
    command: CommandPlan,
    error: &TargetError,
) {
    let class = classify_target_command_failure(target, error);
    state.findings.push(BuildFinding {
        id: format!(
            "attempt:{}:finding:{}",
            state.attempts,
            state.findings.len() + 1
        ),
        class,
        disposition: disposition(class),
        stage: stage.into(),
        recipe: recipe.into(),
        target: target.name.clone(),
        summary: format!("{class:?} target command could not start"),
        evidence: error.to_string(),
        command,
        exit_code: None,
        attempt: state.attempts,
        status: FindingStatus::Open,
        owner: None,
        resolution: None,
    });
    state.status = CampaignStatus::Failed;
    state.history.push(CampaignEvent {
        attempt: state.attempts,
        status: CampaignStatus::Failed,
        recipe: Some(recipe.into()),
        detail: format!("classified target command failure as {class:?}"),
    });
}

fn classify_target_command_failure(target: &BuildTarget, error: &TargetError) -> BuildFindingClass {
    let program = match error {
        TargetError::Spawn(program, _) | TargetError::CommandFailed { program, .. } => program,
        _ => return BuildFindingClass::Unknown,
    };
    if matches!(
        &target.transport,
        TargetTransport::Ssh { command, .. } if command == program
    ) {
        return BuildFindingClass::Transport;
    }
    if matches!(
        &target.executor,
        TargetExecutor::Slurm { command, .. } if command == program
    ) {
        return BuildFindingClass::Executor;
    }
    if matches!(
        &target.runtime,
        TargetRuntime::Podman { command, .. } | TargetRuntime::Docker { command, .. }
            if command == program
    ) {
        return BuildFindingClass::Runtime;
    }
    BuildFindingClass::Runtime
}

fn verification_profiles(manifest: &Value) -> Result<Vec<ProductProfile>, CampaignError> {
    manifest
        .get("profiles")
        .and_then(Value::as_array)
        .map(|profiles| {
            profiles
                .iter()
                .cloned()
                .map(|profile| {
                    serde_json::from_value(profile).map_err(|error| {
                        CampaignError::InvalidBundle(format!("invalid product profile: {error}"))
                    })
                })
                .collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn module_name(
    manifest: &Value,
    package: &str,
    version: &str,
    profile: &ProductProfile,
) -> Result<String, CampaignError> {
    let toolchain_name = manifest
        .pointer("/build/toolchain/name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CampaignError::InvalidBundle(
                "manifest with verification commands has no build.toolchain.name".into(),
            )
        })?;
    let toolchain_version = manifest
        .pointer("/build/toolchain/version")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CampaignError::InvalidBundle(
                "manifest with verification commands has no build.toolchain.version".into(),
            )
        })?;
    Ok(format!(
        "{package}/{version}{}-{toolchain_name}-{toolchain_version}",
        profile.versionsuffix.join("")
    ))
}

fn expand_verification_token(
    token: &str,
    module: &str,
    package: &str,
    version: &str,
    profile: &ProductProfile,
) -> String {
    token
        .replace("{module}", module)
        .replace("{package}", package)
        .replace("{version}", version)
        .replace("{profile}", &profile.name)
        .replace("{versionsuffix}", &profile.versionsuffix.join(""))
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

fn build_failure_evidence(stdout: &str, stderr: &str) -> String {
    let mut evidence = compact_evidence(stdout, stderr);
    let combined = format!("{stdout}\n{stderr}");
    for path in easybuild_output_paths(&combined).into_iter().take(4) {
        let Ok(nested) = std::fs::read_to_string(&path) else {
            continue;
        };
        evidence.push_str(&format!(
            "\nEasyBuild command output {}:\n{}",
            path.display(),
            compact_evidence(&nested, "")
        ));
    }
    evidence
}

fn easybuild_output_paths(output: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in output.lines() {
        if !line.contains("output (stdout + stderr)") {
            continue;
        }
        let Some((_, raw_path)) = line.rsplit_once("->") else {
            continue;
        };
        let raw_path = raw_path
            .split_once('\u{1b}')
            .map(|(path, _)| path)
            .unwrap_or(raw_path)
            .trim();
        if !raw_path.is_empty() {
            paths.push(PathBuf::from(raw_path));
        }
    }
    paths.sort();
    paths.dedup();
    paths
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

struct CampaignLock {
    path: PathBuf,
    _file: std::fs::File,
}

impl CampaignLock {
    fn acquire(state_path: &Path) -> Result<Self, CampaignError> {
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CampaignError::Io(parent.to_path_buf(), error))?;
        }
        let path = PathBuf::from(format!("{}.lock", state_path.display()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    CampaignError::Busy(path.clone())
                } else {
                    CampaignError::Io(path.clone(), error)
                }
            })?;
        Ok(Self { path, _file: file })
    }
}

impl Drop for CampaignLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Error)]
pub enum CampaignError {
    #[error("invalid package bundle: {0}")]
    InvalidBundle(String),
    #[error("unsupported campaign schema version {0}")]
    UnsupportedSchema(u32),
    #[error("campaign state package identity does not match the bundle")]
    StateIdentity,
    #[error("campaign state is busy: {0}")]
    Busy(PathBuf),
    #[error("campaign finding {0} does not exist")]
    FindingNotFound(String),
    #[error("campaign finding {id} is owned by {owner}")]
    FindingOwned { id: String, owner: String },
    #[error("campaign finding {id} cannot be changed from status {status:?}")]
    FindingState { id: String, status: FindingStatus },
    #[error("target command: {0}")]
    Target(#[from] TargetError),
    #[error("read or write {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("JSON {0}: {1}")]
    Json(PathBuf, serde_json::Error),
}
