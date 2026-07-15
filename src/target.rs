//! Declarative transport, executor, runtime, and EasyBuild workload routing.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::Command;
use thiserror::Error;

pub const TARGET_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConfigLayer {
    pub schema_version: u32,
    #[serde(default)]
    pub targets: Vec<TargetPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetPatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<TargetTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<TargetExecutor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<TargetRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easybuild: Option<EasyBuildWorkload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TargetTransport {
    Local,
    Ssh {
        host: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        #[serde(default = "default_ssh_command")]
        command: String,
        #[serde(default = "default_rsync_command")]
        sync_command: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TargetExecutor {
    Direct,
    Slurm {
        #[serde(default = "default_srun_command")]
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partition: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cpus: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        memory: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gres: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum TargetRuntime {
    Host,
    Podman {
        image: String,
        #[serde(default = "default_podman_command")]
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        mounts: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
    Docker {
        image: String,
        #[serde(default = "default_docker_command")]
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        mounts: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EasyBuildWorkload {
    pub command: String,
    #[serde(default)]
    pub robot_paths: Vec<String>,
    pub work_root: String,
    pub tmp_root: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildTarget {
    pub name: String,
    pub transport: TargetTransport,
    pub executor: TargetExecutor,
    pub runtime: TargetRuntime,
    pub easybuild: EasyBuildWorkload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandPlan {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorCheck {
    pub layer: String,
    pub command: CommandPlan,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDoctorReport {
    pub target: String,
    pub checks: Vec<DoctorCheck>,
}

impl TargetDoctorReport {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|check| check.success)
    }
}

impl TargetConfigLayer {
    pub fn from_toml_str(input: &str) -> Result<Self, TargetError> {
        let layer: Self = toml::from_str(input)?;
        layer.validate()?;
        Ok(layer)
    }

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

    pub fn build_command(&self, recipe: &str) -> CommandPlan {
        let mut tokens = vec!["env".to_string()];
        tokens.push(format!("EASYBUILD_TMPDIR={}", self.easybuild.tmp_root));
        tokens.extend(
            self.easybuild
                .environment
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        tokens.push(self.easybuild.command.clone());
        if !self.easybuild.robot_paths.is_empty() {
            tokens.push(format!("--robot={}", self.easybuild.robot_paths.join(":")));
        }
        tokens.push(format!("--buildpath={}/build", self.easybuild.work_root));
        tokens.push(recipe.to_string());
        self.route_tokens(self.runtime_tokens(tokens), true)
    }

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

    pub fn execute(&self) -> Result<std::process::Output, TargetError> {
        Command::new(&self.program)
            .args(&self.args)
            .output()
            .map_err(|error| TargetError::Spawn(self.program.clone(), error))
    }
}

pub fn doctor_target(target: &BuildTarget) -> Result<TargetDoctorReport, TargetError> {
    let transport = target.route_tokens(vec!["true".into()], false);
    let executor = target.route_tokens(vec!["true".into()], true);
    let runtime_program = match &target.runtime {
        TargetRuntime::Host => vec!["true".into()],
        TargetRuntime::Podman { command, .. } | TargetRuntime::Docker { command, .. } => {
            vec![command.clone(), "--version".into()]
        }
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

#[derive(Debug, Error)]
pub enum TargetError {
    #[error("unsupported target config schema version {0}")]
    UnsupportedSchema(u32),
    #[error("target config TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("read target config {0}: {1}")]
    Io(String, std::io::Error),
    #[error("target name cannot be empty")]
    EmptyName,
    #[error("target {0} has no {1} layer")]
    MissingLayer(String, &'static str),
    #[error("spawn target command {0}: {1}")]
    Spawn(String, std::io::Error),
    #[error("target command {program} failed with exit {exit_code:?}: {stderr}")]
    CommandFailed {
        program: String,
        exit_code: Option<i32>,
        stderr: String,
    },
}
