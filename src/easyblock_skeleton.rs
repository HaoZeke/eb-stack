//! When a package owes an easyblock, and what a first draft of one looks like.
//!
//! A package with variant options, a binary name that follows them, or
//! hand-written test and sanity parameters carries logic that an easyconfig
//! can only repeat as data, once per profile. That logic belongs in a
//! software-specific easyblock, and the easyconfigs then lean on it.
//!
//! [`owed_easyblock`] decides whether a plan owes one. [`render`] turns a plan
//! into the Python module a human then completes, and [`apply_takeover`]
//! removes from the plan what the module now owns, so the emitted recipes
//! carry no variant flag the easyblock sets itself.

use crate::eb_easyblock::{defined_classes, encode_class_name};
use crate::package::{
    is_easyconfig_parameter_name, EasyblockArtifact, EasyconfigValue, PackagePlan, ProductProfile,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Longest line a rendered module may carry, flake8's usual project setting.
const MAX_LINE: usize = 120;

/// Generic easyblock a skeleton derives from when the plan names none.
const DEFAULT_BASE: &str = "CMakeMake";

/// Easyconfig parameters that hold hand-written test and sanity logic.
const OWNED_PARAMETERS: &[&str] = &[
    "sanity_check_paths",
    "sanity_check_commands",
    "test_cmd",
    "testopts",
    "pretestopts",
];

/// Parameters the rendered `test_step` reads.
const TEST_PARAMETERS: &[&str] = &["test_cmd", "testopts", "pretestopts"];

/// The EasyBuild copyright header every easyblock module opens with.
const COPYRIGHT_HEADER: &[&str] = &[
    "##",
    "# Copyright 2026 Ghent University",
    "#",
    "# This file is part of EasyBuild,",
    "# originally created by the HPC team of Ghent University (http://ugent.be/hpc/en),",
    "# with support of Ghent University (http://ugent.be/hpc),",
    "# the Flemish Supercomputer Centre (VSC) (https://www.vscentrum.be),",
    "# Flemish Research Foundation (FWO) (http://www.fwo.be/en)",
    "# and the Department of Economy, Science and Innovation (EWI) (http://www.ewi-vlaanderen.be/en).",
    "#",
    "# https://github.com/easybuilders/easybuild",
    "#",
    "# EasyBuild is free software: you can redistribute it and/or modify",
    "# it under the terms of the GNU General Public License as published by",
    "# the Free Software Foundation v2.",
    "#",
    "# EasyBuild is distributed in the hope that it will be useful,",
    "# but WITHOUT ANY WARRANTY; without even the implied warranty of",
    "# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the",
    "# GNU General Public License for more details.",
    "#",
    "# You should have received a copy of the GNU General Public License",
    "# along with EasyBuild.  If not, see <http://www.gnu.org/licenses/>.",
];

/// Why a plan owes a software-specific easyblock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwedEasyblock {
    /// Class EasyBuild derives from the software name, e.g. `EB_WRF` for `WRF`.
    pub class_name: String,
    /// Build-option differences between profiles, each naming the option and
    /// the profiles that carry it.
    pub variant_options: Vec<String>,
    /// Hand-written test and sanity parameters, each naming the parameter and
    /// the scope that sets it.
    pub parameters: Vec<String>,
    /// Binary names that differ between profiles, each naming the profile.
    pub binaries: Vec<String>,
}

impl OwedEasyblock {
    /// The profiles and parameters that would move into the class, one line
    /// per trigger.
    pub fn evidence(&self) -> String {
        let mut lines = Vec::new();
        if !self.variant_options.is_empty() {
            lines.push(format!(
                "config_options that differ between profiles and would move into {}: {}",
                self.class_name,
                self.variant_options.join("; ")
            ));
        }
        if !self.parameters.is_empty() {
            lines.push(format!(
                "easyconfig parameters that would move into {}: {}",
                self.class_name,
                self.parameters.join(", ")
            ));
        }
        if !self.binaries.is_empty() {
            lines.push(format!(
                "binary names that differ between profiles: {}",
                self.binaries.join("; ")
            ));
        }
        lines.join("\n")
    }
}

/// The class EasyBuild derives from the software name.
pub fn derived_class(plan: &PackagePlan) -> String {
    encode_class_name(&plan.package.name)
}

