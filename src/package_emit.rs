//! Deterministic EasyBuild recipe-set emission from materialized package profiles.

use crate::eb_parse::easyconfig_basename;
use crate::package::{materialize_profile, PackagePlan, ProfileEnvironment, ProfileLock};
use std::collections::BTreeSet;
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

        emitted.push(EmittedEasyconfig {
            profile: output.profile.clone(),
            filename: easyconfig_basename(
                &plan.package.name,
                &plan.package.version,
                &plan.build.toolchain,
                (!materialized.versionsuffix.is_empty())
                    .then_some(materialized.versionsuffix.as_str()),
            ),
            text: render_easyconfig(plan, lock, &materialized.versionsuffix),
        });
    }
    Ok(emitted)
}

fn render_easyconfig(plan: &PackagePlan, lock: &ProfileLock, versionsuffix: &str) -> String {
    let profile = plan
        .profiles
        .iter()
        .find(|profile| profile.name == lock.profile)
        .expect("profile validated during materialization");
    let easyblock = plan.build.easyblock.as_deref().unwrap_or("ConfigureMake");
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
    let (source_lines, checksum_lines) = render_sources(plan);
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

    format!(
        "easyblock = '{easyblock}'\n\n\
name = '{name}'\n\
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
builddependencies = {build_dependencies}\n\n\
dependencies = {runtime_dependencies}\n\n\
sanity_check_paths = {{'files': [], 'dirs': []}}\n\n\
moduleclass = '{moduleclass}'\n",
        name = escape_single(&plan.package.name),
        version = escape_single(&plan.package.version),
        homepage = escape_single(homepage),
        description = description.replace("\"\"\"", "\\\"\\\"\\\""),
        toolchain_name = escape_single(&plan.build.toolchain.name),
        toolchain_version = escape_single(&plan.build.toolchain.version),
        build_dependencies = render_list(&build_dependencies),
        runtime_dependencies = render_list(&runtime_dependencies),
    )
}

fn render_sources(plan: &PackagePlan) -> (String, String) {
    let sources = plan
        .sources
        .iter()
        .filter_map(|source| {
            source
                .url
                .clone()
                .or_else(|| match (&source.git, &source.tag) {
                    (Some(git), Some(tag)) if git.contains("github.com") => Some(format!(
                        "{}/archive/refs/tags/{tag}.tar.gz",
                        git.trim_end_matches(".git")
                    )),
                    (Some(git), _) => Some(git.clone()),
                    _ => None,
                })
        })
        .map(|source| format!("'{}'", escape_single(&source)))
        .collect::<Vec<_>>();
    let checksums = plan
        .sources
        .iter()
        .filter_map(|source| source.sha256.as_ref())
        .map(|checksum| format!("'{}'", escape_single(checksum)))
        .collect::<Vec<_>>();
    (
        format!("sources = [{}]", sources.join(", ")),
        format!("checksums = [{}]", checksums.join(", ")),
    )
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
