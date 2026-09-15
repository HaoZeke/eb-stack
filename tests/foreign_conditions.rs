use eb_stack::package::{ConditionContext, ConditionExpr};
use eb_stack::{parse_foreign_path, parse_foreign_str, ForeignFormat, ForeignRuleKind};
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
fn conda_classic_selectors_control_cuda_dependencies() {
    let recipe = parse_foreign_path(
        &fixture("fixtures/foreign_ingest/conda_lammps/meta.yaml"),
        Some(ForeignFormat::CondaForge),
    )
    .expect("parse conda-forge LAMMPS recipe");
    let cuda_versions = recipe
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "cuda-version")
        .collect::<Vec<_>>();
    assert_eq!(cuda_versions.len(), 2, "build and host CUDA markers");
    assert!(cuda_versions
        .iter()
        .all(|dependency| !dependency.condition.evaluate(&ConditionContext::default())));

    let enabled = ConditionContext {
        variables: BTreeMap::from([("cuda_compiler_version".into(), "12.8".into())]),
        ..ConditionContext::default()
    };
    assert!(cuda_versions
        .iter()
        .all(|dependency| dependency.condition.evaluate(&enabled)));
}

#[test]
fn conda_unix_and_cpu_selectors_lower_as_not_windows_and_architecture() {
    let recipe = parse_foreign_str(
        ForeignFormat::CondaForge,
        r#"
package:
  name: selector-fixture
  version: 1.0
source:
  url: https://example.invalid/selector-fixture-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  build:
    - make  # [unix]
    - foo  # [aarch64]
"#,
    )
    .expect("parse unix/cpu selectors");
    let make = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "make")
        .expect("make dependency");
    let foo = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "foo")
        .expect("foo dependency");

    let linux = ConditionContext {
        platform: Some("linux".into()),
        ..ConditionContext::default()
    };
    assert!(
        make.condition.evaluate(&linux),
        "unix must hold on linux: {:?}",
        make.condition
    );
    let win = ConditionContext {
        platform: Some("win".into()),
        ..ConditionContext::default()
    };
    assert!(
        !make.condition.evaluate(&win),
        "unix must fail on win: {:?}",
        make.condition
    );

    let aarch64 = ConditionContext {
        architecture: Some("aarch64".into()),
        platform: Some("linux".into()),
        ..ConditionContext::default()
    };
    assert!(
        foo.condition.evaluate(&aarch64),
        "aarch64 must hold when architecture is aarch64: {:?}",
        foo.condition
    );
    assert!(
        !foo.condition.evaluate(&linux),
        "aarch64 must fail without architecture: {:?}",
        foo.condition
    );
}

#[test]
fn conda_not_py3k_is_not_an_always_on_dependency() {
    let recipe = parse_foreign_str(
        ForeignFormat::CondaForge,
        r#"
package:
  name: selector-fixture
  version: 1.0
source:
  url: https://example.invalid/selector-fixture-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  run:
    - python
    - futures  # [not py3k]
"#,
    )
    .expect("parse conda not-py3k selector");
    let futures = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "futures")
        .expect("futures dependency");
    assert!(
        !futures.condition.evaluate(&ConditionContext::default()),
        "not py3k must not admit under an empty context: {:?}",
        futures.condition
    );

    let plan = eb_stack::package_plan_from_foreign(
        &recipe,
        &eb_stack::Toolchain {
            name: "foss".into(),
            version: "2025a".into(),
        },
    );
    let futures_intent = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "futures")
        .expect("futures intent");
    assert!(
        futures_intent.solver_excluded,
        "undecidable not-py3k must be solver-excluded"
    );
    let materialized = eb_stack::package::materialize_profile(
        &plan,
        "default",
        &eb_stack::package::ProfileEnvironment::default(),
    )
    .expect("materialize default");
    assert!(
        materialized
            .dependencies
            .iter()
            .all(|dependency| dependency.name != "futures"),
        "not py3k must not become an always-on dependency: {:?}",
        materialized
            .dependencies
            .iter()
            .map(|dependency| &dependency.name)
            .collect::<Vec<_>>()
    );
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

#[test]
fn spack_inherits_scoped_when_conditions() {
    let recipe = parse_foreign_str(
        ForeignFormat::Spack,
        r#"
class Scoped(Package):
    homepage = "https://example.invalid/scoped"
    url = "https://example.invalid/scoped-1.0.tar.gz"

    version("1.0", sha256="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")

    for dep_condition in ("+foo", "+bar"):
        with when(dep_condition):
            depends_on("resolved")

    with when(runtime_condition):
        depends_on("opaque")
"#,
    )
    .expect("parse scoped Spack conditions");

    let resolved = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "resolved")
        .expect("resolved scoped dependency");
    assert!(!resolved.condition.evaluate(&feature_context(&[])));
    assert!(resolved
        .condition
        .evaluate(&feature_context(&[("foo", true)])));
    assert!(resolved
        .condition
        .evaluate(&feature_context(&[("bar", true)])));

    let opaque = recipe
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "opaque")
        .expect("opaque scoped dependency");
    assert!(!opaque.condition.evaluate(&feature_context(&[])));
    assert!(recipe
        .notes
        .iter()
        .any(|note| note.contains("dynamic scoped when(runtime_condition)")));
}
