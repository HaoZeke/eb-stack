//! Persisted build-evaluation campaigns with typed failure findings.

use crate::package::ProductProfile;
use crate::target::{
    BuildTarget, CommandPlan, TargetError, TargetExecutor, TargetRuntime, TargetTransport,
};
use crate::{packaging_gate, resolve_easyconfig_file};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Schema version of a campaign state document.
pub const CAMPAIGN_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone)]
/// What to build, where, and where to keep the state.
pub struct CampaignRequest {
    /// Package bundle produced by the planning stage.
    pub bundle: PathBuf,
    /// Where the build runs: local, SSH, scheduler, or container.
    pub target: BuildTarget,
    /// State file. Also the lock: one campaign owns it at a time.
    pub state_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Where a campaign has got to.
pub enum CampaignStatus {
    /// Created, nothing attempted yet.
    Planned,
    /// An attempt is in flight.
    Running,
    /// An attempt failed and left findings to work through.
    Failed,
    /// Every recipe built and its claims were verified.
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// What kind of failure a build hit.
///
/// The class drives routing, so it distinguishes causes that need different
/// responses: a transport failure is worth a retry, a checksum failure never
/// is.
pub enum BuildFindingClass {
    /// The target could not be reached or files could not be moved.
    Transport,
    /// The scheduler or container runtime refused the job.
    Executor,
    /// The build process died without a usable diagnostic.
    Runtime,
    /// The attempt was cancelled or the connection dropped.
    Interrupted,
    /// A source artifact could not be fetched.
    Source,
    /// A downloaded artifact did not match its declared hash.
    Checksum,
    /// A patch failed to apply.
    Patch,
    /// A required dependency is absent from the target.
    DependencyMissing,
    /// The configure step failed.
    Configure,
    /// Compilation failed.
    Compile,
    /// Linking failed.
    Link,
    /// The package test suite failed.
    Test,
    /// The install step failed.
    Install,
    /// EasyBuild's sanity check failed: it built but is not usable.
    Sanity,
    /// The job ran out of memory, disk, or another quota.
    Resource,
    /// The job exceeded its time limit.
    Timeout,
    /// The output did not match any known signature. Left unclassified
    ///     /// rather than guessed at, so it reaches a human.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// What kind of intervention a finding needs.
pub enum FindingDisposition {
    /// A tool can fix it without a decision.
    Mechanical,
    /// Worth retrying unchanged; the cause was transient.
    Retryable,
    /// Needs a human to decide what correct means.
    RequiresJudgment,
    /// The build target is at fault, not the recipe.
    TargetRepair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
/// How far a finding has been taken.
pub enum FindingStatus {
    #[default]
    /// Nobody has taken it.
    Open,
    /// Claimed by an owner who is working on it.
    InProgress,
    /// Closed with an action and evidence.
    Resolved,
    /// Overtaken by events, e.g. a later attempt stopped hitting it.
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// What was done about a finding, and the proof.
pub struct FindingResolution {
    /// What was changed or decided.
    pub action: String,
    /// Why that is believed to have worked, in reviewable terms.
    pub evidence: String,
    #[serde(default)]
    /// Files or recipes touched.
    pub changes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One thing that went wrong, classified so it can be routed.
pub struct BuildFinding {
    /// Stable identifier, used to claim and resolve it.
    pub id: String,
    /// What kind of failure it is.
    pub class: BuildFindingClass,
    /// What kind of intervention it needs.
    pub disposition: FindingDisposition,
    /// Build stage that failed.
    pub stage: String,
    /// Recipe being built.
    pub recipe: String,
    /// Target it was building on.
    pub target: String,
    /// One line a reviewer can triage from.
    pub summary: String,
    /// Captured output supporting the classification.
    pub evidence: String,
    /// The command that failed, so it can be rerun by hand.
    pub command: CommandPlan,
    /// Exit status, when the process produced one. Absent when it was
    ///     /// killed or never started.
    pub exit_code: Option<i32>,
    /// Attempt number this arose on.
    pub attempt: u32,
    #[serde(default)]
    /// How far it has been taken.
    pub status: FindingStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Who claimed it, while it is in progress.
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// What closed it, once it is resolved.
    pub resolution: Option<FindingResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// What has actually been demonstrated, in increasing strength.
///
/// Each rung is a separate claim: solving is not building, and building is not
/// evidence that the installed binaries run.
pub struct ClaimLadder {
    /// Dependencies were selected successfully.
    pub resolves: bool,
    /// The recipes built.
    pub builds: bool,
    /// The declared binaries were run and behaved.
    pub binary_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One entry in the campaign history.
pub struct CampaignEvent {
    /// Attempt this belongs to.
    pub attempt: u32,
    /// Status after the event.
    pub status: CampaignStatus,
    /// Recipe in play, when the event concerned one.
    pub recipe: Option<String>,
    /// What happened.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The durable record of a campaign, rewritten under a lock.
pub struct CampaignState {
    /// Must equal [`CAMPAIGN_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Package being built.
    pub package: String,
    /// Package version.
    pub version: String,
    /// Bundle the recipes came from.
    pub bundle: String,
    /// Target the build runs on.
    pub target: String,
    /// Where the campaign has got to.
    pub status: CampaignStatus,
    /// How many attempts have been made.
    pub attempts: u32,
    /// What has been demonstrated so far.
    pub claims: ClaimLadder,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Recipe in flight, while one is.
    pub current_recipe: Option<String>,
    #[serde(default)]
    /// Everything that went wrong, open and closed alike.
    pub findings: Vec<BuildFinding>,
    #[serde(default)]
    /// Ordered record of what happened.
    pub history: Vec<CampaignEvent>,
}

/// Run a campaign to completion or to its first unresolved failure.
///
/// Takes the state file lock for the duration, so a second campaign against
/// the same state fails with [`CampaignError::Busy`] rather than interleaving
/// writes with the first.
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
    let recipes = load_campaign_recipes(&request.bundle)?;
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