/// File name of the module that holds the derived class, e.g. `wrf.py`.
pub fn module_filename(plan: &PackagePlan) -> String {
    let stem = plan
        .package
        .name
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("{stem}.py")
}

/// Whether a shipped module defines `class`.
///
/// Uses the classes recorded on the artifact when the plan has been checked,
/// and parses the module from disk otherwise.
pub fn ships_class(plan: &PackagePlan, class: &str) -> bool {
    plan.build
        .easyblocks
        .iter()
        .any(|artifact| artifact_classes(artifact).iter().any(|name| name == class))
}

fn artifact_classes(artifact: &EasyblockArtifact) -> Vec<String> {
    if !artifact.classes.is_empty() {
        return artifact
            .classes
            .iter()
            .map(|class| class.name.clone())
            .collect();
    }
    let source = artifact
        .resolved_source
        .clone()
        .or_else(|| artifact.source.as_deref().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(&artifact.filename));
    std::fs::read_to_string(&source)
        .ok()
        .and_then(|text| defined_classes(&text, &artifact.filename).ok())
        .map(|classes| classes.into_iter().map(|class| class.name).collect())
        .unwrap_or_default()
}

/// Whether the recipe resolves to a generic easyblock.
///
/// The plan names no easyblock or one other than the derived class, and no
/// shipped module defines the derived class.
pub fn has_generic_easyblock(plan: &PackagePlan) -> bool {
    let class = derived_class(plan);
    plan.build.easyblock.as_deref() != Some(class.as_str()) && !ships_class(plan, &class)
}

/// What a generic-easyblock plan owes, or `None` when no trigger applies.
///
/// Three triggers, any one of which suffices:
///
/// 1. two or more profiles whose `config_options` differ from each other;
/// 2. test or sanity parameters written into the easyconfig, namely
///    `sanity_check_paths`, `sanity_check_commands`, `test_cmd`, `testopts`,
///    `pretestopts`, or `runtest` as a string command;
/// 3. a binary named in the sanity files or commands that differs between
///    profiles.
pub fn owed_easyblock(plan: &PackagePlan) -> Option<OwedEasyblock> {
    if !has_generic_easyblock(plan) {
        return None;
    }
    let variant_options = differing_options(plan)
        .into_iter()
        .map(|(option, profiles)| format!("{option} ({})", profiles.join(", ")))
        .collect::<Vec<_>>();
    let parameters = owned_parameter_evidence(plan);
    let binaries = differing_binaries(plan);
    if variant_options.is_empty() && parameters.is_empty() && binaries.is_empty() {
        return None;
    }
    Some(OwedEasyblock {
        class_name: derived_class(plan),
        variant_options,
        parameters,
        binaries,
    })
}

fn default_profile(plan: &PackagePlan) -> Option<&ProductProfile> {
    plan.profiles
        .iter()
        .find(|profile| profile.default)
        .or_else(|| plan.profiles.first())
}

fn feature_value(profile: &ProductProfile, feature: &str) -> bool {
    profile.features.get(feature).copied().unwrap_or(false)
}

fn feature_names(plan: &PackagePlan) -> BTreeSet<String> {
    plan.profiles
        .iter()
        .flat_map(|profile| profile.features.keys().cloned())
        .collect()
}

fn merged_parameters(
    plan: &PackagePlan,
    profile: &ProductProfile,
) -> BTreeMap<String, EasyconfigValue> {
    let mut merged = plan.build.easyconfig_parameters.clone();
    merged.extend(profile.easyconfig_parameters.clone());
    merged
}

/// The part of a configure option that names it: `-DQMC_COMPLEX=` for
/// `-DQMC_COMPLEX=ON`, and the whole option when it carries no value.
fn option_marker(option: &str) -> String {
    match option.find('=') {
        Some(index) => option[..=index].to_string(),
        None => option.to_string(),
    }
}

/// Options some profiles carry and others lack, with the profiles that carry
/// them. Empty for a plan with fewer than two profiles.
fn differing_options(plan: &PackagePlan) -> BTreeMap<String, Vec<String>> {
    let mut carriers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if plan.profiles.len() < 2 {
        return carriers;
    }
    for profile in &plan.profiles {
        for option in &profile.config_options {
            let names = carriers.entry(option.clone()).or_default();
            if !names.contains(&profile.name) {
                names.push(profile.name.clone());
            }
        }
    }
    carriers.retain(|_, names| names.len() < plan.profiles.len());
    carriers
}

