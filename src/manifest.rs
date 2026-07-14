//! Canonical adapter from syntax-aware foreign recipes to package plans.

use crate::domain::Toolchain;
use crate::foreign::{
    canonicalize_eb_package_name, guess_easyblock, map_dep_name_to_eb, ForeignFormat,
    ForeignRecipe, ForeignRuleKind,
};
use crate::package::{
    BuildSpec, DependencyIntent, DependencyRole, OutputRequest, PackageMetadata, PackageOrigin,
    PackagePlan, PackageRule, PackageRuleKind, ProductProfile, Residual, ResidualSeverity,
    ResidualStage, SourceArtifact, PACKAGE_SCHEMA_VERSION,
};
use std::collections::BTreeMap;

pub fn package_plan_from_foreign(recipe: &ForeignRecipe, toolchain: &Toolchain) -> PackagePlan {
    let package_name = canonicalize_eb_package_name(&recipe.name);
    let sources = recipe
        .sources
        .iter()
        .map(|source| SourceArtifact {
            url: source.url.clone(),
            filename: source.filename.clone(),
            sha256: source.sha256.clone(),
            git: source.git.clone(),
            tag: source.tag.clone(),
            commit: source.commit.clone(),
            target_directory: source.target_directory.clone(),
            provenance: Vec::new(),
        })
        .collect();

    let dependencies = recipe
        .dependencies
        .iter()
        .enumerate()
        .map(|(index, dependency)| {
            let eb_name = map_dep_name_to_eb(&dependency.name);
            DependencyIntent {
                id: format!("dep:{index}:{name}", name = dependency.name),
                name: dependency.name.clone(),
                eb_name: Some(eb_name.clone()),
                constraint: dependency.pin.clone(),
                roles: dependency_roles(&dependency.role),
                condition: dependency.condition.clone(),
                virtual_capability: is_canonical_virtual(&eb_name).then_some(eb_name),
                provenance: dependency.provenance.clone(),
            }
        })
        .collect();

    let rules = recipe
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| PackageRule {
            id: format!("rule:{index}:{}", rule.spec),
            kind: match rule.kind {
                ForeignRuleKind::Conflict => PackageRuleKind::Conflict,
                ForeignRuleKind::Requirement => PackageRuleKind::Requirement,
            },
            spec: rule.spec.clone(),
            when: rule.when.clone(),
            condition: rule.condition.clone(),
            message: rule.message.clone(),
            provenance: rule.provenance.clone(),
        })
        .collect();

    let mut easyblock_notes = Vec::new();
    let easyblock = guess_easyblock(recipe, &mut easyblock_notes);
    let mut features = BTreeMap::new();
    let mut parameters = BTreeMap::new();
    for variant in &recipe.variants {
        match variant.default.as_deref() {
            Some(value) if value.eq_ignore_ascii_case("true") => {
                features.insert(variant.name.clone(), true);
            }
            Some(value) if value.eq_ignore_ascii_case("false") => {
                features.insert(variant.name.clone(), false);
            }
            Some(value) => {
                parameters.insert(variant.name.clone(), value.to_string());
            }
            None => {}
        }
    }

    let config_options = recipe.configopts.iter().cloned().collect::<Vec<_>>();
    let profile = ProductProfile {
        name: "default".into(),
        default: true,
        versionsuffix: Vec::new(),
        features,
        parameters,
        toolchain_options: BTreeMap::new(),
        config_options: config_options.clone(),
        verification_commands: Vec::new(),
    };

    let mut residuals = Vec::new();
    for (index, note) in recipe
        .notes
        .iter()
        .chain(easyblock_notes.iter())
        .filter(|note| {
            let note = note.to_ascii_lowercase();
            note.contains("residual") || note.contains("dynamic") || note.contains("no sha256")
        })
        .enumerate()
    {
        residuals.push(Residual {
            id: format!("foreign-note:{index}"),
            stage: ResidualStage::Parse,
            category: "foreign-metadata".into(),
            severity: ResidualSeverity::Judgment,
            summary: note.clone(),
            evidence: None,
            provenance: None,
        });
    }
    if recipe.sources.iter().any(|source| source.sha256.is_none()) {
        residuals.push(Residual {
            id: "source:missing-sha256".into(),
            stage: ResidualStage::Normalize,
            category: "checksum".into(),
            severity: ResidualSeverity::Blocking,
            summary: "one or more source artifacts have no sha256".into(),
            evidence: None,
            provenance: None,
        });
    }

    PackagePlan {
        schema_version: PACKAGE_SCHEMA_VERSION,
        origin: match recipe.format {
            ForeignFormat::CondaForge => PackageOrigin::CondaForge,
            ForeignFormat::Spack => PackageOrigin::Spack,
        },
        package: PackageMetadata {
            name: package_name,
            version: recipe.version.clone(),
            homepage: recipe.homepage.clone(),
            description: recipe
                .description
                .clone()
                .or_else(|| recipe.summary.clone()),
            license: recipe.license.clone(),
        },
        sources,
        dependencies,
        rules,
        build: BuildSpec {
            toolchain: toolchain.clone(),
            easyblock: Some(easyblock),
            build_systems: recipe.build_system_hints.clone(),
            config_options,
            moduleclass: None,
            patches: recipe.patches.clone(),
        },
        profiles: vec![profile],
        outputs: vec![OutputRequest {
            profile: "default".into(),
            stack: toolchain.label(),
        }],
        residuals,
    }
}

fn dependency_roles(role: &str) -> Vec<DependencyRole> {
    let mut roles = Vec::new();
    for value in role.split('+').map(str::trim) {
        let role = match value {
            "build" => Some(DependencyRole::Build),
            "host" => Some(DependencyRole::Host),
            "run" => Some(DependencyRole::Run),
            "test" => Some(DependencyRole::Test),
            _ => None,
        };
        if let Some(role) = role {
            if !roles.contains(&role) {
                roles.push(role);
            }
        }
    }
    if roles.is_empty() {
        roles.push(DependencyRole::Run);
    }
    roles
}

fn is_canonical_virtual(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "mpi" | "blas" | "lapack" | "fftw" | "fftw-api" | "c" | "cxx" | "fortran"
    )
}