    record_interrupted_attempt(&mut state, &request.state_path);
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

    for recipe in &recipes {
        let relative_recipe = recipe
            .strip_prefix(&request.bundle)
            .map_err(|_| CampaignError::InvalidBundle("recipe is outside bundle".into()))?;
        let recipe_text = relative_recipe.display().to_string();
        let preflight = resolve_easyconfig_file(recipe)
            .map_err(|error| error.to_string())
            .and_then(|resolved| {
                packaging_gate(&resolved, &[])
                    .map_err(|errors| format!("packaging gate failed: {}", errors.join("; ")))
            });
        if let Err(evidence) = preflight {
            let class = if evidence.contains("checksum") {
                BuildFindingClass::Checksum
            } else {
                BuildFindingClass::Unknown
            };
            state.findings.push(BuildFinding {
                id: format!(
                    "attempt:{}:finding:{}",
                    state.attempts,
                    state.findings.len() + 1
                ),
                class,
                disposition: disposition(class),
                stage: "preflight".into(),
                recipe: recipe_text.clone(),
                target: request.target.name.clone(),
                summary: "recipe packaging preflight failed".into(),
                evidence,
                command: CommandPlan {
                    program: "recipe-metadata-gate".into(),
                    args: vec![recipe.display().to_string()],
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
                recipe: Some(recipe_text),
                detail: format!("classified packaging preflight failure as {class:?}"),
            });
            write_state(&request.state_path, &state)?;
            return Ok(state);
        }
    }

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
        let staged_overlay = Path::new(&staged_bundle)
            .join("easyconfigs")
            .display()
            .to_string();
        let command = request.target.build_command_with_robot_paths(
            &staged_recipe.display().to_string(),
            &[staged_overlay],
        );
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

fn record_interrupted_attempt(state: &mut CampaignState, state_path: &Path) {
    if state.status != CampaignStatus::Running {
        return;
    }
    let recipe = state.current_recipe.take().unwrap_or_default();
    let attempt = state.attempts;
    state.findings.push(BuildFinding {
        id: format!("attempt:{attempt}:finding:{}", state.findings.len() + 1),
        class: BuildFindingClass::Interrupted,
        disposition: FindingDisposition::Retryable,
        stage: "campaign".into(),
        recipe: recipe.clone(),
        target: state.target.clone(),
        summary: "campaign controller exited before recording a terminal state".into(),
        evidence: "an exclusive campaign lock was acquired while the persisted state was running"
            .into(),
        command: CommandPlan {
            program: "campaign-controller".into(),
            args: vec![state_path.display().to_string()],
        },
        exit_code: None,
        attempt,
        status: FindingStatus::Superseded,
        owner: None,
        resolution: Some(FindingResolution {
            action: "resumed campaign after controller interruption".into(),
            evidence: "the new controller acquired the exclusive campaign lock".into(),
            changes: Vec::new(),
        }),
    });
    state.history.push(CampaignEvent {
        attempt,
        status: CampaignStatus::Failed,
        recipe: (!recipe.is_empty()).then_some(recipe),
        detail: "recorded interrupted campaign controller".into(),
    });
}

/// Take ownership of an open finding.
///
/// Fails when another owner already holds it, so two people cannot both
/// believe they are fixing the same failure.
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

/// Close a finding with an action and its evidence.
///
/// Only the owner may close it, and only while it is in progress.
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

/// Classify a failed build stage from its output.
///
/// Routing depends on this: the class decides whether a failure is worth
/// retrying, needs a recipe change, or means the target itself is broken.
pub fn classify_build_failure(
    stage: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> BuildFindingClass {
    let text = format!("{stage}\n{stdout}\n{stderr}").to_ascii_lowercase();
    let missing_executable = text.contains("no such file or directory")
        && (text.contains("could not execute process")
            || text.contains("never executed")
            || text.contains("command not found")
            || text.contains("executable file not found")
            || text.contains("env:")
            || exit_code == Some(127));
    let checksum_failure = text.lines().any(|line| {
        line.contains("checksum mismatch")
            || line.contains("checksums do not match")
            || line.contains("checksum failed")
            || line.contains("checksum verification")
                && (line.contains("failed") || line.contains("failure"))
    });
    let source_failure = text.contains("failed to download")
        || text.contains("download failed")
        || text.contains("unable to download")
        || text.contains("could not download")
        || text.contains("couldn't find file") && text.contains("downloading it didn't work");
    let patch_failure = text.contains("failed to apply patch")
        || text.contains("could not apply patch")
        || text.contains("couldn't apply patch")
        || text.contains("can't find file to patch")
        || text.contains("patch failed")
        || text.contains("hunk #") && text.contains("failed");
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
    } else if text.contains("oom-kill")
        || text.contains("out of memory")
        || text.contains("virtual memory exhausted")
        || text.contains("killed signal terminated program")
    {
        BuildFindingClass::Resource
    } else if (text.contains("glibc_") && text.contains("not found")) || missing_executable {
        BuildFindingClass::Runtime
    } else if checksum_failure {
        BuildFindingClass::Checksum
    } else if source_failure {
        BuildFindingClass::Source
    } else if patch_failure {
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
    } else if text.contains("error:")
        || text.contains("compilation terminated")
        || stage.eq_ignore_ascii_case("build") && text.contains("make") && text.contains("***")
    {
        BuildFindingClass::Compile
    } else if text.contains("install") && text.contains("failed") {
        BuildFindingClass::Install
    } else if text.contains("timed out") || exit_code == Some(124) {
        BuildFindingClass::Timeout
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
        BuildFindingClass::Interrupted
        | BuildFindingClass::Resource
        | BuildFindingClass::Timeout => FindingDisposition::Retryable,
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

/// Prefer declared closure build order; fall back to recursive easyconfig discovery.
fn load_campaign_recipes(bundle: &Path) -> Result<Vec<PathBuf>, CampaignError> {
    let build_order_path = bundle.join("build-order.json");
    if build_order_path.is_file() {
        return load_declared_build_order(bundle, &build_order_path);
    }
    discover_files(&bundle.join("easyconfigs"), "eb")
}

fn load_declared_build_order(
    bundle: &Path,
    build_order_path: &Path,
) -> Result<Vec<PathBuf>, CampaignError> {
    let document: Value = read_json(build_order_path)?;
    let schema_version = document
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            CampaignError::InvalidBundle("build-order.json missing schema_version".into())
        })?;
    if schema_version != 1 {
        return Err(CampaignError::InvalidBundle(format!(
            "unsupported build-order.json schema version {schema_version}"
        )));
    }
    let recipes = document
        .get("recipes")
        .and_then(Value::as_array)
        .ok_or_else(|| CampaignError::InvalidBundle("build-order.json missing recipes".into()))?;
    let mut paths = Vec::with_capacity(recipes.len());
    for entry in recipes {
        let relative = entry.as_str().ok_or_else(|| {
            CampaignError::InvalidBundle("build-order.json recipes must be strings".into())
        })?;
        if relative.is_empty() || Path::new(relative).is_absolute() {
            return Err(CampaignError::InvalidBundle(format!(
                "build-order recipe must be a non-empty relative path: {relative}"
            )));
        }
        if relative.split(['/', '\\']).any(|part| part == "..") {
            return Err(CampaignError::InvalidBundle(format!(
                "build-order recipe escapes bundle: {relative}"
            )));
        }
        let path = bundle.join(relative);
        if !path.is_file() {
            return Err(CampaignError::InvalidBundle(format!(
                "build-order recipe missing: {relative}"
            )));
        }
        paths.push(path);
    }
    Ok(paths)
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
    metadata_path: PathBuf,
    _guard: std::fs::File,
}

impl CampaignLock {
    fn acquire(state_path: &Path) -> Result<Self, CampaignError> {
        if let Some(parent) = state_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CampaignError::Io(parent.to_path_buf(), error))?;
        }
        let metadata_path = PathBuf::from(format!("{}.lock", state_path.display()));
        let guard_path = PathBuf::from(format!("{}.lock.guard", state_path.display()));
        let guard = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&guard_path)
            .map_err(|error| CampaignError::Io(guard_path.clone(), error))?;
        FileExt::try_lock_exclusive(&guard).map_err(|error| {
            if error.kind() == fs2::lock_contended_error().kind() {
                CampaignError::Busy(metadata_path.clone())
            } else {
                CampaignError::Io(guard_path.clone(), error)
            }
        })?;

