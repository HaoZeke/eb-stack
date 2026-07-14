use eb_stack::package::{ConditionContext, ConditionExpr};
use eb_stack::{parse_foreign_path, ForeignFormat, ForeignRuleKind};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn feature_context(features: &[(&str, bool)]) -> ConditionContext {
    ConditionContext {
        package_version: "4.3.0".into(),
        features: features
            .iter()
            .map(|(name, enabled)| ((*name).to_string(), *enabled))
            .collect::<BTreeMap<_, _>>(),
        ..ConditionContext::default()
    }
}

#[test]
fn spack_ignores_commented_directives_and_keeps_source_spans() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse QMCPACK package.py");

    let names: Vec<&str> = recipe
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect();
    assert!(
        names.contains(&"mixed"),
        "real mixed variant missing: {names:?}"
    );
    assert!(
        !names.contains(&"+mixed"),
        "commented pseudo-directive was parsed: {names:?}"
    );

    let mpi = recipe
        .variants
        .iter()
        .find(|variant| variant.name == "mpi")
        .expect("mpi variant");
    let source = mpi.provenance.first().expect("variant provenance");
    assert!(source.span.path.ends_with("spack_qmcpack/package.py"));
    assert!(source.span.start_line > 0);
    assert!(source.original.contains("variant(\"mpi\""));
}

#[test]
fn spack_preserves_conditional_hdf5_edges() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse QMCPACK package.py");

    let hdf5: Vec<_> = recipe
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "hdf5")
        .collect();
    assert_eq!(hdf5.len(), 2, "serial and parallel HDF5 edges: {hdf5:?}");

    let serial = hdf5
        .iter()
        .find(|dependency| dependency.original_spec.as_deref() == Some("hdf5~mpi"))
        .expect("serial HDF5 edge");
    let parallel = hdf5
        .iter()
        .find(|dependency| dependency.original_spec.as_deref() == Some("hdf5+mpi"))
        .expect("parallel HDF5 edge");

    let serial_context = feature_context(&[("phdf5", false), ("mpi", true)]);
    assert!(serial.condition.evaluate(&serial_context));
    assert!(!parallel.condition.evaluate(&serial_context));

    let parallel_context = feature_context(&[("phdf5", true), ("mpi", true)]);
    assert!(!serial.condition.evaluate(&parallel_context));
    assert!(parallel.condition.evaluate(&parallel_context));
}

#[test]
fn spack_extracts_conflicts_and_requirements_as_rules() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse QMCPACK package.py");

    let conflicts = recipe
        .rules
        .iter()
        .filter(|rule| rule.kind == ForeignRuleKind::Conflict)
        .count();
    let requirements = recipe
        .rules
        .iter()
        .filter(|rule| rule.kind == ForeignRuleKind::Requirement)
        .count();
    assert_eq!(conflicts, 19, "QMCPACK conflict directives");
    assert_eq!(requirements, 2, "QMCPACK requirement directives");

    let phdf5 = recipe
        .rules
        .iter()
        .find(|rule| {
            rule.kind == ForeignRuleKind::Conflict
                && rule.spec == "+phdf5"
                && rule.when.as_deref() == Some("~mpi")
        })
        .expect("parallel-HDF5/MPI conflict");
    assert!(phdf5.provenance.span.start_line > 0);
    assert!(phdf5
        .message
        .as_deref()
        .is_some_and(|message| message.contains("MPI")));
}

#[test]
fn conda_keeps_mutually_exclusive_platform_selectors() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/conda_eon/recipe.yaml"),
        Some(ForeignFormat::CondaForge),
    )
    .expect("parse eOn recipe.yaml");

    let blas: Vec<_> = recipe
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "libblas")
        .collect();
    assert_eq!(blas.len(), 2, "win and non-win BLAS edges: {blas:?}");

    let win = ConditionContext {
        package_version: recipe.version.clone(),
        platform: Some("win".into()),
        ..ConditionContext::default()
    };
    let linux = ConditionContext {
        package_version: recipe.version.clone(),
        platform: Some("linux".into()),
        ..ConditionContext::default()
    };
    assert_eq!(
        blas.iter()
            .filter(|dependency| dependency.condition.evaluate(&win))
            .count(),
        1,
        "one Windows BLAS edge"
    );
    assert_eq!(
        blas.iter()
            .filter(|dependency| dependency.condition.evaluate(&linux))
            .count(),
        1,
        "one non-Windows BLAS edge"
    );
    assert!(blas.iter().all(|dependency| {
        dependency
            .provenance
            .first()
            .is_some_and(|source| source.span.path.ends_with("conda_eon/recipe.yaml"))
    }));
}

#[test]
fn unconditional_dependencies_use_always_condition() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        Some(ForeignFormat::Spack),
    )
    .expect("parse QMCPACK package.py");
    let libxml2 = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "libxml2")
        .expect("libxml2 dependency");
    assert_eq!(libxml2.condition, ConditionExpr::Always);
}
