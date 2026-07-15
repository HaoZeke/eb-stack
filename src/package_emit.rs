//! Deterministic EasyBuild recipe-set emission from materialized package profiles.

use crate::eb_parse::easyconfig_basename;
use crate::package::{
    is_easyconfig_parameter_name, materialize_profile, EasyconfigValue, PackagePlan,
    ProfileEnvironment, ProfileLock,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedEasyconfig {
    pub profile: String,
    pub filename: String,
    pub text: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PackageEmitError {
    #[error("profile materialization: {0}")]
    Materialize(String),
    #[error("profile {0} has no lock")]
    MissingLock(String),
    #[error("profile lock for {profile} does not match {package}-{version}")]
    LockIdentity {
        profile: String,
        package: String,
        version: String,
    },
    #[error("profile lock for {0} has a mismatched toolchain or versionsuffix")]
    LockConfiguration(String),
    #[error("invalid EasyBuild parameter name {0:?}")]
    InvalidEasyconfigParameter(String),
}

pub fn emit_profile_easyconfigs(
    plan: &PackagePlan,
    locks: &[ProfileLock],
) -> Result<Vec<EmittedEasyconfig>, PackageEmitError> {
    let mut emitted = Vec::new();
    for output in &plan.outputs {
        if output.stack != plan.build.toolchain.label() {
            return Err(PackageEmitError::LockConfiguration(output.profile.clone()));
        }
        let materialized =
            materialize_profile(plan, &output.profile, &ProfileEnvironment::default())
                .map_err(|error| PackageEmitError::Materialize(error.to_string()))?;
        let lock = locks
            .iter()
            .find(|lock| lock.profile == output.profile)
            .ok_or_else(|| PackageEmitError::MissingLock(output.profile.clone()))?;
        if lock.package != plan.package.name || lock.version != plan.package.version {
            return Err(PackageEmitError::LockIdentity {
                profile: output.profile.clone(),
                package: plan.package.name.clone(),
                version: plan.package.version.clone(),
            });
        }
        if lock.toolchain != plan.build.toolchain
            || lock.versionsuffix != materialized.versionsuffix
        {
            return Err(PackageEmitError::LockConfiguration(output.profile.clone()));
        }
        for name in plan
            .build
            .easyconfig_parameters
            .keys()
            .chain(materialized.profile.easyconfig_parameters.keys())
        {
            if !is_easyconfig_parameter_name(name) {
                return Err(PackageEmitError::InvalidEasyconfigParameter(name.clone()));
            }
        }

        emitted.push(EmittedEasyconfig {
            profile: output.profile.clone(),
            filename: easyconfig_basename(
                &plan.package.name,
                &plan.package.version,
                &plan.build.toolchain,
                (!materialized.versionsuffix.is_empty())
                    .then_some(materialized.versionsuffix.as_str()),
            ),
            text: render_easyconfig(plan, lock, &materialized),
        });
    }
    Ok(emitted)
}

fn render_easyconfig(
    plan: &PackagePlan,
    lock: &ProfileLock,
    materialized: &crate::package::MaterializedProfile,
) -> String {
    let profile = plan
        .profiles
        .iter()
        .find(|profile| profile.name == lock.profile)
        .expect("profile validated during materialization");
    let easyblock_line = plan
        .build
        .easyblock
        .as_deref()
        .map(|easyblock| format!("easyblock = '{}'\n\n", escape_single(easyblock)))
        .unwrap_or_default();
    let homepage = plan
        .package
        .homepage
        .as_deref()
        .unwrap_or("https://example.invalid/");
    let description = plan
        .package
        .description
        .as_deref()
        .unwrap_or("Package imported from an upstream recipe.");
    let versionsuffix = materialized.versionsuffix.as_str();
    let versionsuffix_line = if versionsuffix.is_empty() {
        String::new()
    } else {
        format!("versionsuffix = '{}'\n", escape_single(versionsuffix))
    };
    let toolchain_options = profile
        .toolchain_options
        .iter()
        .map(|(name, enabled)| format!("'{name}': {}", python_bool(*enabled)))
        .collect::<Vec<_>>()
        .join(", ");
    let toolchain_options_line = if toolchain_options.is_empty() {
        String::new()
    } else {
        format!("toolchainopts = {{{toolchain_options}}}\n")
    };
    let (source_lines, checksum_lines) = render_sources(&materialized.sources);
    let patch_line = if plan.build.patches.is_empty() {
        String::new()
    } else {
        format!(
            "patches = [{}]\n",
            plan.build
                .patches
                .iter()
                .map(|patch| format!("'{}'", escape_single(patch)))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut seen_options = BTreeSet::new();
    let config_options = plan
        .build
        .config_options
        .iter()
        .chain(profile.config_options.iter())
        .filter(|option| seen_options.insert((*option).clone()))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let config_line = if config_options.is_empty() {
        String::new()
    } else {
        format!("configopts = '{}'\n", escape_single(&config_options))
    };
    let mut easyconfig_parameters = plan.build.easyconfig_parameters.clone();
    easyconfig_parameters.extend(profile.easyconfig_parameters.clone());
    let easyconfig_parameter_lines = render_easyconfig_parameters(&easyconfig_parameters);
    let build_dependencies = lock
        .dependencies
        .iter()
        .filter(|dependency| dependency.build)
        .map(|dependency| render_dependency(dependency, &lock.toolchain))
        .collect::<Vec<_>>();
    let runtime_dependencies = lock
        .dependencies
        .iter()
        .filter(|dependency| !dependency.build)
        .map(|dependency| render_dependency(dependency, &lock.toolchain))
        .collect::<Vec<_>>();
    let moduleclass = plan.build.moduleclass.as_deref().unwrap_or("lib");

    let rendered = format!(
        "{easyblock_line}name = '{name}'\n\
version = '{version}'\n\
{versionsuffix_line}\n\
homepage = '{homepage}'\n\
description = \"\"\"{description}\"\"\"\n\n\
toolchain = {{'name': '{toolchain_name}', 'version': '{toolchain_version}'}}\n\
{toolchain_options_line}\n\
{source_lines}\n\
{checksum_lines}\n\
{patch_line}\
{config_line}\
{easyconfig_parameter_lines}\
builddependencies = {build_dependencies}\n\n\
dependencies = {runtime_dependencies}\n\n\
moduleclass = '{moduleclass}'\n",
        name = escape_single(&plan.package.name),
        version = escape_single(&plan.package.version),
        homepage = escape_single(homepage),
        description = description.replace("\"\"\"", "\\\"\\\"\\\""),
        toolchain_name = escape_single(&plan.build.toolchain.name),
        toolchain_version = escape_single(&plan.build.toolchain.version),
        build_dependencies = render_list(&build_dependencies),
        runtime_dependencies = render_list(&runtime_dependencies),
    );
    crate::eb_style::format_style(&rendered).text
}

fn render_easyconfig_parameters(parameters: &BTreeMap<String, EasyconfigValue>) -> String {
    if parameters.is_empty() {
        return String::new();
    }
    let mut rendered = parameters
        .iter()
        .map(|(name, value)| format!("{name} = {}", render_easyconfig_value(value, 0)))
        .collect::<Vec<_>>()
        .join("\n");
    rendered.push_str("\n\n");
    rendered
}

fn render_easyconfig_value(value: &EasyconfigValue, indentation: usize) -> String {
    match value {
        EasyconfigValue::Bool(value) => python_bool(*value).into(),
        EasyconfigValue::Integer(value) => value.to_string(),
        EasyconfigValue::String(value) => format!("'{}'", escape_single(value)),
        EasyconfigValue::List(values) => render_easyconfig_sequence(values, indentation),
        EasyconfigValue::Table(values) => render_easyconfig_table(values, indentation),
    }
}

fn render_easyconfig_sequence(values: &[EasyconfigValue], indentation: usize) -> String {
    if values.is_empty() {
        return "[]".into();
    }
    let item_indentation = indentation + 4;
    let prefix = " ".repeat(item_indentation);
    let suffix = " ".repeat(indentation);
    let items = values
        .iter()
        .map(|value| {
            format!(
                "{prefix}{},",
                render_easyconfig_value(value, item_indentation)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("[\n{items}\n{suffix}]")
}

fn render_easyconfig_table(
    values: &BTreeMap<String, EasyconfigValue>,
    indentation: usize,
) -> String {
    if values.is_empty() {
        return "{}".into();
    }
    let item_indentation = indentation + 4;
    let prefix = " ".repeat(item_indentation);
    let suffix = " ".repeat(indentation);
    let items = values
        .iter()
        .map(|(key, value)| {
            format!(
                "{prefix}'{}': {},",
                escape_single(key),
                render_easyconfig_value(value, item_indentation)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{{\n{items}\n{suffix}}}")
}

fn render_sources(source_artifacts: &[crate::package::SourceArtifact]) -> (String, String) {
    let sources = source_artifacts
        .iter()
        .filter_map(|source| {
            let url = source
                .url
                .clone()
                .or_else(|| match (&source.git, &source.tag) {
                    (Some(git), Some(tag)) if git.contains("github.com") => Some(format!(
                        "{}/archive/refs/tags/{tag}.tar.gz",
                        git.trim_end_matches(".git")
                    )),
                    (Some(git), _) => Some(git.clone()),
                    _ => None,
                })?;
            Some(render_source(source, &url))
        })
        .collect::<Vec<_>>();
    let checksums = source_artifacts
        .iter()
        .filter_map(|source| source.sha256.as_ref())
        .map(|checksum| format!("'{}'", escape_single(checksum)))
        .collect::<Vec<_>>();
    (
        format!("sources = {}", render_multiline_list(&sources)),
        format!("checksums = {}", render_multiline_list(&checksums)),
    )
}

fn render_source(source: &crate::package::SourceArtifact, url: &str) -> String {
    let Some(target) = source.target_directory.as_deref() else {
        return format!("'{}'", escape_single(url));
    };
    let Some((source_url, download_filename)) = split_source_url(url) else {
        return format!("'{}'", escape_single(url));
    };
    if !safe_relative_target(target) || !is_tar_archive(download_filename) {
        return format!("'{}'", escape_single(url));
    }

    let filename = source.filename.as_deref().unwrap_or(download_filename);
    let mut fields = vec!["{".to_string()];
    fields.push(format!(
        "    'source_urls': ['{}'],",
        escape_single(source_url)
    ));
    if source.filename.is_some() {
        fields.push(format!(
            "    'download_filename': '{}',",
            escape_single(download_filename)
        ));
    }
    fields.push(format!("    'filename': '{}',", escape_single(filename)));
    fields.push(format!(
        "    'extract_cmd': 'mkdir -p %(builddir)s/{target} && ' +\n        \
                 'tar -xf %s -C %(builddir)s/{target} --strip-components=1',"
    ));
    fields.push("}".to_string());
    fields.join("\n")
}

fn split_source_url(url: &str) -> Option<(&str, &str)> {
    let (directory, filename) = url.rsplit_once('/')?;
    (!filename.is_empty()).then_some((&url[..directory.len() + 1], filename))
}

fn safe_relative_target(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('/')
        && target.split('/').all(|component| {
            !matches!(component, "" | "." | "..")
                && component
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '+'))
        })
}

fn is_tar_archive(filename: &str) -> bool {
    let filename = filename.to_ascii_lowercase();
    [
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst",
    ]
    .iter()
    .any(|suffix| filename.ends_with(suffix))
}

fn render_dependency(
    dependency: &crate::package::LockedDependency,
    package_toolchain: &crate::domain::Toolchain,
) -> String {
    if dependency.toolchain != *package_toolchain {
        return format!(
            "('{}', '{}', '{}', ('{}', '{}'))",
            escape_single(&dependency.name),
            escape_single(&dependency.version),
            escape_single(dependency.versionsuffix.as_deref().unwrap_or("")),
            escape_single(&dependency.toolchain.name),
            escape_single(&dependency.toolchain.version),
        );
    }
    match dependency.versionsuffix.as_deref() {
        Some(versionsuffix) if !versionsuffix.is_empty() => format!(
            "('{}', '{}', '{}')",
            escape_single(&dependency.name),
            escape_single(&dependency.version),
            escape_single(versionsuffix)
        ),
        _ => format!(
            "('{}', '{}')",
            escape_single(&dependency.name),
            escape_single(&dependency.version)
        ),
    }
}

fn render_list(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".into();
    }
    format!("[\n    {},\n]", values.join(",\n    "))
}

fn render_multiline_list(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".into();
    }
    let entries = values
        .iter()
        .map(|value| {
            let value = value
                .lines()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{value},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("[\n{entries}\n]")
}

fn escape_single(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn python_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}