        let record = CampaignLockRecord {
            schema_version: 1,
            host: campaign_lock_host(),
            pid: std::process::id(),
            process_start_ticks: process_start_ticks(std::process::id()),
        };
        let mut metadata = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&metadata_path)
            .map_err(|error| CampaignError::Io(metadata_path.clone(), error))?;
        serde_json::to_writer_pretty(&mut metadata, &record)
            .map_err(|error| CampaignError::Json(metadata_path.clone(), error))?;
        metadata
            .write_all(b"\n")
            .and_then(|()| metadata.sync_all())
            .map_err(|error| CampaignError::Io(metadata_path.clone(), error))?;

        Ok(Self {
            metadata_path,
            _guard: guard,
        })
    }
}

impl Drop for CampaignLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.metadata_path);
        let _ = FileExt::unlock(&self._guard);
    }
}

#[derive(Debug, Serialize)]
struct CampaignLockRecord {
    schema_version: u32,
    host: String,
    pid: u32,
    process_start_ticks: Option<u64>,
}

fn campaign_lock_host() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(target_os = "linux")]
fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = stat.get(stat.rfind(") ")? + 2..)?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn process_start_ticks(_pid: u32) -> Option<u64> {
    None
}

#[derive(Debug, Error)]
/// Why a campaign could not run or be updated.
pub enum CampaignError {
    #[error("invalid package bundle: {0}")]
    /// The bundle is missing or does not describe a buildable package.
    InvalidBundle(String),
    #[error("unsupported campaign schema version {0}")]
    /// The state file declares a schema this build does not read.
    UnsupportedSchema(u32),
    #[error("campaign state package identity does not match the bundle")]
    /// The state file describes a different package or bundle.
    StateIdentity,
    #[error("campaign state is busy: {0}")]
    /// Another process holds the campaign lock.
    Busy(PathBuf),
    #[error("campaign finding {0} does not exist")]
    /// No finding with that identifier.
    FindingNotFound(String),
    #[error("campaign finding {id} is owned by {owner}")]
    /// The finding is claimed by someone else.
    FindingOwned {
        /// Finding someone else holds.
        id: String,
        /// Who holds it.
        owner: String,
    },
    #[error("campaign finding {id} cannot be changed from status {status:?}")]
    /// The finding is not in a state that allows this.
    FindingState {
        /// Finding whose state forbids the operation.
        id: String,
        /// State it is actually in.
        status: FindingStatus,
    },
    #[error("target command: {0}")]
    /// The build target could not be reached or prepared.
    Target(#[from] TargetError),
    #[error("read or write {0}: {1}")]
    /// A campaign file could not be read or written.
    Io(PathBuf, std::io::Error),
    #[error("JSON {0}: {1}")]
    /// A campaign file could not be parsed or serialized.
    Json(PathBuf, serde_json::Error),
}

#[cfg(test)]
mod campaign_lock_tests {
    use super::*;