fn owned_parameter_evidence(plan: &PackagePlan) -> Vec<String> {
    let mut found = Vec::new();
    collect_owned(&plan.build.easyconfig_parameters, "build", &mut found);
    for profile in &plan.profiles {
        collect_owned(
            &profile.easyconfig_parameters,
            &format!("profile {}", profile.name),
            &mut found,
        );
    }
    found
}

fn collect_owned(
    parameters: &BTreeMap<String, EasyconfigValue>,
    scope: &str,
    found: &mut Vec<String>,
) {
    for (name, value) in parameters {
        let owned = OWNED_PARAMETERS.contains(&name.as_str())
            || (name == "runtest" && matches!(value, EasyconfigValue::String(_)));
        if owned {
            found.push(format!("{name} ({scope})"));
        }
    }
}

fn string_list(value: Option<&EasyconfigValue>) -> Vec<String> {
    match value {
        Some(EasyconfigValue::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                EasyconfigValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        Some(EasyconfigValue::String(text)) => vec![text.clone()],
        _ => Vec::new(),
    }
}

fn sanity_table_list(
    parameters: &BTreeMap<String, EasyconfigValue>,
    key: &str,
) -> Option<Vec<String>> {
    match parameters.get("sanity_check_paths") {
        Some(EasyconfigValue::Table(table)) => Some(string_list(table.get(key))),
        _ => None,
    }
}

/// Names of the `bin/` files the sanity check lists, in order.
fn binary_files(parameters: &BTreeMap<String, EasyconfigValue>) -> Vec<String> {
    sanity_table_list(parameters, "files")
        .unwrap_or_default()
        .iter()
        .filter_map(|file| file.strip_prefix("bin/").map(str::to_string))
        .collect()
}

/// Binaries the sanity files and commands mention.
fn binary_set(parameters: &BTreeMap<String, EasyconfigValue>) -> BTreeSet<String> {
    let mut names = binary_files(parameters)
        .into_iter()
        .collect::<BTreeSet<_>>();
    for command in string_list(parameters.get("sanity_check_commands")) {
        if let Some(program) = command.split_whitespace().next() {
            names.insert(program.to_string());
        }
    }
    names
}

fn differing_binaries(plan: &PackagePlan) -> Vec<String> {
    let sets = plan
        .profiles
        .iter()
        .map(|profile| {
            (
                profile.name.clone(),
                binary_set(&merged_parameters(plan, profile)),
            )
        })
        .filter(|(_, set)| !set.is_empty())
        .collect::<Vec<_>>();
    let Some((_, first)) = sets.first() else {
        return Vec::new();
    };
    if sets.iter().all(|(_, set)| set == first) {
        return Vec::new();
    }
    sets.iter()
        .map(|(name, set)| {
            format!(
                "{} ({name})",
                set.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect()
}

/// A feature and the configure options it switches.
struct FeatureMapping {
    feature: String,
    default: bool,
    /// Options present where the feature is true and absent where it is false.
    on: Vec<String>,
    /// Options present where the feature is false and absent where it is
    /// true, kept only when they set the same option as one in `on`.
    off: Vec<String>,
}

fn feature_mappings(plan: &PackagePlan) -> Vec<FeatureMapping> {
    let default = default_profile(plan);
    let mut assigned: BTreeSet<String> = BTreeSet::new();
    let mut mappings = Vec::new();
    for feature in feature_names(plan) {
        let enabled = plan
            .profiles
            .iter()
            .filter(|profile| feature_value(profile, &feature))
            .collect::<Vec<_>>();
        let disabled = plan
            .profiles
            .iter()
            .filter(|profile| !feature_value(profile, &feature))
            .collect::<Vec<_>>();
        let mut on: Vec<String> = Vec::new();
        let mut off: Vec<String> = Vec::new();
        if let (Some(first_enabled), Some(first_disabled)) = (enabled.first(), disabled.first()) {
            for option in &first_enabled.config_options {
                let everywhere = enabled
                    .iter()
                    .all(|profile| profile.config_options.contains(option));
                let nowhere = !disabled
                    .iter()
                    .any(|profile| profile.config_options.contains(option));
                if everywhere && nowhere && !assigned.contains(option) && !on.contains(option) {
                    on.push(option.clone());
                }
            }
            for option in &first_disabled.config_options {
                let everywhere = disabled
                    .iter()
                    .all(|profile| profile.config_options.contains(option));
                let nowhere = !enabled
                    .iter()
                    .any(|profile| profile.config_options.contains(option));
                if everywhere && nowhere && !assigned.contains(option) && !off.contains(option) {
                    off.push(option.clone());
                }
            }
            let markers = on
                .iter()
                .map(|option| option_marker(option))
                .collect::<BTreeSet<_>>();
            off.retain(|option| markers.contains(&option_marker(option)));
        }
        assigned.extend(on.iter().cloned());
        assigned.extend(off.iter().cloned());
        mappings.push(FeatureMapping {
            default: default.is_some_and(|profile| feature_value(profile, &feature)),
            feature,
            on,
            off,
        });
    }
    mappings
}

/// A binary that one profile names differently from the default profile.
struct BinaryRename {
    from: String,
    to: String,
    feature: String,
    value: bool,
}

fn binary_renames(plan: &PackagePlan) -> Vec<BinaryRename> {
    let mut renames: Vec<BinaryRename> = Vec::new();
    let Some(default) = default_profile(plan) else {
        return renames;
    };
    let base = binary_files(&merged_parameters(plan, default));
    if base.is_empty() {
        return renames;
    }
    let features = feature_names(plan);
    for profile in plan
        .profiles
        .iter()
        .filter(|profile| profile.name != default.name)
    {
        let names = binary_files(&merged_parameters(plan, profile));
        if names.len() != base.len() {
            continue;
        }
        let Some(feature) = features.iter().find(|feature| {
            feature_value(profile, feature.as_str()) != feature_value(default, feature.as_str())
        }) else {
            continue;
        };
        let value = feature_value(profile, feature.as_str());
        for (from, to) in base.iter().zip(names.iter()) {
            if from == to {
                continue;
            }
            let known = renames.iter().any(|rename| {
                rename.from == *from
                    && rename.to == *to
                    && rename.feature == *feature
                    && rename.value == value
            });
            if !known {
                renames.push(BinaryRename {
                    from: from.clone(),
                    to: to.clone(),
                    feature: feature.clone(),
                    value,
                });
            }
        }
    }
    renames
}

/// Remove from the plan what the rendered easyblock owns.
///
/// Every option the class sets from a feature leaves the build-level and the
/// profile `config_options`, so the emitted recipes carry no `-D` flag the
/// easyblock passes itself. Each non-default profile gains one boolean
/// easyconfig parameter per feature that drives the class and differs from
/// the default profile, which is how a recipe selects its variant.
pub fn apply_takeover(plan: &mut PackagePlan) {
    let mappings = feature_mappings(plan);
    let renames = binary_renames(plan);
    let markers = mappings
        .iter()
        .flat_map(|mapping| mapping.on.iter())
        .map(|option| option_marker(option))
        .collect::<BTreeSet<_>>();
    let driving = mappings
        .iter()
        .filter(|mapping| !mapping.on.is_empty())
        .map(|mapping| mapping.feature.clone())
        .chain(renames.iter().map(|rename| rename.feature.clone()))
        .collect::<BTreeSet<_>>();
    let defaults = mappings
        .iter()
        .map(|mapping| (mapping.feature.clone(), mapping.default))
        .collect::<BTreeMap<_, _>>();
    let default_name = default_profile(plan).map(|profile| profile.name.clone());

    plan.build
        .config_options
        .retain(|option| !markers.contains(&option_marker(option)));
    for profile in &mut plan.profiles {
        profile
            .config_options
            .retain(|option| !markers.contains(&option_marker(option)));
        if default_name.as_deref() == Some(profile.name.as_str()) {
            continue;
        }
        for feature in &driving {
            let value = feature_value(profile, feature);
            let default = defaults.get(feature).copied().unwrap_or(false);
            if value != default {
                profile
                    .easyconfig_parameters
                    .insert(feature.clone(), EasyconfigValue::Bool(value));
            }
        }
    }
}

/// A Python string literal for `text`, single-quoted unless that needs an
/// escape the double quote avoids.
fn quote(text: &str) -> String {
    let delimiter = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push(delimiter);
    for character in text.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            other if other == delimiter => {
                quoted.push('\\');
                quoted.push(other);
            }
            other => quoted.push(other),
        }
    }
    quoted.push(delimiter);
    quoted
}

/// Quoted pieces of `text`, each short enough to fit `available` columns.
fn chunk_literals(text: &str, available: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        let mut candidate = current.clone();
        candidate.push(character);
        if !current.is_empty() && quote(&candidate).chars().count() > available {
            chunks.push(quote(&current));
            current.clear();
        }
        current.push(character);
    }
    if !current.is_empty() {
        chunks.push(quote(&current));
    }
    chunks
}

/// Push `leading`, the literal for `text`, and `trailing` as one line, or as
/// implicitly concatenated literals inside parentheses when one line is too
/// long.
fn push_string(lines: &mut Vec<String>, indent: usize, leading: &str, text: &str, trailing: &str) {
    let pad = " ".repeat(indent);
    let whole = format!("{pad}{leading}{}{trailing}", quote(text));
    if whole.chars().count() <= MAX_LINE {
        lines.push(whole);
        return;
    }
    let lead = leading.chars().count();
    let continuation = " ".repeat(indent + lead + 1);
    let available = MAX_LINE
        .saturating_sub(indent + lead + 1 + trailing.chars().count() + 1)
        .max(8);
    let chunks = chunk_literals(text, available);
    let last = chunks.len().saturating_sub(1);
    for (index, chunk) in chunks.iter().enumerate() {
        let mut line = if index == 0 {
            format!("{pad}{leading}({chunk}")
        } else {
            format!("{continuation}{chunk}")
        };
        if index == last {
            line.push(')');
            line.push_str(trailing);
        }
        lines.push(line);
    }
}

/// Greedy word wrap of `text` to `width` columns.
fn wrap(text: &str, width: usize, first: &str, rest: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = first.to_string();
    let mut empty = true;
    for word in text.split_whitespace() {
        let extra = if empty { 0 } else { 1 };
        if !empty && current.chars().count() + extra + word.chars().count() > width {
            lines.push(current);
            current = rest.to_string();
            empty = true;
        }
        if !empty {
            current.push(' ');
        }
        current.push_str(word);
        empty = false;
    }
    if !empty {
        lines.push(current);
    }
    lines
}

/// Text that sits safely inside a Python docstring.
fn docstring_text(text: &str) -> String {
    text.replace('\\', "/").replace("\"\"\"", "'''")
}

fn python_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

fn identifier(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn generic_module(base: &str) -> String {
    base.to_lowercase()
}

/// Render an easyblock skeleton from the plan.
///
/// The module derives the class EasyBuild looks up for the software name from
/// the plan's generic easyblock (`CMakeMake` when the plan names none). It
/// carries one `CUSTOM` parameter per feature, a `configure_step` that maps
/// each feature to the options the profiles set with it, a `test_step`, a
/// `sanity_check_step` with variant binary names templated on the feature
/// that flips them, and `make_module_extra`. It is a starting point that a
/// human completes.
///
/// The text is parsed back with [`defined_classes`] and checked for line
/// length and trailing whitespace before it is returned.
pub fn render(plan: &PackagePlan) -> Result<String, String> {
    let class = derived_class(plan);
    if !class
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(format!(
            "derived easyblock class {class} is not a Python identifier"
        ));
    }
    let base = plan
        .build
        .easyblock
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE.to_string());
    if base == class {
        return Err(format!("plan already names {class} as its easyblock"));
    }
    if base.starts_with(crate::eb_easyblock::EASYBLOCK_CLASS_PREFIX) {
        return Err(format!(
            "easyblock {base} is software-specific and has no generic module to derive from"
        ));
    }
    if base.is_empty() || !base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("easyblock {base:?} is not a Python identifier"));
    }
    let features = feature_names(plan);
    for feature in &features {
        if !is_easyconfig_parameter_name(feature) {
            return Err(format!(
                "feature {feature:?} is not usable as an easyconfig parameter name"
            ));
        }
    }

    let mappings = feature_mappings(plan);
    let renames = binary_renames(plan);
    let default = default_profile(plan);
    let default_parameters = match default {
        Some(profile) => merged_parameters(plan, profile),
        None => plan.build.easyconfig_parameters.clone(),
    };
    let has_test = plan
        .profiles
        .iter()
        .map(|profile| &profile.easyconfig_parameters)
        .chain(std::iter::once(&plan.build.easyconfig_parameters))
        .any(|parameters| {
            TEST_PARAMETERS
                .iter()
                .any(|name| parameters.contains_key(*name))
        });
    let sanity_files = sanity_table_list(&default_parameters, "files");
    let sanity_dirs = sanity_table_list(&default_parameters, "dirs");
    let has_paths = sanity_files.is_some();
    let sanity_commands = string_list(default_parameters.get("sanity_check_commands"));
    let has_sanity = has_paths || !sanity_commands.is_empty();
    let modextrapaths = match default_parameters.get("modextrapaths") {
        Some(EasyconfigValue::Table(table)) => table
            .iter()
            .map(|(name, value)| (name.clone(), string_list(Some(value))))
            .filter(|(_, paths)| !paths.is_empty())
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let has_options = mappings.iter().any(|mapping| !mapping.on.is_empty());
    let name = docstring_text(&plan.package.name);

    let mut out: Vec<String> = COPYRIGHT_HEADER
        .iter()
        .map(|line| (*line).to_string())
        .collect();

    // Module docstring.
    out.push("\"\"\"".to_string());
    out.push(format!(
        "EasyBuild support for building and installing {name}, implemented as an easyblock"
    ));
    out.push(String::new());
    out.extend(wrap(
        &format!(
            "This module is a skeleton rendered from the package plan for {name} {}. It has not \
             been built or run. A human completes and reviews it before it ships.",
            docstring_text(&plan.package.version)
        ),
        100,
        "",
        "",
    ));
    out.push(String::new());
    out.extend(wrap(
        &format!(
            "{class} derives from {base}, imported from easybuild.easyblocks.generic.{}. \
             Options shared by every profile stay in the easyconfig.",
            generic_module(&base)
        ),
        100,
        "",
        "",
    ));
    if !mappings.is_empty() {
        out.push(String::new());
        out.push(
            "Variants, one easyconfig parameter each, with defaults from the default profile:"
                .into(),
        );
        out.push(String::new());
        for mapping in &mappings {
            let effect = if mapping.on.is_empty() {
                "no option varies between the profiles".to_string()
            } else if mapping.off.is_empty() {
                format!("sets {} when true", docstring_text(&mapping.on.join(" ")))
            } else {
                format!(
                    "sets {} when true and {} when false",
                    docstring_text(&mapping.on.join(" ")),
                    docstring_text(&mapping.off.join(" "))
                )
            };
            out.extend(wrap(
                &format!(
                    "{}: default {}; {effect}",
                    docstring_text(&mapping.feature),
                    python_bool(mapping.default)
                ),
                100,
                "- ",
                "  ",
            ));
        }
    }
    let mut steps = Vec::new();
    if has_test {
        steps.push("test_step: runs test_cmd with pretestopts and testopts when runtest is True");
    }
    if has_sanity {
        steps.push("sanity_check_step: the files and commands the easyconfigs list");
    }
    if !modextrapaths.is_empty() {
        steps.push("make_module_extra: the paths the easyconfigs list in modextrapaths");
    }
    if !steps.is_empty() {
        out.push(String::new());
        out.push("Steps:".into());
        out.push(String::new());
        for step in steps {
            out.extend(wrap(step, 100, "- ", "  "));
        }
    }
    out.push("\"\"\"".to_string());

    // Imports.
    out.push(format!(
        "from easybuild.easyblocks.generic.{} import {base}",
        generic_module(&base)
    ));
    if !features.is_empty() {
        out.push("from easybuild.framework.easyconfig import CUSTOM".into());
    }
    if has_test {
        out.push("from easybuild.tools.run import run_shell_cmd".into());
    }
    out.push(String::new());
    out.push(String::new());

    // Class.
    out.push(format!("class {class}({base}):"));
    out.push(format!(
        "    \"\"\"Support for building and installing {name}.\"\"\""
    ));

    let mut methods: Vec<Vec<String>> = Vec::new();

    if !features.is_empty() {
        let mut method = vec![
            "    @staticmethod".to_string(),
            "    def extra_options(extra_vars=None):".to_string(),
            format!("        \"\"\"Extra easyconfig parameters specific to {name}.\"\"\""),
            format!("        extra_vars = {base}.extra_options(extra_vars)"),
            "        extra_vars.update({".to_string(),
        ];
        for mapping in &mappings {
            let description = if mapping.on.is_empty() {
                format!("Build variant '{}'", mapping.feature)
            } else {
                format!(
                    "Build variant '{}', which sets {} when enabled",
                    mapping.feature,
                    mapping.on.join(" ")
                )
            };
            method.push(format!("            {}: [", quote(&mapping.feature)));
            method.push(format!("                {},", python_bool(mapping.default)));
            push_string(&mut method, 16, "", &description, ",");
            method.push("                CUSTOM,".to_string());
            method.push("            ],".to_string());
        }
        method.push("        })".to_string());
        method.push("        return extra_vars".to_string());
        methods.push(method);
    }

    if has_options {
        let mut method = vec![
            "    def configure_step(self, *args, **kwargs):".to_string(),
            "        \"\"\"Pass the variant options, leaving any option the easyconfig sets in configopts.\"\"\""
                .to_string(),
            "        # the option values are needed without templating, as configopts is matched as written"
                .to_string(),
            "        with self.cfg.disable_templating():".to_string(),
            "            configopts = self.cfg['configopts']".to_string(),
            String::new(),
            "        options = []".to_string(),
        ];
        for mapping in mappings.iter().filter(|mapping| !mapping.on.is_empty()) {
            method.push(format!("        if self.cfg[{}]:", quote(&mapping.feature)));
            method.push("            options.extend([".to_string());
            for option in &mapping.on {
                push_string(&mut method, 16, "", option, ",");
            }
            method.push("            ])".to_string());
            if !mapping.off.is_empty() {
                method.push("        else:".to_string());
                method.push("            options.extend([".to_string());
                for option in &mapping.off {
                    push_string(&mut method, 16, "", option, ",");
                }
                method.push("            ])".to_string());
            }
        }
        let uniform = mappings
            .iter()
            .filter(|mapping| mapping.on.is_empty())
            .map(|mapping| mapping.feature.clone())
            .collect::<Vec<_>>();
        if !uniform.is_empty() {
            method.extend(wrap(
                &format!(
                    "no option varies with {}; wire a parameter to its option here when it does",
                    uniform.join(", ")
                ),
                100,
                "        # ",
                "        # ",
            ));
        }
        method.push("        for option in options:".to_string());
        method.push(
            "            marker = option.split('=', 1)[0] + '=' if '=' in option else option"
                .to_string(),
        );
        method.push("            if marker in configopts:".to_string());
        method.push(
            "                self.log.info(\"%s is set in configopts, leaving it\", marker)"
                .to_string(),
        );
        method.push("            else:".to_string());
        method.push("                self.cfg.update('configopts', option)".to_string());
        method.push("        return super().configure_step(*args, **kwargs)".to_string());
        methods.push(method);
    }

    if has_test {
        methods.push(vec![
            "    def test_step(self):".to_string(),
            "        \"\"\"Run test_cmd with pretestopts and testopts when runtest is True.\"\"\""
                .to_string(),
            "        test_cmd = self.cfg['test_cmd']".to_string(),
            "        if self.cfg['runtest'] is True and test_cmd:".to_string(),
            "            parts = (self.cfg['pretestopts'], test_cmd, self.cfg['testopts'])"
                .to_string(),
            "            return run_shell_cmd(' '.join(part for part in parts if part)).output"
                .to_string(),
            "        return super().test_step()".to_string(),
        ]);
    }

    if has_sanity {
        let mut original_names: Vec<String> = Vec::new();
        for rename in &renames {
            if !original_names.contains(&rename.from) {
                original_names.push(rename.from.clone());
            }
        }
        let variable = |from: &str| -> String {
            if original_names.len() == 1 {
                "binary".to_string()
            } else {
                format!("binary_{}", identifier(from))
            }
        };
        let mut method = vec![
            "    def sanity_check_step(self):".to_string(),
            "        \"\"\"Check the files and commands the easyconfigs list, with the variant binary name.\"\"\""
                .to_string(),
        ];
        for from in &original_names {
            let name_variable = variable(from);
            push_string(&mut method, 8, &format!("{name_variable} = "), from, "");
            let mut keyword = "if";
            for rename in renames.iter().filter(|rename| rename.from == *from) {
                let negation = if rename.value { "" } else { "not " };
                method.push(format!(
                    "        {keyword} {negation}self.cfg[{}]:",
                    quote(&rename.feature)
                ));
                push_string(
                    &mut method,
                    12,
                    &format!("{name_variable} = "),
                    &rename.to,
                    "",
                );
                keyword = "elif";
            }
        }
        if has_paths {
            method.push("        custom_paths = {".to_string());
            for (key, entries) in [
                ("files", sanity_files.unwrap_or_default()),
                ("dirs", sanity_dirs.unwrap_or_default()),
            ] {
                if entries.is_empty() {
                    method.push(format!("            '{key}': [],"));
                    continue;
                }
                method.push(format!("            '{key}': ["));
                for entry in &entries {
                    let renamed = entry.strip_prefix("bin/").filter(|name| {
                        key == "files" && original_names.iter().any(|from| from == name)
                    });
                    match renamed {
                        Some(name) => {
                            method.push(format!("                'bin/' + {},", variable(name)))
                        }
                        None => push_string(&mut method, 16, "", entry, ","),
                    }
                }
                method.push("            ],".to_string());
            }
            method.push("        }".to_string());
        }
        if !sanity_commands.is_empty() {
            method.push("        custom_commands = [".to_string());
            for command in &sanity_commands {
                let program = command.split_whitespace().next().unwrap_or("");
                if original_names.iter().any(|from| from == program) {
                    let rest = command
                        .trim_start()
                        .strip_prefix(program)
                        .unwrap_or_default()
                        .to_string();
                    if rest.is_empty() {
                        method.push(format!("            {},", variable(program)));
                    } else {
                        push_string(
                            &mut method,
                            12,
                            &format!("{} + ", variable(program)),
                            &rest,
                            ",",
                        );
                    }
                } else {
                    push_string(&mut method, 12, "", command, ",");
                }
            }
            method.push("        ]".to_string());
        }
        method.push(
            match (has_paths, sanity_commands.is_empty()) {
                (true, false) => {
                    "        return super().sanity_check_step(custom_paths=custom_paths, custom_commands=custom_commands)"
                }
                (true, true) => "        return super().sanity_check_step(custom_paths=custom_paths)",
                _ => "        return super().sanity_check_step(custom_commands=custom_commands)",
            }
            .to_string(),
        );
        methods.push(method);
    }

    if !modextrapaths.is_empty() {
        let mut method = vec![
            "    def make_module_extra(self):".to_string(),
            "        \"\"\"Prepend the paths the easyconfigs list in modextrapaths.\"\"\""
                .to_string(),
            "        txt = super().make_module_extra()".to_string(),
        ];
        for (variable_name, paths) in &modextrapaths {
            method.push(format!(
                "        if {} not in self.cfg['modextrapaths']:",
                quote(variable_name)
            ));
            method.push(format!(
                "            txt += self.module_generator.prepend_paths({}, [{}])",
                quote(variable_name),
                paths
                    .iter()
                    .map(|path| quote(path))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        method.push("        return txt".to_string());
        methods.push(method);
    }

    for method in methods {
        out.push(String::new());
        out.extend(method);
    }

    let text = format!("{}\n", out.join("\n"));
    check_rendered(&text, plan, &class, &base, &features)?;
    Ok(text)
}

fn check_rendered(
    text: &str,
    plan: &PackagePlan,
    class: &str,
    base: &str,
    features: &BTreeSet<String>,
) -> Result<(), String> {
    if !text.is_ascii() {
        return Err("rendered easyblock carries non-ASCII text".into());
    }
    for (index, line) in text.lines().enumerate() {
        if line.chars().count() > MAX_LINE {
            return Err(format!(
                "rendered easyblock line {} is longer than {MAX_LINE} characters",
                index + 1
            ));
        }
        if line != line.trim_end() {
            return Err(format!(
                "rendered easyblock line {} has trailing whitespace",
                index + 1
            ));
        }
    }
    let filename = module_filename(plan);
    let classes = defined_classes(text, &filename)
        .map_err(|error| format!("rendered easyblock does not parse: {error}"))?;
    let rendered = classes
        .iter()
        .find(|defined| defined.name == class)
        .ok_or_else(|| format!("rendered easyblock does not define {class}"))?;
    if rendered.bases != [base.to_string()] {
        return Err(format!(
            "rendered easyblock derives from {:?}, not {base}",
            rendered.bases
        ));
    }
    for feature in features {
        if !rendered.extra_options.contains(feature) {
            return Err(format!(
                "rendered easyblock lacks the extra_options key {feature}"
            ));
        }
    }
    Ok(())
}
