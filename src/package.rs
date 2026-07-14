//! Canonical package artifacts shared by foreign imports, bumps, solving, and
//! EasyBuild emission.

use crate::domain::Toolchain;
use crate::version::matches_req;
use cyclonedx_bom::models::component::{Classification, Component, Components};
use cyclonedx_bom::models::dependency::{Dependencies, Dependency};
use cyclonedx_bom::models::lifecycle::{Lifecycle, Lifecycles, Phase};
use cyclonedx_bom::models::metadata::Metadata;
use cyclonedx_bom::models::property::{Properties, Property};
use cyclonedx_bom::models::tool::{Tool, Tools};
use cyclonedx_bom::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::str::FromStr;
use thiserror::Error;

pub const PACKAGE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub path: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    Exact,
    Derived,
    Ambiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub span: SourceSpan,
    pub extractor: String,
    pub original: String,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageOrigin {
    CondaForge,
    Spack,
    EasyBuild,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMetadata {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SourceArtifact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<String>,
    #[serde(default)]
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConditionPredicate {
    PackageVersion {
        requirement: String,
    },
    Feature {
        name: String,
        enabled: bool,
    },
    DependencyFeature {
        dependency: String,
        name: String,
        enabled: bool,
    },
    Compiler {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Toolchain {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Platform {
        name: String,
    },
    Architecture {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "op", content = "args", rename_all = "kebab-case")]
pub enum ConditionExpr {
    #[default]
    Always,
    Predicate(ConditionPredicate),
    All(Vec<ConditionExpr>),
    Any(Vec<ConditionExpr>),
    Not(Box<ConditionExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NamedVersion {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConditionContext {
    pub package_version: String,
    pub features: BTreeMap<String, bool>,
    pub dependency_features: BTreeMap<String, BTreeMap<String, bool>>,
    pub compiler: Option<NamedVersion>,
    pub toolchain: Option<Toolchain>,
    pub platform: Option<String>,
    pub architecture: Option<String>,
}

impl ConditionExpr {
    pub fn evaluate(&self, context: &ConditionContext) -> bool {
        match self {
            Self::Always => true,
            Self::Predicate(predicate) => predicate.evaluate(context),
            Self::All(expressions) => expressions.iter().all(|expr| expr.evaluate(context)),
            Self::Any(expressions) => expressions.iter().any(|expr| expr.evaluate(context)),
            Self::Not(expression) => !expression.evaluate(context),
        }
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyRole {
    Build,
    Host,
    Run,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyIntent {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eb_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<String>,
    #[serde(default)]
    pub roles: Vec<DependencyRole>,
    #[serde(default)]
    pub condition: ConditionExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_capability: Option<String>,
    #[serde(default)]
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    pub toolchain: Toolchain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub easyblock: Option<String>,
    #[serde(default)]
    pub build_systems: Vec<String>,
    #[serde(default)]
    pub config_options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moduleclass: Option<String>,
    #[serde(default)]
    pub patches: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductProfile {
    pub name: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub versionsuffix: Vec<String>,
    #[serde(default)]
    pub features: BTreeMap<String, bool>,
    #[serde(default)]
    pub toolchain_options: BTreeMap<String, bool>,
    #[serde(default)]
    pub config_options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRequest {
    pub profile: String,
    pub stack: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResidualStage {
    Parse,
    Normalize,
    Resolve,
    Emit,
    Build,
    Verify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResidualSeverity {
    Mechanical,
    Judgment,
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Residual {
    pub id: String,
    pub stage: ResidualStage,
    pub category: String,
    pub severity: ResidualSeverity,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagePlan {
    pub schema_version: u32,
    pub origin: PackageOrigin,
    pub package: PackageMetadata,
    #[serde(default)]
    pub sources: Vec<SourceArtifact>,
    #[serde(default)]
    pub dependencies: Vec<DependencyIntent>,
    pub build: BuildSpec,
    #[serde(default)]
    pub profiles: Vec<ProductProfile>,
    #[serde(default)]
    pub outputs: Vec<OutputRequest>,
    #[serde(default)]
    pub residuals: Vec<Residual>,
}

impl PackagePlan {
    pub fn from_json_str(input: &str) -> Result<Self, PackageError> {
        let plan: Self = serde_json::from_str(input)?;
        plan.validate_schema()?;
        Ok(plan)
    }

    pub fn validate_schema(&self) -> Result<(), PackageError> {
        if self.schema_version != PACKAGE_SCHEMA_VERSION {
            return Err(PackageError::UnsupportedSchema(self.schema_version));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("unsupported package schema version {0}")]
    UnsupportedSchema(u32),
    #[error("package JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("CycloneDX serialization: {0}")]
    CycloneDx(String),
}

fn component_ref(name: &str, version: &str) -> String {
    format!("pkg:generic/{name}@{version}")
}

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
    root.properties = Some(Properties(vec![
        Property::new("eb-stack:origin", origin_name(&plan.origin)),
        Property::new("eb-stack:lifecycle", "pre-build-plan"),
    ]));

    let mut components = vec![root];
    let mut dependency_refs = Vec::new();
    for dependency in &plan.dependencies {
        let name = dependency.eb_name.as_deref().unwrap_or(&dependency.name);
        let version = dependency.constraint.as_deref().unwrap_or("unresolved");
        let reference = component_ref(name, version);
        dependency_refs.push(reference.clone());

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
        ]));
        components.push(component);
    }

    let mut dependencies = vec![Dependency {
        dependency_ref: root_ref,
        dependencies: dependency_refs,
    }];
    dependencies.extend(plan.dependencies.iter().map(|dependency| {
        let name = dependency.eb_name.as_deref().unwrap_or(&dependency.name);
        let version = dependency.constraint.as_deref().unwrap_or("unresolved");
        Dependency {
            dependency_ref: component_ref(name, version),
            dependencies: Vec::new(),
        }
    }));

    let mut metadata = Metadata::new().unwrap_or_default();
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
    }
}
