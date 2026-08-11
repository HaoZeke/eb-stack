//! Canonical package artifacts shared by foreign imports, bumps, solving, and
//! EasyBuild emission.

use crate::domain::{Candidate, Toolchain};
use crate::version::matches_req;
use cyclonedx_bom::models::component::{Classification, Component, Components};
use cyclonedx_bom::models::dependency::{Dependencies, Dependency};
use cyclonedx_bom::models::external_reference::{
    ExternalReference, ExternalReferenceType, ExternalReferences,
};
use cyclonedx_bom::models::hash::{Hash, HashAlgorithm, HashValue, Hashes};
use cyclonedx_bom::models::lifecycle::{Lifecycle, Lifecycles, Phase};
use cyclonedx_bom::models::metadata::Metadata;
use cyclonedx_bom::models::property::{Properties, Property};
use cyclonedx_bom::models::tool::{Tool, Tools};
use cyclonedx_bom::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::str::FromStr;
use thiserror::Error;

/// Schema version of a package plan document.
pub const PACKAGE_SCHEMA_VERSION: u32 = 1;
/// Schema version of a per-profile dependency lock.
pub const PROFILE_LOCK_SCHEMA_VERSION: u32 = 1;
/// Schema version of a stack policy document.
pub const STACK_POLICY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// How strictly a stack pin binds the solve.
pub enum StackPinMode {
    /// Take this version when it is available, otherwise fall back and say so.
    Preferred,
    /// Take this version or fail; no fallback is acceptable.
    Locked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A version constraint the site imposes on one dependency.
pub struct StackPin {
    /// Package the pin applies to.
    pub name: String,
    /// Version requirement, e.g. an exact version or a range.
    pub version_requirement: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Restrict the pin to one toolchain. `None` matches any.
    pub toolchain: Option<Toolchain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Restrict the pin to one versionsuffix. `None` matches any.
    pub versionsuffix: Option<String>,
    /// Whether a fallback is permitted when the pin cannot be met.
    pub mode: StackPinMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Where the pin came from, carried through for auditability.
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A candidate the solve must not choose, and why.
pub struct CandidateExclusion {
    /// Package excluded.
    pub name: String,
    /// Versions excluded, as a requirement expression.
    pub version_requirement: String,
    /// Why it is excluded. Recorded in the lock so a reader is not left
    /// guessing why an obvious candidate was skipped.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Where the exclusion applies, when it is not global.
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Site rules a dependency solve must respect.
pub struct StackPolicy {
    /// Must equal [`STACK_POLICY_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Policy name, for messages and provenance.
    pub name: String,
    /// Toolchain generation the policy governs.
    pub toolchain: Toolchain,
    #[serde(default)]
    /// Version constraints imposed on individual packages.
    pub pins: Vec<StackPin>,
    #[serde(default)]
    /// Candidates the solve must not choose.
    pub exclusions: Vec<CandidateExclusion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// What a pin actually achieved, recorded per solve.
pub struct StackPinOutcome {
    /// Package the pin named.
    pub name: String,
    /// Version requirement as requested.
    pub requested: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Toolchain requested, when the pin restricted one.
    pub requested_toolchain: Option<Toolchain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Versionsuffix requested, when the pin restricted one.
    pub requested_versionsuffix: Option<String>,
    /// Version chosen. `None` when nothing satisfied the pin.
    pub selected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Toolchain of the chosen candidate.
    pub selected_toolchain: Option<Toolchain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Versionsuffix of the chosen candidate.
    pub selected_versionsuffix: Option<String>,
    /// Whether the solve had to fall back off the pin. Only a `Preferred`
    /// pin can reach this; a `Locked` pin fails the solve instead.
    pub fallback: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Why the fallback happened, when one did.
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The result of applying a stack policy to a candidate set.
pub struct StackPolicySolve {
    /// Candidates the policy allows, after pins and exclusions.
    pub selected: Vec<Candidate>,
    /// One outcome per pin, including the ones that fell back.
    pub pin_outcomes: Vec<StackPinOutcome>,
    /// Exclusions that were applied, echoed for the lock.
    pub exclusions: Vec<CandidateExclusion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// The conditions a profile evaluates against.
pub struct ProfileEnvironment {
    #[serde(default)]
    /// Feature flags per dependency, keyed by dependency then flag.
    pub dependency_features: BTreeMap<String, BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Compiler in play, when a condition depends on it.
    pub compiler: Option<NamedVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Target platform, when a condition depends on it.
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Target architecture, when a condition depends on it.
    pub architecture: Option<String>,
    #[serde(default)]
    /// Free-form variables conditions may test.
    pub variables: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One profile with every condition resolved: what will actually be built.
pub struct MaterializedProfile {
    /// Package identity and metadata.
    pub package: PackageMetadata,
    /// Build system and easyconfig parameters.
    pub build: BuildSpec,
    /// Source artifacts this profile downloads.
    pub sources: Vec<SourceArtifact>,
    /// The profile this was materialized from.
    pub profile: ProductProfile,
    /// Concatenated versionsuffix for the emitted recipe.
    pub versionsuffix: String,
    /// Dependencies whose conditions held for this profile.
    pub dependencies: Vec<DependencyIntent>,
    /// Rules whose conditions held for this profile.
    pub rules: Vec<PackageRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One dependency as resolved into a lock.
pub struct LockedDependency {
    /// Package name.
    pub name: String,
    /// Version selected.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Versionsuffix of the selected variant, when it has one.
    pub versionsuffix: Option<String>,
    /// Toolchain the selected easyconfig builds against.
    pub toolchain: Toolchain,
    /// Easyconfig the selection came from.
    pub easyconfig_path: String,
    /// True for a build-time-only dependency, so a consumer can tell the
    /// runtime closure from the build closure.
    pub build: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The reproducible dependency selection for one profile.
pub struct ProfileLock {
    /// Must equal [`PROFILE_LOCK_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Package this lock belongs to.
    pub package: String,
    /// Package version.
    pub version: String,
    /// Profile name.
    pub profile: String,
    /// Toolchain the profile targets.
    pub toolchain: Toolchain,
    /// Versionsuffix of the emitted recipe.
    pub versionsuffix: String,
    #[serde(default)]
    /// Every dependency selected, build and runtime alike.
    pub dependencies: Vec<LockedDependency>,
    #[serde(default)]
    /// What each policy pin achieved.
    pub pin_outcomes: Vec<StackPinOutcome>,
    #[serde(default)]
    /// Exclusions the policy applied.
    pub exclusions: Vec<CandidateExclusion>,
    /// Solver that produced the lock.
    pub solver: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Where in a foreign recipe a value came from.
pub struct SourceSpan {
    /// Recipe file, as given to the parser.
    pub path: String,
    /// First line of the span, 1-based.
    pub start_line: u32,
    /// First column, 1-based.
    pub start_column: u32,
    /// Last line of the span, 1-based.
    pub end_line: u32,
    /// Last column, 1-based.
    pub end_column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// How sure the extractor is that it read a value correctly.
pub enum Confidence {
    /// Read directly; no interpretation was involved.
    Exact,
    /// Inferred from surrounding context, and worth a reviewer's eye.
    Derived,
    /// Could not be resolved to one meaning. Treat as needing review.
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The trail from an extracted value back to the text it came from.
///
/// Carried so a reviewer can check a translated recipe against its source
/// rather than trusting the extraction.
pub struct Provenance {
    /// Text the value was read from.
    pub span: SourceSpan,
    /// Which extractor produced it.
    pub extractor: String,
    /// The original text, before any normalisation.
    pub original: String,
    /// How much the extractor trusts the reading.
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Ecosystem a package definition came from.
pub enum PackageOrigin {
    /// A conda-forge `meta.yaml` or `recipe.yaml`.
    CondaForge,
    /// A Spack `package.py`.
    Spack,
    /// An EasyBuild easyconfig.
    EasyBuild,
    /// Offline PyPI metadata or a requirements.txt.
    Pypi,
    /// A CRAN DESCRIPTION file or package list.
    Cran,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Package identity as it will appear in the emitted recipe.
pub struct PackageMetadata {
    /// Package name, in EasyBuild's spelling.
    pub name: String,
    /// Version the emitted recipe declares.
    pub version: String,
    /// Version identity used by the foreign recipe when it differs from the
    /// emitted EasyBuild version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Project homepage.
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Short description for the module.
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// License string, as the recipe declares it.
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// One thing the build downloads or checks out.
///
/// A downloaded file carries `url` and `sha256`; a checkout carries `git`
/// with a `tag` or `commit`. The two are alternatives, and a checkout has no
/// artifact checksum to verify.
pub struct SourceArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Download URL, for a fetched artifact.
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Local filename to save as, when it differs from the URL basename.
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// SHA-256 of the downloaded bytes. Only meaningful for a download, and
    /// only valid for the artifact class that URL serves.
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Remote to clone, for a checkout instead of a download.
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Tag to check out.
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Commit to check out, which pins more tightly than a tag.
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Subdirectory to unpack into, when the build expects one.
    pub target_directory: Option<String>,
    #[serde(default)]
    /// When this artifact applies. Defaults to always.
    pub condition: ConditionExpr,
    #[serde(default)]
    /// Where this artifact was read from.
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// One testable fact about the build being configured.
pub enum ConditionPredicate {
    /// The package version satisfies a requirement.
    PackageVersion {
        /// Version requirement to test.
        requirement: String,
    },
    /// A profile feature flag has a given value.
    Feature {
        /// Feature name.
        name: String,
        /// Value it must have.
        enabled: bool,
    },
    /// A dependency was built with a feature flag set a given way.
    DependencyFeature {
        /// Dependency carrying the flag.
        dependency: String,
        /// Feature name.
        name: String,
        /// Value it must have.
        enabled: bool,
    },
    /// The compiler matches, optionally at a version.
    Compiler {
        /// Compiler name.
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Version to match, or any version when absent.
        version: Option<String>,
    },
    /// The toolchain matches, optionally at a version.
    Toolchain {
        /// Toolchain name.
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Version to match, or any version when absent.
        version: Option<String>,
    },
    /// The target platform matches.
    Platform {
        /// Platform name.
        name: String,
    },
    /// The target architecture matches.
    Architecture {
        /// Architecture name.
        name: String,
    },
    /// A context variable compares as stated.
    VariableComparison {
        /// Variable name or literal on the left.
        left: String,
        /// Comparison operator.
        operator: String,
        /// Value on the right.
        right: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "op", content = "args", rename_all = "kebab-case")]
/// A condition gating a dependency, source, or rule.
///
/// Foreign selectors are lowered into this tree so they can be evaluated per
/// profile. Anything that could not be lowered becomes [`Self::Opaque`]
/// rather than being dropped or guessed at.
pub enum ConditionExpr {
    #[default]
    /// Always true, the default for an unconditional item.
    Always,
    /// Never true, which excludes the item entirely.
    Never,
    /// A single predicate.
    Predicate(ConditionPredicate),
    /// True when every branch is.
    All(Vec<ConditionExpr>),
    /// True when any branch is.
    Any(Vec<ConditionExpr>),
    /// Negation.
    Not(Box<ConditionExpr>),
    /// A selector that could not be lowered. Preserved verbatim so a
    /// reviewer sees what was not understood instead of a silent drop.
    Opaque {
        /// The selector text as written.
        source: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// A name paired with a version, used where a full toolchain is too much.
pub struct NamedVersion {
    /// Name, e.g. a compiler.
    pub name: String,
    /// Version string.
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
/// What a condition is evaluated against for one profile.
pub struct ConditionContext {
    /// Version of the package being built.
    pub package_version: String,
    /// Feature flags of the profile.
    pub features: BTreeMap<String, bool>,
    /// Feature flags of each selected dependency.
    pub dependency_features: BTreeMap<String, BTreeMap<String, bool>>,
    /// Compiler in use, when known.
    pub compiler: Option<NamedVersion>,
    /// Toolchain in use, when known.
    pub toolchain: Option<Toolchain>,
    /// Target platform, when known.
    pub platform: Option<String>,
    /// Target architecture, when known.
    pub architecture: Option<String>,
    /// Free-form variables a comparison predicate may reference.
    pub variables: BTreeMap<String, String>,
}

impl ConditionExpr {
    /// Resolve package-version predicates while preserving conditions that
    /// depend on a profile, toolchain, platform, or dependency selection.
    pub fn specialize_package_version(&self, package_version: &str) -> Self {
        match self {
            Self::Always | Self::Never => self.clone(),
            Self::Predicate(ConditionPredicate::PackageVersion { requirement }) => {
                if matches_req(package_version, requirement) {
                    Self::Always
                } else {
                    Self::Never
                }
            }
            Self::Predicate(_) | Self::Opaque { .. } => self.clone(),
            Self::All(expressions) => specialize_all(expressions, package_version),
            Self::Any(expressions) => specialize_any(expressions, package_version),
            Self::Not(expression) => match expression.specialize_package_version(package_version) {
                Self::Always => Self::Never,
                Self::Never => Self::Always,
                expression => Self::Not(Box::new(expression)),
            },
        }
    }

    /// Whether this condition holds in `context`.
    pub fn evaluate(&self, context: &ConditionContext) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Predicate(predicate) => predicate.evaluate(context),
            Self::All(expressions) => expressions.iter().all(|expr| expr.evaluate(context)),
            Self::Any(expressions) => expressions.iter().any(|expr| expr.evaluate(context)),
            Self::Not(expression) => !expression.evaluate(context),
            Self::Opaque { .. } => false,
        }
    }
}

fn specialize_all(expressions: &[ConditionExpr], package_version: &str) -> ConditionExpr {
    let mut specialized = Vec::new();
    for expression in expressions {
        match expression.specialize_package_version(package_version) {
            ConditionExpr::Never => return ConditionExpr::Never,
            ConditionExpr::Always => {}
            ConditionExpr::All(nested) => specialized.extend(nested),
            expression => specialized.push(expression),
        }
    }
    match specialized.len() {
        0 => ConditionExpr::Always,
        1 => specialized.pop().unwrap_or(ConditionExpr::Always),
        _ => ConditionExpr::All(specialized),
    }
}

fn specialize_any(expressions: &[ConditionExpr], package_version: &str) -> ConditionExpr {
    let mut specialized = Vec::new();
    for expression in expressions {
        match expression.specialize_package_version(package_version) {
            ConditionExpr::Always => return ConditionExpr::Always,
            ConditionExpr::Never => {}
            ConditionExpr::Any(nested) => specialized.extend(nested),
            expression => specialized.push(expression),
        }
    }
    match specialized.len() {
        0 => ConditionExpr::Never,
        1 => specialized.pop().unwrap_or(ConditionExpr::Never),
        _ => ConditionExpr::Any(specialized),
    }
}

impl ConditionPredicate {
    fn evaluate(&self, context: &ConditionContext) -> bool {
        match self {
            Self::PackageVersion { requirement } => {
                matches_req(&context.package_version, requirement)
            }
            Self::Feature { name, enabled } => {
                context.features.get(name).copied().unwrap_or(false) == *enabled
            }
            Self::DependencyFeature {
                dependency,
                name,
                enabled,
            } => {
                context
                    .dependency_features
                    .get(dependency)
                    .and_then(|features| features.get(name))
                    .copied()
                    .unwrap_or(false)
                    == *enabled
            }
            Self::Compiler { name, version } => context.compiler.as_ref().is_some_and(|compiler| {
                compiler.name.eq_ignore_ascii_case(name)
                    && version
                        .as_deref()
                        .is_none_or(|requirement| matches_req(&compiler.version, requirement))
            }),
            Self::Toolchain { name, version } => {
                context.toolchain.as_ref().is_some_and(|toolchain| {
                    toolchain.name.eq_ignore_ascii_case(name)
                        && version
                            .as_deref()
                            .is_none_or(|requirement| matches_req(&toolchain.version, requirement))
                })
            }
            Self::Platform { name } => context
                .platform
                .as_deref()
                .is_some_and(|platform| platform.eq_ignore_ascii_case(name)),
            Self::Architecture { name } => context
                .architecture
                .as_deref()
                .is_some_and(|architecture| architecture.eq_ignore_ascii_case(name)),
            Self::VariableComparison {
                left,
                operator,
                right,
            } => {
                let Some(left_value) = condition_left_value(context, left) else {
                    return false;
                };
                let right_value = condition_right_value(context, right);
                match operator.as_str() {
                    "==" => left_value == right_value,
                    "!=" => left_value != right_value,
                    _ => false,
                }
            }
        }
    }
}

fn condition_left_value(context: &ConditionContext, operand: &str) -> Option<String> {
    let operand = operand.trim();
    if let Some(inner) = operand
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        if let Some((variable, fallback)) = inner.split_once(" or ") {
            let variable = variable.trim();
            let fallback = unquote_condition_literal(fallback.trim());
            return Some(
                context
                    .variables
                    .get(variable)
                    .filter(|value| !value.is_empty())
                    .cloned()
                    .unwrap_or(fallback),
            );
        }
    }
    context.variables.get(operand).cloned()
}

fn condition_right_value(context: &ConditionContext, operand: &str) -> String {
    let operand = operand.trim();
    if is_quoted_condition_literal(operand) {
        return unquote_condition_literal(operand);
    }
    context
        .variables
        .get(operand)
        .cloned()
        .unwrap_or_else(|| operand.to_string())
}

fn is_quoted_condition_literal(value: &str) -> bool {
    value.len() >= 2
        && matches!(value.as_bytes()[0], b'\'' | b'"')
        && value.as_bytes()[0] == value.as_bytes()[value.len() - 1]
}

fn unquote_condition_literal(value: &str) -> String {
    if is_quoted_condition_literal(value) {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// When a dependency is needed.
pub enum DependencyRole {
    /// Needed to build, not at run time.
    Build,
    /// Needed on the build host, for a cross build.
    Host,
    /// Needed at run time.
    Run,
    /// Needed only to run the test suite.
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One dependency the recipe declares, before any solve.
pub struct DependencyIntent {
    /// Stable identifier within the plan, so a rule can refer to it.
    pub id: String,
    /// Name as the foreign recipe spells it.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// EasyBuild's name for the same thing, when they differ.
    pub eb_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Version requirement, or `None` for any version.
    pub constraint: Option<String>,
    /// Explicit EasyBuild dependency toolchain after generation retargeting.
    /// `None` keeps minimal-toolchain selection within the output hierarchy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<Toolchain>,
    #[serde(default)]
    /// When it is needed. Empty means the recipe never said.
    pub roles: Vec<DependencyRole>,
    #[serde(default)]
    /// When this dependency applies. Defaults to always.
    pub condition: ConditionExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Capability this satisfies, when it stands in for a virtual package.
    pub virtual_capability: Option<String>,
    #[serde(default)]
    /// True when the solve must skip it, because the toolchain already
    /// provides it.
    pub solver_excluded: bool,
    #[serde(default)]
    /// Where this dependency was read from.
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Whether a rule forbids a combination or demands one.
pub enum PackageRuleKind {
    /// The combination must not occur.
    Conflict,
    /// The combination is required.
    Requirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A constraint the recipe states beyond its dependency list.
pub struct PackageRule {
    /// Stable identifier within the plan.
    pub id: String,
    /// Whether this forbids or requires.
    pub kind: PackageRuleKind,
    /// The combination, in the foreign recipe's own spelling.
    pub spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Original `when=` text, kept for review alongside the lowered form.
    pub when: Option<String>,
    #[serde(default)]
    /// When the rule applies, lowered from the selector.
    pub condition: ConditionExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Message to show when the rule fires.
    pub message: Option<String>,
    /// Where the rule was read from.
    pub provenance: Provenance,
}

/// Python data values accepted by public EasyBuild package policy.
///
/// Expressions are deliberately absent: package configuration describes
/// easyblock inputs without becoming an arbitrary Python execution surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A value built by joining strings, kept in parts so they stay reviewable.
pub struct EasyconfigStringConcat {
    /// Fragments joined in order.
    pub concat: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// A value written verbatim into an easyconfig parameter.
pub enum EasyconfigValue {
    /// A Python bool.
    Bool(bool),
    /// A Python int.
    Integer(i64),
    /// A Python string.
    String(String),
    /// A Python list.
    List(Vec<EasyconfigValue>),
    /// A string built by concatenation.
    Concat(EasyconfigStringConcat),
    /// A Python dict.
    Table(BTreeMap<String, EasyconfigValue>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A patch the build applies.
pub struct PatchArtifact {
    /// Filename EasyBuild references. A bare name, never a path.
    pub filename: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// SHA-256 of the patch bytes.
    pub sha256: Option<String>,
    /// Exact remote patch URL accepted by EasyBuild's `patches` parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Local path to copy the patch from.
    pub source: Option<String>,
    #[serde(default)]
    /// When this patch applies.
    pub condition: ConditionExpr,
    #[serde(skip)]
    /// Where the patch was found on disk, resolved relative to the layer that
    /// named it. Not serialized: it is a detail of this run.
    pub resolved_source: Option<PathBuf>,
}

pub(crate) fn is_easyconfig_parameter_name(name: &str) -> bool {
    let mut characters = name.chars();
    let identifier = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_');
    identifier
        && !matches!(
            name,
            "easyblock"
                | "name"
                | "version"
                | "versionsuffix"
                | "homepage"
                | "description"
                | "toolchain"
                | "toolchainopts"
                | "sources"
                | "checksums"
                | "patches"
                | "configopts"
                | "builddependencies"
                | "dependencies"
                | "moduleclass"
        )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// How the package is built, independent of profile.
pub struct BuildSpec {
    /// Toolchain the build targets.
    pub toolchain: Toolchain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// EasyBuild easyblock class.
    pub easyblock: Option<String>,
    #[serde(default)]
    /// Build systems detected or configured.
    pub build_systems: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Subdirectory of the unpacked source to build from.
    pub source_root: Option<String>,
    #[serde(default)]
    /// Configure flags common to every profile.
    pub config_options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// EasyBuild moduleclass.
    pub moduleclass: Option<String>,
    #[serde(default)]
    /// Patches applied before configuring.
    pub patches: Vec<PatchArtifact>,
    #[serde(default)]
    /// Raw easyconfig parameters written through verbatim.
    pub easyconfig_parameters: BTreeMap<String, EasyconfigValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A command that proves the installed build works.
pub struct VerificationCommand {
    /// Program to run.
    pub program: String,
    #[serde(default)]
    /// Arguments passed to it.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One build variant of the package.
pub struct ProductProfile {
    /// Profile name, unique within the plan.
    pub name: String,
    #[serde(default)]
    /// Whether this is chosen when no profile is named. Exactly one profile
    /// must be the default.
    pub default: bool,
    #[serde(default)]
    /// Versionsuffix fragments, concatenated in order.
    pub versionsuffix: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Platform this profile is restricted to.
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Architecture this profile is restricted to.
    pub architecture: Option<String>,
    #[serde(default)]
    /// Feature flags, which conditions test.
    pub features: BTreeMap<String, bool>,
    #[serde(default)]
    /// Free-form parameters conditions may reference.
    pub parameters: BTreeMap<String, String>,
    #[serde(default)]
    /// EasyBuild toolchain options such as `pic` or `openmp`.
    pub toolchain_options: BTreeMap<String, bool>,
    #[serde(default)]
    /// Configure flags for this profile, on top of the build-level set.
    pub config_options: Vec<String>,
    #[serde(default)]
    /// Raw easyconfig parameters for this profile.
    pub easyconfig_parameters: BTreeMap<String, EasyconfigValue>,
    #[serde(default)]
    /// Commands proving this variant works.
    pub verification_commands: Vec<VerificationCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// A request to emit one profile against one stack.
pub struct OutputRequest {
    /// Profile to emit.
    pub profile: String,
    /// Stack policy to solve against.
    pub stack: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Which step of the pipeline left work behind.
pub enum ResidualStage {
    /// Reading the foreign recipe.
    Parse,
    /// Lowering it into the canonical plan.
    Normalize,
    /// Selecting dependencies.
    Resolve,
    /// Writing the easyconfig.
    Emit,
    /// Building the package.
    Build,
    /// Verifying the installed build.
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// How much attention a residual needs.
pub enum ResidualSeverity {
    /// A mechanical gap; a tool could close it.
    Mechanical,
    /// Needs a human decision.
    Judgment,
    /// Must be resolved before the result can be trusted.
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Work the pipeline could not do mechanically, recorded rather than dropped
/// so nothing is silently lost in translation.
pub struct Residual {
    /// Stable identifier within the plan.
    pub id: String,
    /// Step that produced it.
    pub stage: ResidualStage,
    /// Short machine-readable category.
    pub category: String,
    /// How much attention it needs.
    pub severity: ResidualSeverity,
    /// One-line description for a reviewer.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Supporting text, such as the source that was not understood.
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Where in the recipe it arose.
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// The canonical description of a package: the shape every ingest lowers to
/// and every emitter reads.
pub struct PackagePlan {
    /// Must equal [`PACKAGE_SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Ecosystem the definition came from.
    pub origin: PackageOrigin,
    /// Package identity and metadata.
    pub package: PackageMetadata,
    #[serde(default)]
    /// Artifacts the build downloads or checks out.
    pub sources: Vec<SourceArtifact>,
    #[serde(default)]
    /// Dependencies declared, before any solve.
    pub dependencies: Vec<DependencyIntent>,
    #[serde(default)]
    /// Constraints beyond the dependency list.
    pub rules: Vec<PackageRule>,
    /// How the package is built.
    pub build: BuildSpec,
    #[serde(default)]
    /// Build variants.
    pub profiles: Vec<ProductProfile>,
    #[serde(default)]
    /// Profile and stack combinations to emit.
    pub outputs: Vec<OutputRequest>,
    #[serde(default)]
    /// Work left for a human, carried rather than dropped.
    pub residuals: Vec<Residual>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    /// PyPI run dependencies the robot does not ship. Emitted as extra
    /// `exts_list` entries on the leftover `PythonBundle`, not as SAT holes.
    pub overlay_extensions: Vec<OverlayExtension>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// One leftover PyPI package installed in the same `PythonBundle` as the root.
pub struct OverlayExtension {
    /// PyPI / EasyBuild extension name.
    pub name: String,
    /// Exact version EasyBuild will download.
    pub version: String,
}

impl PackagePlan {
    /// Parse a plan from JSON and check its schema version.
    pub fn from_json_str(input: &str) -> Result<Self, PackageError> {
        let plan: Self = serde_json::from_str(input)?;
        plan.validate_schema()?;
        Ok(plan)
    }

    /// Reject a plan whose schema this build does not read.
    pub fn validate_schema(&self) -> Result<(), PackageError> {
        if self.schema_version != PACKAGE_SCHEMA_VERSION {
            return Err(PackageError::UnsupportedSchema(self.schema_version));
        }
        Ok(())
    }
}

/// Resolve one profile's conditions against an environment.
///
/// Every conditional dependency, source and rule is evaluated, so the result
/// describes exactly what will be built. Fails when the profile does not
/// exist rather than falling back to the default.
pub fn materialize_profile(
    plan: &PackagePlan,
    profile_name: &str,
    environment: &ProfileEnvironment,
) -> Result<MaterializedProfile, PackageError> {
    plan.validate_schema()?;
    let profile = plan
        .profiles
        .iter()
        .find(|profile| profile.name == profile_name)
        .cloned()
        .ok_or_else(|| PackageError::ProfileNotFound(profile_name.to_string()))?;

    let mut variables = profile.parameters.clone();
    variables.extend(environment.variables.clone());
    let context = ConditionContext {
        package_version: plan
            .package
            .upstream_version
            .clone()
            .unwrap_or_else(|| plan.package.version.clone()),
        features: profile.features.clone(),
        dependency_features: environment.dependency_features.clone(),
        compiler: environment.compiler.clone(),
        toolchain: Some(plan.build.toolchain.clone()),
        platform: environment
            .platform
            .clone()
            .or_else(|| profile.platform.clone()),
        architecture: environment
            .architecture
            .clone()
            .or_else(|| profile.architecture.clone()),
        variables,
    };
    let dependencies = plan
        .dependencies
        .iter()
        .filter(|dependency| dependency.condition.evaluate(&context))
        .cloned()
        .collect();
    let sources = plan
        .sources
        .iter()
        .filter(|source| source.condition.evaluate(&context))
        .cloned()
        .collect();
    let rules = plan
        .rules
        .iter()
        .filter(|rule| rule.condition.evaluate(&context))
        .cloned()
        .collect();
    let mut build = plan.build.clone();
    build
        .patches
        .retain(|patch| patch.condition.evaluate(&context));

    Ok(MaterializedProfile {
        package: plan.package.clone(),
        build,
        sources,
        versionsuffix: profile.versionsuffix.concat(),
        profile,
        dependencies,
        rules,
    })
}

#[derive(Debug, Error)]
/// Why a package plan could not be read or rendered.
pub enum PackageError {
    #[error("unsupported package schema version {0}")]
    /// The plan declares a schema this build does not read.
    UnsupportedSchema(u32),
    #[error("package profile {0} does not exist")]
    /// The named profile is not in the plan.
    ProfileNotFound(String),
    #[error("package JSON: {0}")]
    /// The plan is not valid JSON, or does not match the schema.
    Json(#[from] serde_json::Error),
    #[error("CycloneDX serialization: {0}")]
    /// The SBOM could not be serialized.
    CycloneDx(String),
}

fn component_ref(name: &str, version: &str) -> String {
    format!("pkg:generic/{name}@{version}")
}

/// Build a CycloneDX BOM describing what the plan intends to install.
///
/// This records intent. It is not evidence of what a build produced, and it
/// carries a checksum only where the plan had one.
pub fn package_plan_to_bom(plan: &PackagePlan) -> Result<Bom, PackageError> {
    plan.validate_schema()?;

    let root_ref = component_ref(&plan.package.name, &plan.package.version);
    let mut root = Component::new(
        Classification::Application,
        &plan.package.name,
        &plan.package.version,
        Some(root_ref.clone()),
    );
    root.purl = Purl::from_str(&root_ref).ok();
    root.description = plan
        .package
        .description
        .as_deref()
        .map(NormalizedString::new);
    root.hashes = plan
        .sources
        .first()
        .and_then(|source| source.sha256.as_deref())
        .map(|checksum| Hashes(vec![sha256_hash(checksum)]));
    let mut source_references = Vec::new();
    let mut seen_references = BTreeSet::new();
    for source in &plan.sources {
        if let Some(git) = source.git.as_deref() {
            let key = (ExternalReferenceType::Vcs.to_string(), git.to_string());
            if seen_references.insert(key) {
                let mut reference =
                    ExternalReference::new(ExternalReferenceType::Vcs, Uri::new(git));
                let identity = [
                    source.tag.as_deref().map(|tag| format!("tag={tag}")),
                    source
                        .commit
                        .as_deref()
                        .map(|commit| format!("commit={commit}")),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
                if !identity.is_empty() {
                    reference.comment = Some(identity);
                }
                source_references.push(reference);
            }
        }
        if let Some(url) = source_archive_url(source) {
            let key = (ExternalReferenceType::Distribution.to_string(), url.clone());
            if seen_references.insert(key) {
                let mut reference =
                    ExternalReference::new(ExternalReferenceType::Distribution, Uri::new(&url));
                reference.hashes = source
                    .sha256
                    .as_deref()
                    .map(|checksum| Hashes(vec![sha256_hash(checksum)]));
                reference.comment = source
                    .target_directory
                    .as_deref()
                    .map(|directory| format!("staged in package source directory {directory}"));
                source_references.push(reference);
            }
        }
    }
    if !source_references.is_empty() {
        root.external_references = Some(ExternalReferences(source_references));
    }
    let mut root_properties = vec![
        Property::new("eb-stack:origin", origin_name(&plan.origin)),
        Property::new("eb-stack:lifecycle", "pre-build-plan"),
    ];
    if let Some(upstream_version) = plan.package.upstream_version.as_deref() {
        root_properties.push(Property::new("eb-stack:upstream-version", upstream_version));
    }
    for patch in &plan.build.patches {
        let value = patch
            .sha256
            .as_deref()
            .map(|sha256| format!("{} sha256:{sha256}", patch.filename))
            .unwrap_or_else(|| patch.filename.clone());
        root_properties.push(Property::new("eb-stack:patch", &value));
    }
    root.properties = Some(Properties(root_properties));

    let mut components = vec![root];
    let mut seen_component_refs = BTreeSet::new();
    seen_component_refs.insert(root_ref.clone());
    let mut dependency_refs = Vec::new();
    for dependency in &plan.dependencies {
        let name = dependency.eb_name.as_deref().unwrap_or(&dependency.name);
        let version = dependency.constraint.as_deref().unwrap_or("unresolved");
        let reference = component_ref(name, version);
        if !dependency_refs.contains(&reference) {
            dependency_refs.push(reference.clone());
        }

        if !seen_component_refs.insert(reference.clone()) {
            continue;
        }

        let mut component = Component::new(Classification::Library, name, version, Some(reference));
        let roles = dependency
            .roles
            .iter()
            .map(|role| format!("{role:?}").to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(",");
        let condition = serde_json::to_string(&dependency.condition)?;
        component.properties = Some(Properties(vec![
            Property::new("eb-stack:upstream-name", &dependency.name),
            Property::new("eb-stack:roles", &roles),
            Property::new("eb-stack:condition", &condition),
            Property::new(
                "eb-stack:solver-excluded",
                &dependency.solver_excluded.to_string(),
            ),
        ]));
        components.push(component);
    }

    let mut dependencies = vec![Dependency {
        dependency_ref: root_ref,
        dependencies: dependency_refs.clone(),
    }];
    dependencies.extend(
        dependency_refs
            .iter()
            .cloned()
            .map(|dependency_ref| Dependency {
                dependency_ref,
                dependencies: Vec::new(),
            }),
    );

    let mut metadata = Metadata::new().unwrap_or_default();
    metadata.timestamp = None;
    metadata.tools = Some(Tools::List(vec![Tool::new(
        "eb-stack",
        "eb-stack",
        env!("CARGO_PKG_VERSION"),
    )]));
    metadata.component = components.first().cloned();
    metadata.properties = Some(Properties(vec![Property::new(
        "eb-stack:document-kind",
        "canonical-package-plan",
    )]));
    metadata.lifecycles = Some(Lifecycles(vec![Lifecycle::Phase(Phase::PreBuild)]));

    Ok(Bom {
        version: 1,
        serial_number: None,
        metadata: Some(metadata),
        components: Some(Components(components)),
        services: None,
        external_references: None,
        dependencies: Some(Dependencies(dependencies)),
        compositions: None,
        properties: None,
        vulnerabilities: None,
        signature: None,
        annotations: None,
        formulation: None,
        spec_version: SpecVersion::V1_5,
    })
}

fn sha256_hash(checksum: &str) -> Hash {
    Hash {
        alg: HashAlgorithm::SHA_256,
        content: HashValue(checksum.to_string()),
    }
}

fn source_archive_url(source: &SourceArtifact) -> Option<String> {
    source.url.clone().or_else(|| {
        let git = source.git.as_deref()?;
        let base = git.trim_end_matches(".git");
        if let Some(tag) = source.tag.as_deref() {
            Some(format!("{base}/archive/refs/tags/{tag}.tar.gz"))
        } else {
            source
                .commit
                .as_deref()
                .map(|commit| format!("{base}/archive/{commit}.tar.gz"))
        }
    })
}

/// The planned BOM as JSON.
pub fn package_plan_to_cyclonedx(plan: &PackagePlan) -> Result<Value, PackageError> {
    let bom = package_plan_to_bom(plan)?;
    let mut output = Vec::new();
    bom.output_as_json_v1_5(&mut output)
        .map_err(|error| PackageError::CycloneDx(error.to_string()))?;
    serde_json::from_slice(&output).map_err(PackageError::from)
}

fn origin_name(origin: &PackageOrigin) -> &'static str {
    match origin {
        PackageOrigin::CondaForge => "conda-forge",
        PackageOrigin::Spack => "spack",
        PackageOrigin::EasyBuild => "easybuild",
        PackageOrigin::Pypi => "pypi",
        PackageOrigin::Cran => "cran",
    }
}
