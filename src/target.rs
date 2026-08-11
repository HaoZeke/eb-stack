//! Declarative transport, executor, runtime, and EasyBuild workload routing.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command;
use thiserror::Error;

/// Schema version of a target configuration layer.
pub const TARGET_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One TOML layer of build-target configuration.
pub struct TargetConfigLayer {
    /// Must equal [`TARGET_CONFIG_SCHEMA_VERSION`].
    pub schema_version: u32,
    #[serde(default)]
    /// Target definitions, merged by name across layers.
    pub targets: Vec<TargetPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A target definition, or an override of one from an earlier layer.
///
/// Each layer is optional so a site can override just the piece it cares
/// about, e.g. only the scheduler account.
pub struct TargetPatch {
    /// Target name, the key layers merge on.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// How commands reach the machine.
    pub transport: Option<TargetTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// How work is scheduled once there.
    pub executor: Option<TargetExecutor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// What the build runs inside.
    pub runtime: Option<TargetRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// How EasyBuild itself is invoked.
    pub easybuild: Option<EasyBuildWorkload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// How a command gets to the target machine.
pub enum TargetTransport {
    /// Run on this machine.
    Local,
    /// Run over SSH.
    Ssh {
        /// Host to connect to, as ssh understands it.
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Port, when it is not the default.
        port: Option<u16>,
        #[serde(default = "default_ssh_command")]
        /// SSH client program.
        command: String,
        #[serde(default = "default_rsync_command")]
        /// Program used to copy the bundle across.
        sync_command: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// How work is scheduled on the target.
pub enum TargetExecutor {
    /// Run immediately, in the foreground.
    Direct,
    /// Submit through Slurm and wait.
    Slurm {
        #[serde(default = "default_srun_command")]
        /// Submission program.
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Partition to submit to.
        partition: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Account to charge.
        account: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// CPUs to request.
        cpus: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Memory to request, in the scheduler's own units.
        memory: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Wall-clock limit, in the scheduler's own format.
        time: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Generic resources, e.g. GPUs.
        gres: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// What the build runs inside.
///
/// A container limits ABI contamination. It is not by itself a security
/// boundary, so mount only what the build needs.
pub enum TargetRuntime {
    /// Directly on the host.
    Host,
    /// Inside a Podman container.
    Podman {
        /// Image to run.
        image: String,
        #[serde(default = "default_podman_command")]
        /// Container program.
        command: String,
        #[serde(default)]
        /// Extra arguments passed to it.
        args: Vec<String>,
        #[serde(default)]
        /// Mounts, in the runtime's own syntax.
        mounts: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Working directory inside the container.
        workdir: Option<String>,
    },
    /// Inside a Docker container.
    Docker {
        /// Image to run.
        image: String,
        #[serde(default = "default_docker_command")]
        /// Container program.
        command: String,
        #[serde(default)]
        /// Extra arguments passed to it.
        args: Vec<String>,
        #[serde(default)]
        /// Mounts, in the runtime's own syntax.
        mounts: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Working directory inside the container.
        workdir: Option<String>,
    },
    /// EESSI apptainer via `eessi_container.sh` (CVMFS fuse in the image).
    ///
    /// Plan on the host; this runtime is the install backend. The host
    /// EasyBuild binary need not run inside the Debian 12 image.
    Eessi {
        #[serde(default = "default_eessi_command")]
        /// Path to `eessi_container.sh`.
        command: String,
        /// Host directory used as `--storage` (image cache and tmp).
        storage: String,
        #[serde(default = "default_eessi_access")]
        /// `ro` or `rw` CVMFS access.
        access: String,
        #[serde(default)]
        /// Extra binds in `src:dest:mode` form, joined as `--extra-bind-paths`.
        extra_bind_paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Resume an existing `--storage` session directory.
        resume: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// How EasyBuild is invoked on the target.
pub struct EasyBuildWorkload {
    /// The eb program.
    pub command: String,
    #[serde(default)]
    /// Robot search paths, in order.
    pub robot_paths: Vec<String>,
    /// Where builds are staged.
    pub work_root: String,
    /// Temporary space. Point this at disk, not a small tmpfs.
    pub tmp_root: String,
    #[serde(default)]
    /// Environment variables set for the build.
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A fully resolved target: every layer decided.
pub struct BuildTarget {
    /// Target name.
    pub name: String,
    /// How commands reach it.
    pub transport: TargetTransport,
    /// How work is scheduled.
    pub executor: TargetExecutor,
    /// What the build runs inside.
    pub runtime: TargetRuntime,
    /// How EasyBuild is invoked.
    pub easybuild: EasyBuildWorkload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A command as it will be run, after every layer has wrapped it.
///
/// Kept as program and arguments rather than a string so it can be executed
/// without a shell, and quoted back to a reader exactly as it ran.
pub struct CommandPlan {
    /// Program to execute.
    pub program: String,
    /// Arguments, already ordered.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One probe of a target layer and what it returned.
pub struct DoctorCheck {
    /// Layer probed: transport, executor, runtime, or easybuild.
    pub layer: String,
    /// The command run, so it can be repeated by hand.
    pub command: CommandPlan,
    /// Whether the probe passed.
    pub success: bool,
    /// Exit status, when the process produced one.
    pub exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The result of probing every layer of a target.
pub struct TargetDoctorReport {
    /// Target probed.
    pub target: String,
    /// One check per layer, in the order they were run.
    pub checks: Vec<DoctorCheck>,
}

impl TargetDoctorReport {
    /// Whether every check passed.
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.success)
    }
}

impl TargetConfigLayer {
    /// Parse a target layer from TOML text.
    pub fn from_toml_str(input: &str) -> Result<Self, TargetError> {
        let layer: Self = toml::from_str(input)?;
        layer.validate()?;
        Ok(layer)
    }

    /// Parse a target layer from a file.
    pub fn from_path(path: &Path) -> Result<Self, TargetError> {
        let input = std::fs::read_to_string(path)
            .map_err(|error| TargetError::Io(path.display().to_string(), error))?;
        Self::from_toml_str(&input)
    }

    fn validate(&self) -> Result<(), TargetError> {
        if self.schema_version != TARGET_CONFIG_SCHEMA_VERSION {
            return Err(TargetError::UnsupportedSchema(self.schema_version));
        }
        if self
            .targets
            .iter()
            .any(|target| target.name.trim().is_empty())
        {
            return Err(TargetError::EmptyName);
        }
        Ok(())
    }
}

/// Merge layers by target name and return the fully resolved targets.
///
/// A later layer overrides an earlier one field by field, so a site layer can
/// change the scheduler account without restating the transport.
pub fn resolve_target_layers(
    layers: &[TargetConfigLayer],
) -> Result<Vec<BuildTarget>, TargetError> {
    let mut order = Vec::new();
    let mut targets: HashMap<String, TargetPatch> = HashMap::new();
    for layer in layers {
        layer.validate()?;
        for patch in &layer.targets {
            if !targets.contains_key(&patch.name) {
                order.push(patch.name.clone());
                targets.insert(
                    patch.name.clone(),
                    TargetPatch {
                        name: patch.name.clone(),
                        transport: None,
                        executor: None,
                        runtime: None,
                        easybuild: None,
                    },
                );
            }
            let target = targets.get_mut(&patch.name).expect("target inserted");
            if patch.transport.is_some() {
                target.transport = patch.transport.clone();
            }
            if patch.executor.is_some() {
                target.executor = patch.executor.clone();
            }
            if patch.runtime.is_some() {
                target.runtime = patch.runtime.clone();
            }
            if patch.easybuild.is_some() {
                target.easybuild = patch.easybuild.clone();
            }
        }
    }

    order
        .into_iter()
        .map(|name| {
            let target = targets.remove(&name).expect("ordered target exists");
            Ok(BuildTarget {
                name: name.clone(),
                transport: target
                    .transport
                    .ok_or_else(|| TargetError::MissingLayer(name.clone(), "transport"))?,
                executor: target
                    .executor
                    .ok_or_else(|| TargetError::MissingLayer(name.clone(), "executor"))?,
                runtime: target
                    .runtime
                    .ok_or_else(|| TargetError::MissingLayer(name.clone(), "runtime"))?,
                easybuild: target
                    .easybuild
                    .ok_or(TargetError::MissingLayer(name, "easybuild"))?,
            })
        })
        .collect()
}

impl BuildTarget {
    /// Where the bundle will live on the target.
    pub fn staged_bundle_path(&self, local_bundle: &Path) -> String {
        match &self.transport {
            TargetTransport::Local => local_bundle.display().to_string(),
            TargetTransport::Ssh { .. } => {
                let name = local_bundle
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .unwrap_or("bundle");
                format!("{}/bundles/{name}", self.easybuild.work_root)
            }
        }
    }

    /// Copy the bundle to the target and return its path there.
    pub fn stage_bundle(&self, local_bundle: &Path) -> Result<String, TargetError> {
        let destination = self.staged_bundle_path(local_bundle);
        let TargetTransport::Ssh {
            host,
            port,
            command,
            sync_command,
        } = &self.transport
        else {
            return Ok(destination);
        };

        let mkdir = self.route_tokens(
            vec!["mkdir".into(), "-p".into(), destination.clone()],
            false,
        );
        let mkdir_output = mkdir.execute()?;
        if !mkdir_output.status.success() {
            return Err(TargetError::CommandFailed {
                program: mkdir.program,
                exit_code: mkdir_output.status.code(),
                stderr: String::from_utf8_lossy(&mkdir_output.stderr).into_owned(),
            });
        }

        let mut sync = Command::new(sync_command);
        sync.arg("-az");
        let remote_shell = match port {
            Some(port) => format!("{command} -p {port}"),
            None => command.clone(),
        };
        sync.arg("--rsh").arg(remote_shell);
        sync.arg(format!("{}/", local_bundle.display()));
        sync.arg(format!("{host}:{destination}/"));
        let output = sync
            .output()
            .map_err(|error| TargetError::Spawn(sync_command.clone(), error))?;
        if !output.status.success() {
            return Err(TargetError::CommandFailed {
                program: sync_command.clone(),
                exit_code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(destination)
    }

    /// The command that builds one recipe, wrapped by every layer.
    pub fn build_command(&self, recipe: &str) -> CommandPlan {
        self.build_command_with_robot_paths(recipe, &[])
    }

    /// Route an EasyBuild command with additional robot roots after configured roots.
    pub fn build_command_with_robot_paths(
        &self,
        recipe: &str,
        additional_robot_paths: &[String],
    ) -> CommandPlan {
        let mut tokens = vec!["env".to_string()];
        tokens.push(format!("EASYBUILD_TMPDIR={}", self.easybuild.tmp_root));
        tokens.extend(
            self.easybuild
                .environment
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        tokens.push(self.easybuild.command.clone());
        let mut robot_paths = self.easybuild.robot_paths.clone();
        for path in additional_robot_paths {
            if !robot_paths.contains(path) {
                robot_paths.push(path.clone());
            }
        }
        if !robot_paths.is_empty() {
            tokens.push(format!("--robot={}", robot_paths.join(":")));
        }
        tokens.push(format!("--buildpath={}/build", self.easybuild.work_root));
        tokens.push(recipe.to_string());
        self.route_tokens(self.runtime_tokens(tokens), true)
    }

    /// The command that runs a verification program on the target.
    pub fn verification_command(&self, program: &str, args: &[String]) -> CommandPlan {
        let mut tokens = vec!["env".to_string()];
        tokens.extend(
            self.easybuild
                .environment
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        tokens.push(program.to_string());
        tokens.extend(args.iter().cloned());
        self.route_tokens(self.runtime_tokens(tokens), true)
    }

    fn runtime_tokens(&self, command: Vec<String>) -> Vec<String> {
        match &self.runtime {
            TargetRuntime::Host => command,
            TargetRuntime::Podman {
                image,
                command: runtime,
                args,
                mounts,
                workdir,
            }
            | TargetRuntime::Docker {
                image,
                command: runtime,
                args,
                mounts,
                workdir,
            } => {
                let mut tokens = vec![runtime.clone(), "run".into(), "--rm".into()];
                tokens.extend(args.iter().cloned());
                for mount in mounts {
                    tokens.push("--volume".into());
                    tokens.push(mount.clone());
                }
                if let Some(workdir) = workdir {
                    tokens.push("--workdir".into());
                    tokens.push(workdir.clone());
                }
                tokens.push(image.clone());
                tokens.extend(command);
                tokens
            }
            TargetRuntime::Eessi {
                command: runtime,
                storage,
                access,
                extra_bind_paths,
                resume,
            } => {
                let mut tokens = vec![
                    runtime.clone(),
                    "--mode".into(),
                    "exec".into(),
                    "--access".into(),
                    access.clone(),
                    "--storage".into(),
                    storage.clone(),
                ];
                if let Some(resume) = resume {
                    tokens.push("--resume".into());
                    tokens.push(resume.clone());
                }
                if !extra_bind_paths.is_empty() {
                    tokens.push("--extra-bind-paths".into());
                    tokens.push(extra_bind_paths.join(","));
                }
                tokens.push("--".into());
                tokens.extend(command);
                tokens
            }
        }
    }

    fn executor_tokens(&self, command: Vec<String>) -> Vec<String> {
        match &self.executor {
            TargetExecutor::Direct => command,
            TargetExecutor::Slurm {
                command: srun,
                partition,
                account,
                cpus,
                memory,
                time,
                gres,
            } => {
                let mut tokens = vec![srun.clone()];
                push_option(&mut tokens, "--partition", partition.as_deref());
                push_option(&mut tokens, "--account", account.as_deref());
                if let Some(cpus) = cpus {
                    tokens.push("--cpus-per-task".into());
                    tokens.push(cpus.to_string());
                }
                push_option(&mut tokens, "--mem", memory.as_deref());
                push_option(&mut tokens, "--time", time.as_deref());
                push_option(&mut tokens, "--gres", gres.as_deref());
                tokens.push("--".into());
                tokens.extend(command);
                tokens
            }
        }
    }

    fn route_tokens(&self, command: Vec<String>, use_executor: bool) -> CommandPlan {
        let tokens = if use_executor {
            self.executor_tokens(command)
        } else {
            command
        };
        match &self.transport {
            TargetTransport::Local => CommandPlan::from_tokens(tokens),
            TargetTransport::Ssh {
                host,
                port,
                command,
                ..
            } => {
                let mut args = Vec::new();
                if let Some(port) = port {
                    args.push("-p".into());
                    args.push(port.to_string());
                }
                args.push(host.clone());
                args.push("--".into());
                args.push(shell_join(&tokens));
                CommandPlan {
                    program: command.clone(),
                    args,
                }
            }
        }
    }
}

impl CommandPlan {
    fn from_tokens(mut tokens: Vec<String>) -> Self {
        let program = if tokens.is_empty() {
            "true".into()
        } else {
            tokens.remove(0)
        };
        Self {
            program,
            args: tokens,
        }
    }

    /// Run the command and capture its output.
    ///
    /// Executed without a shell, so nothing in the plan is re-parsed for
    /// metacharacters.
    pub fn execute(&self) -> Result<std::process::Output, TargetError> {
        Command::new(&self.program)
            .args(&self.args)
            .output()
            .map_err(|error| TargetError::Spawn(self.program.clone(), error))
    }
}

/// Probe every layer of a target and report what answered.
///
/// Run this before a campaign: a target that cannot be reached should fail
/// here rather than halfway through a build.
pub fn doctor_target(target: &BuildTarget) -> Result<TargetDoctorReport, TargetError> {
    let transport = target.route_tokens(vec!["true".into()], false);
    let executor = target.route_tokens(vec!["true".into()], true);
    let runtime_program = match &target.runtime {
        TargetRuntime::Host => vec!["true".into()],
        TargetRuntime::Podman { command, .. } | TargetRuntime::Docker { command, .. } => {
            vec![command.clone(), "--version".into()]
        }
        TargetRuntime::Eessi { command, .. } => vec![command.clone(), "--help".into()],
    };
    let runtime = target.route_tokens(runtime_program, true);
    let easybuild = target.route_tokens(
        target.runtime_tokens(vec![target.easybuild.command.clone(), "--version".into()]),
        true,
    );
    let checks = [
        ("transport", transport),
        ("executor", executor),
        ("runtime", runtime),
        ("easybuild", easybuild),
    ]
    .into_iter()
    .map(|(layer, command)| run_doctor_check(layer, command))
    .collect::<Result<Vec<_>, _>>()?;
    Ok(TargetDoctorReport {
        target: target.name.clone(),
        checks,
    })
}

fn run_doctor_check(layer: &str, command: CommandPlan) -> Result<DoctorCheck, TargetError> {
    let output = command.execute()?;
    Ok(DoctorCheck {
        layer: layer.into(),
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        command,
    })
}

fn push_option(tokens: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        tokens.push(flag.into());
        tokens.push(value.into());
    }
}

fn shell_join(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|token| shell_quote(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(token: &str) -> String {
    if !token.is_empty()
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/_.:=,@%+-".contains(character))
    {
        token.to_string()
    } else {
        format!("'{}'", token.replace('\'', "'\\''"))
    }
}

fn default_ssh_command() -> String {
    "ssh".into()
}

fn default_srun_command() -> String {
    "srun".into()
}

fn default_rsync_command() -> String {
    "rsync".into()
}

fn default_podman_command() -> String {
    "podman".into()
}

fn default_docker_command() -> String {
    "docker".into()
}

fn default_eessi_command() -> String {
    "eessi_container.sh".into()
}

fn default_eessi_access() -> String {
    "ro".into()
}

#[derive(Debug, Error)]
/// Why a target could not be configured or reached.
pub enum TargetError {
    #[error("unsupported target config schema version {0}")]
    /// The layer declares a schema this build does not read.
    UnsupportedSchema(u32),
    #[error("target config TOML: {0}")]
    /// The layer is not valid TOML.
    Toml(#[from] toml::de::Error),
    #[error("read target config {0}: {1}")]
    /// A target file could not be read.
    Io(String, std::io::Error),
    #[error("target name cannot be empty")]
    /// A target has no name, so no layer could merge onto it.
    EmptyName,
    #[error("target {0} has no {1} layer")]
    /// A target left a layer undefined that has no default.
    MissingLayer(String, &'static str),
    #[error("spawn target command {0}: {1}")]
    /// The command could not be started.
    Spawn(String, std::io::Error),
    #[error("target command {program} failed with exit {exit_code:?}: {stderr}")]
    /// The command ran and failed.
    CommandFailed {
        /// Program that failed.
        program: String,
        /// Exit status, when there was one.
        exit_code: Option<i32>,
        /// What it printed on standard error.
        stderr: String,
    },
}
