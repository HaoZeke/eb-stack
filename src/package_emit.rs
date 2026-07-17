//! Deterministic EasyBuild recipe-set emission from materialized package profiles.
//!
//! Emission targets conventional EasyBuild style used by easybuilders/easyconfigs:
//! - primary GitHub tag archives use `github_account` / `GITHUB_SOURCE` /
//!   `SOURCELOWER_TAR_GZ` when they match the package identity;
//! - dependency tuples omit the toolchain when the lock identity sits on the
//!   package toolchain or a hierarchy member (GCCcore/gompi under foss, …);
//! - cross-generation pins keep an explicit four-element tuple.

use crate::domain::Toolchain;
use crate::eb_parse::easyconfig_basename;
use crate::hierarchy::{hierarchy_for, hierarchy_member_rank};
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
    let toolchain_options = render_toolchain_options(&profile.toolchain_options);
    let toolchain_options_line = if toolchain_options.is_empty() {
        String::new()
    } else {
        format!("toolchainopts = {{{toolchain_options}}}\n")
    };
    let source_block = render_sources(
        &plan.package.name,
        &plan.package.version,
        &materialized.sources,
        &materialized.build.patches,
        materialized.build.source_root.as_deref(),
    );
    let patch_line = if materialized.build.patches.is_empty() {
        String::new()
    } else {
        let patches = materialized
            .build
            .patches
            .iter()
            .map(|patch| {
                format!(
                    "'{}'",
                    escape_single(patch.url.as_deref().unwrap_or(&patch.filename))
                )
            })
            .collect::<Vec<_>>();
        format!("patches = {}\n", render_multiline_list(&patches))
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
{source_prelude}\
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
        source_prelude = source_block.prelude,
        source_lines = source_block.sources,
        checksum_lines = source_block.checksums,
        build_dependencies = render_list(&build_dependencies),
        runtime_dependencies = render_list(&runtime_dependencies),
    );
    crate::eb_style::format_style(&rendered).text
}