    fn local_host() -> String {
        std::env::var("HOSTNAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                std::fs::read_to_string("/etc/hostname")
                    .expect("host identity")
                    .trim()
                    .to_string()
            })
    }

    #[test]
    fn campaign_lock_records_process_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path().join("campaign.json");
        let lock_path = temp.path().join("campaign.json.lock");

        let lock = CampaignLock::acquire(&state).expect("campaign lock");
        let record: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&lock_path).expect("read campaign lock"))
                .expect("lock metadata JSON");

        assert_eq!(record["schema_version"], 1);
        assert_eq!(record["host"], local_host());
        assert_eq!(record["pid"], std::process::id());
        assert!(record["process_start_ticks"].as_u64().is_some());
        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn campaign_lock_reclaims_dead_same_host_owner() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path().join("campaign.json");
        let lock_path = temp.path().join("campaign.json.lock");
        std::fs::write(
            &lock_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "host": local_host(),
                "pid": u32::MAX,
                "process_start_ticks": 1,
            }))
            .expect("lock JSON"),
        )
        .expect("stale lock");

        let lock = CampaignLock::acquire(&state).expect("reclaimed campaign lock");
        let record: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&lock_path).expect("read reclaimed lock"),
        )
        .expect("reclaimed lock metadata JSON");
        assert_eq!(record["pid"], std::process::id());
        drop(lock);
        assert!(!lock_path.exists());
    }

    #[test]
    fn campaign_lock_keeps_live_owner_exclusive() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path().join("campaign.json");
        let lock = CampaignLock::acquire(&state).expect("campaign lock");

        assert!(matches!(
            CampaignLock::acquire(&state),
            Err(CampaignError::Busy(_))
        ));

        drop(lock);
    }
}
