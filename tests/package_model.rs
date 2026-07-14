use eb_stack::package::{
    package_plan_to_cyclonedx, BuildSpec, ConditionContext, ConditionExpr, ConditionPredicate,
    Confidence, DependencyIntent, DependencyRole, OutputRequest, PackageMetadata, PackageOrigin,
    PackagePlan, ProductProfile, Provenance, Residual, ResidualSeverity, ResidualStage, SourceSpan,
    PACKAGE_SCHEMA_VERSION,
};
use eb_stack::Toolchain;
use serde_json::Value;
use std::collections::BTreeMap;

fn provenance(line: u32, original: &str) -> Provenance {
    Provenance {
        span: SourceSpan {
            path: "fixtures/foreign_ingest/spack_qmcpack/package.py".into(),
            start_line: line,
            start_column: 5,
            end_line: line,
            end_column: 40,
        },
        extractor: "spack-static".into(),
        original: original.into(),
        confidence: Confidence::Exact,
    }
}

fn qmcpack_plan() -> PackagePlan {
    let mut features = BTreeMap::new();
    features.insert("mpi".into(), true);
    features.insert("complex".into(), false);

    PackagePlan {
        schema_version: PACKAGE_SCHEMA_VERSION,
        origin: PackageOrigin::Spack,
        package: PackageMetadata {
            name: "QMCPACK".into(),
            version: "4.3.0".into(),
            homepage: Some("https://qmcpack.org".into()),
            description: Some("Quantum Monte Carlo application".into()),
            license: Some("BSD-3-Clause".into()),
        },
        sources: Vec::new(),
        dependencies: vec![DependencyIntent {
            id: "dep:hdf5".into(),
            name: "hdf5".into(),
            eb_name: Some("HDF5".into()),
            constraint: Some(">=1.14".into()),
            roles: vec![DependencyRole::Run],
            condition: ConditionExpr::Predicate(ConditionPredicate::Feature {
                name: "mpi".into(),
                enabled: true,
            }),
            virtual_capability: None,
            provenance: vec![provenance(181, "depends_on(\"hdf5+mpi\", when=\"+phdf5\")")],
        }],
        rules: Vec::new(),
        build: BuildSpec {
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2026.1".into(),
            },
            easyblock: Some("CMakeNinja".into()),
            build_systems: vec!["CMakePackage".into()],
            config_options: vec!["-DQMC_MPI=ON".into(), "-DQMC_OMP=ON".into()],
            moduleclass: Some("chem".into()),
            patches: Vec::new(),
        },
        profiles: vec![ProductProfile {
            name: "default".into(),
            default: true,
            versionsuffix: Vec::new(),
            features,
            toolchain_options: BTreeMap::from([("usempi".into(), true), ("openmp".into(), true)]),
            config_options: vec![
                "-DQMC_MIXED_PRECISION=OFF".into(),
                "-DQMC_COMPLEX=OFF".into(),
            ],
        }],
        outputs: vec![OutputRequest {
            profile: "default".into(),
            stack: "foss-2026.1".into(),
        }],
        residuals: vec![Residual {
            id: "spack:dynamic-cmake-args:1".into(),
            stage: ResidualStage::Parse,
            category: "dynamic-build-logic".into(),
            severity: ResidualSeverity::Judgment,
            summary: "dynamic cmake_args require evaluation".into(),
            evidence: Some("def cmake_args(self):".into()),
            provenance: Some(provenance(214, "def cmake_args(self):")),
        }],
    }
}

#[test]
fn package_plan_round_trips_without_duplicate_build_config() {
    let plan = qmcpack_plan();
    let json = serde_json::to_value(&plan).expect("serialize package plan");
    assert_eq!(json["schema_version"], PACKAGE_SCHEMA_VERSION);
    assert!(json.get("build").is_some());
    assert!(
        json.get("build_config").is_none(),
        "canonical plan must have one build representation: {json}"
    );

    let decoded = PackagePlan::from_json_str(
        &serde_json::to_string_pretty(&json).expect("render package plan"),
    )
    .expect("read version-one package plan");
    assert_eq!(decoded, plan);
}

#[test]
fn package_plan_rejects_unknown_schema_version() {
    let mut json = serde_json::to_value(qmcpack_plan()).expect("serialize");
    json["schema_version"] = Value::from(99);
    let error = PackagePlan::from_json_str(&json.to_string()).expect_err("unknown schema");
    assert!(
        error
            .to_string()
            .contains("unsupported package schema version 99"),
        "unexpected error: {error}"
    );
}

#[test]
fn conditions_evaluate_against_materialized_profile_and_stack() {
    let expression = ConditionExpr::All(vec![
        ConditionExpr::Predicate(ConditionPredicate::PackageVersion {
            requirement: ">=4.0".into(),
        }),
        ConditionExpr::Predicate(ConditionPredicate::Feature {
            name: "mpi".into(),
            enabled: true,
        }),
        ConditionExpr::Not(Box::new(ConditionExpr::Predicate(
            ConditionPredicate::Feature {
                name: "cuda".into(),
                enabled: true,
            },
        ))),
        ConditionExpr::Predicate(ConditionPredicate::Toolchain {
            name: "foss".into(),
            version: Some("2026.1".into()),
        }),
    ]);

    let context = ConditionContext {
        package_version: "4.3.0".into(),
        features: BTreeMap::from([("mpi".into(), true), ("cuda".into(), false)]),
        toolchain: Some(Toolchain {
            name: "foss".into(),
            version: "2026.1".into(),
        }),
        ..ConditionContext::default()
    };
    assert!(expression.evaluate(&context));

    let cuda = ConditionContext {
        features: BTreeMap::from([("mpi".into(), true), ("cuda".into(), true)]),
        ..context
    };
    assert!(!expression.evaluate(&cuda));
}

#[test]
fn canonical_plan_writes_typed_cyclonedx_components() {
    let sbom = package_plan_to_cyclonedx(&qmcpack_plan()).expect("typed CycloneDX SBOM");
    assert_eq!(sbom["bomFormat"], "CycloneDX");
    let components = sbom["components"].as_array().expect("components array");
    let names: Vec<&str> = components
        .iter()
        .filter_map(|component| component["name"].as_str())
        .collect();
    assert!(
        names.contains(&"QMCPACK"),
        "root component missing: {names:?}"
    );
    assert!(
        names.contains(&"HDF5"),
        "dependency component missing: {names:?}"
    );
}