/// Prefer conventional EasyBuild key order for common toolchainopts.
fn render_toolchain_options(options: &BTreeMap<String, bool>) -> String {
    const PREFERRED: &[&str] = &["usempi", "openmp", "pic", "opt", "debug"];
    let mut names = options.keys().cloned().collect::<Vec<_>>();
    names.sort_by(|left, right| {
        let left_rank = PREFERRED
            .iter()
            .position(|name| name == left)
            .unwrap_or(PREFERRED.len());
        let right_rank = PREFERRED
            .iter()
            .position(|name| name == right)
            .unwrap_or(PREFERRED.len());
        left_rank.cmp(&right_rank).then_with(|| left.cmp(right))
    });
    names
        .into_iter()
        .filter_map(|name| {
            options
                .get(&name)
                .map(|enabled| format!("'{name}': {}", python_bool(*enabled)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

struct SourceBlock {
    prelude: String,
    sources: String,
    checksums: String,
}

fn render_easyconfig_parameters(parameters: &BTreeMap<String, EasyconfigValue>) -> String {
    if parameters.is_empty() {
        return String::new();
    }
    let mut rendered = parameters
        .iter()
        .map(|(name, value)| match value {
            EasyconfigValue::Concat(fragments) => render_string_concat(name, &fragments.concat),
            _ => format!("{name} = {}", render_easyconfig_value(value, 0)),
        })
        .collect::<Vec<_>>()
        .join("\n");
    rendered.push_str("\n\n");
    rendered
}

fn render_string_concat(name: &str, fragments: &[String]) -> String {
    let mut fragments = fragments.iter();
    let Some(first) = fragments.next() else {
        return format!("{name} = ''");
    };
    let mut rendered = format!("{name} = '{}'", escape_single(first));
    for fragment in fragments {
        rendered.push_str(&format!("\n{name} += '{}'", escape_single(fragment)));
    }
    rendered
}

fn render_easyconfig_value(value: &EasyconfigValue, indentation: usize) -> String {
    match value {
        EasyconfigValue::Bool(value) => python_bool(*value).into(),
        EasyconfigValue::Integer(value) => value.to_string(),
        EasyconfigValue::String(value) => format!("'{}'", escape_single(value)),
        EasyconfigValue::List(values) => render_easyconfig_sequence(values, indentation),
        EasyconfigValue::Concat(fragments) => {
            format!("'{}'", escape_single(&fragments.concat.join("")))
        }
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

fn render_sources(
    package_name: &str,
    package_version: &str,
    source_artifacts: &[crate::package::SourceArtifact],
    patches: &[crate::package::PatchArtifact],
    source_root: Option<&str>,
) -> SourceBlock {
    let resolved = source_artifacts
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
            Some((source, url))
        })
        .collect::<Vec<_>>();

    let checksums = source_artifacts
        .iter()
        .filter_map(|source| source.sha256.as_ref())
        .chain(patches.iter().filter_map(|patch| patch.sha256.as_ref()))
        .map(|checksum| format!("'{}'", escape_single(checksum)))
        .collect::<Vec<_>>();
    let checksum_lines = format!("checksums = {}", render_multiline_list(&checksums));

    if let [(source, url)] = resolved.as_slice() {
        if source.target_directory.is_none() {
            if let Some(block) =
                try_render_github_primary(package_name, package_version, source, url)
            {
                return SourceBlock {
                    prelude: block.prelude,
                    sources: block.sources,
                    checksums: checksum_lines,
                };
            }
        }
    }

    let sources = resolved
        .iter()
        .map(|(source, url)| render_source(source, url, source_root))
        .collect::<Vec<_>>();
    SourceBlock {
        prelude: String::new(),
        sources: format!("sources = {}", render_multiline_list(&sources)),
        checksums: checksum_lines,
    }
}

/// Conventional EasyBuild form for a single primary GitHub tag archive.
fn try_render_github_primary(
    package_name: &str,
    package_version: &str,
    source: &crate::package::SourceArtifact,
    url: &str,
) -> Option<SourceBlock> {
    let (account, repo, tag) = parse_github_tag_archive(url)?;
    let namelower = package_name.to_ascii_lowercase();
    let uses_source_lower = repo.eq_ignore_ascii_case(&namelower);
    let download_filename = if tag == format!("v{package_version}") {
        "v%(version)s.tar.gz".to_string()
    } else if tag == package_version {
        "%(version)s.tar.gz".to_string()
    } else {
        format!("{tag}.tar.gz")
    };
    let filename_expr = if uses_source_lower && tag.ends_with(package_version) {
        "SOURCELOWER_TAR_GZ".to_string()
    } else if let Some(filename) = source.filename.as_deref() {
        format!("'{}'", escape_single(filename))
    } else {
        format!("'{repo}-{package_version}.tar.gz'")
    };

    let prelude = format!(
        "github_account = '{}'\nsource_urls = [GITHUB_SOURCE]\n",
        escape_single(&account)
    );
    // Same multi-line dict shape as staged multi-source entries so format_style
    // and review tooling see conventional indentation.
    let dict = [
        "{".to_string(),
        format!("    'download_filename': '{download_filename}',"),
        format!("    'filename': {filename_expr},"),
        "}".to_string(),
    ]
    .join("\n");
    Some(SourceBlock {
        prelude,
        sources: format!(
            "sources = {}",
            render_multiline_list(std::slice::from_ref(&dict))
        ),
        checksums: String::new(),
    })
}

/// Parse `https://github.com/{account}/{repo}/archive/refs/tags/{tag}.tar.gz`.
fn parse_github_tag_archive(url: &str) -> Option<(String, String, String)> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let mut parts = rest.split('/');
    let account = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if parts.next()? != "archive" || parts.next()? != "refs" || parts.next()? != "tags" {
        return None;
    }
    let file = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let tag = file
        .strip_suffix(".tar.gz")
        .or_else(|| file.strip_suffix(".tgz"))?
        .to_string();
    if account.is_empty() || repo.is_empty() || tag.is_empty() {
        return None;
    }
    Some((account, repo, tag))
}

fn render_source(
    source: &crate::package::SourceArtifact,
    url: &str,
    source_root: Option<&str>,
) -> String {
    let Some(target) = source.target_directory.as_deref() else {
        return format!("'{}'", escape_single(url));
    };
    let Some((source_url, download_filename)) = split_source_url(url) else {
        return format!("'{}'", escape_single(url));
    };
    if !safe_relative_target(target)
        || source_root.is_some_and(|root| !safe_relative_source_root(root))
        || !is_tar_archive(download_filename)
    {
        return format!("'{}'", escape_single(url));
    }

    let filename = source.filename.as_deref().unwrap_or(download_filename);
    let staging_directory = source_root.map_or_else(
        || format!("%(builddir)s/{target}"),
        |root| format!("%(builddir)s/{root}/{target}"),
    );
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
        "    'extract_cmd': 'mkdir -p {staging_directory} && ' +\n        \
                 'tar -xf %s -C {staging_directory} --strip-components=1',"
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

fn safe_relative_source_root(root: &str) -> bool {
    !root.is_empty()
        && !root.starts_with('/')
        && root.split('/').all(|component| {
            !matches!(component, "" | "." | "..")
                && component.chars().all(|ch| {
                    ch.is_ascii_alphanumeric()
                        || matches!(ch, '-' | '_' | '.' | '+' | '%' | '(' | ')')
                })
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
    package_toolchain: &Toolchain,
) -> String {
    // Conventional easyconfigs omit the toolchain when EasyBuild can resolve it
    // through the package hierarchy (e.g. Boost on GCCcore under foss). Keep an
    // explicit identity only for cross-generation or out-of-hierarchy pins.
    if dependency_requires_explicit_toolchain(dependency, package_toolchain) {
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

fn dependency_requires_explicit_toolchain(
    dependency: &crate::package::LockedDependency,
    package_toolchain: &Toolchain,
) -> bool {
    if dependency.toolchain == *package_toolchain {
        return false;
    }
    match hierarchy_for(package_toolchain, None) {
        Ok(hierarchy) => hierarchy_member_rank(&hierarchy, &dependency.toolchain).is_none(),
        // Unknown parent: keep the full identity rather than guessing.
        Err(_) => true,
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
