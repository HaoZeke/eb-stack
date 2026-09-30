//! When a package owes an easyblock: the residual, the rendered skeleton, and
//! `--emit-easyblock` through the workflow.

use eb_stack::easyblock_skeleton;
use eb_stack::eb_easyblock::defined_classes;
use eb_stack::package::{
    BuildSpec, OutputRequest, PackageMetadata, PackageOrigin, PackagePlan, ProductProfile,
    ResidualSeverity, ResidualStage, StackPolicy, PACKAGE_SCHEMA_VERSION,
    STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::package_config::{apply_package_layers, PackageConfigLayer};
use eb_stack::version::matches_req;
use eb_stack::{
    inspect_new_package, plan_new_package, plan_new_package_with, raise_missing_easyblock,
    write_package_bundle, ForeignFormat, NewPackageRequest, PackageEmitOptions, Toolchain,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn policy() -> StackPolicy {
    StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "easyblock-owed".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    }
}

fn profile(name: &str, default: bool, options: &[&str]) -> ProductProfile {
    ProductProfile {
        name: name.into(),
        default,
        versionsuffix: Vec::new(),
        platform: None,
        architecture: None,
        features: BTreeMap::new(),
        parameters: BTreeMap::new(),
        toolchain_options: BTreeMap::new(),
        config_options: options.iter().map(|option| (*option).to_string()).collect(),
        easyconfig_parameters: BTreeMap::new(),
        verification_commands: Vec::new(),
    }
}

fn plan(name: &str, easyblock: Option<&str>, profiles: Vec<ProductProfile>) -> PackagePlan {
    PackagePlan {
        schema_version: PACKAGE_SCHEMA_VERSION,
        origin: PackageOrigin::Spack,
        package: PackageMetadata {
            name: name.into(),
            version: "1.0".into(),
            upstream_version: None,
            homepage: None,
            description: None,
            license: None,
        },
        sources: Vec::new(),
        dependencies: Vec::new(),
        rules: Vec::new(),
        build: BuildSpec {
            toolchain: toolchain(),
            easyblock: easyblock.map(str::to_string),
            build_systems: Vec::new(),
            source_root: None,
            config_options: Vec::new(),
            moduleclass: None,
            patches: Vec::new(),
            easyblocks: Vec::new(),
            easyconfig_parameters: BTreeMap::new(),
        },
        outputs: profiles
            .iter()
            .map(|profile| OutputRequest {
                profile: profile.name.clone(),
                stack: "foss-2026.1".into(),
            })
            .collect(),
        profiles,
        residuals: Vec::new(),
        overlay_extensions: Vec::new(),
        package_index: Default::default(),
    }
}

fn qmcpack_plan() -> PackagePlan {
    let package = PackageConfigLayer::from_path(&repo().join("examples/packages/qmcpack.toml"))
        .expect("QMCPACK package config");
    let (plan, _sbom) = inspect_new_package(
        &repo().join("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        Some(ForeignFormat::Spack),
        &toolchain(),
        std::slice::from_ref(&package),
    )
    .expect("inspect QMCPACK");
    plan
}

fn missing_easyblock_residuals(plan: &PackagePlan) -> Vec<&eb_stack::package::Residual> {
    plan.residuals
        .iter()
        .filter(|residual| residual.category == "missing-easyblock")
        .collect()
}

#[test]
fn profiles_with_different_options_owe_the_derived_class() {
    let mut plan = plan(
        "QMCPACK",
        Some("CMakeNinja"),
        vec![
            profile("default", true, &["-DQMC_COMPLEX=OFF"]),
            profile("complex", false, &["-DQMC_COMPLEX=ON"]),
        ],
    );
    raise_missing_easyblock(&mut plan);
    raise_missing_easyblock(&mut plan);

    let residuals = missing_easyblock_residuals(&plan);
    assert_eq!(residuals.len(), 1, "{:?}", plan.residuals);
    let residual = residuals[0];
    assert_eq!(residual.id, "missing-easyblock:EB_QMCPACK");
    assert_eq!(residual.stage, ResidualStage::Emit);
    assert_eq!(residual.severity, ResidualSeverity::Judgment);
    assert!(
        residual.summary.contains("EB_QMCPACK"),
        "{}",
        residual.summary
    );
    let evidence = residual.evidence.as_deref().expect("evidence");
    assert!(
        evidence.contains("-DQMC_COMPLEX=ON (complex)"),
        "{evidence}"
    );
    assert!(
        evidence.contains("-DQMC_COMPLEX=OFF (default)"),
        "{evidence}"
    );
}

#[test]
fn profiles_with_the_same_options_and_no_hand_written_parameters_owe_nothing() {
    let mut plan = plan(
        "Plain",
        Some("CMakeMake"),
        vec![
            profile("default", true, &["-DA=ON"]),
            profile("other", false, &["-DA=ON"]),
        ],
    );
    raise_missing_easyblock(&mut plan);
    assert!(
        missing_easyblock_residuals(&plan).is_empty(),
        "{:?}",
        plan.residuals
    );
}

#[test]
fn a_layer_that_ships_the_class_owes_nothing() {
    let layers = ["seissol.toml", "seissol-easyblock.toml"].map(|name| {
        PackageConfigLayer::from_path(&repo().join("examples/packages").join(name))
            .expect("SeisSol package config")
    });
    let mut plan = plan(
        "SeisSol",
        Some("CMakeMake"),
        vec![profile("default", true, &[])],
    );
    apply_package_layers(&mut plan, &layers).expect("apply SeisSol layers");

    assert_eq!(plan.build.easyblock.as_deref(), Some("EB_SeisSol"));
    assert!(easyblock_skeleton::ships_class(&plan, "EB_SeisSol"));
    raise_missing_easyblock(&mut plan);
    assert!(
        missing_easyblock_residuals(&plan).is_empty(),
        "{:?}",
        plan.residuals
    );
}

#[test]
fn rendered_qmcpack_skeleton_parses_and_fits_flake8() {
    let plan = qmcpack_plan();
    let text = easyblock_skeleton::render(&plan).expect("render QMCPACK skeleton");

    let classes = defined_classes(&text, "qmcpack.py").expect("rendered module parses");
    let class = classes
        .iter()
        .find(|class| class.name == "EB_QMCPACK")
        .expect("EB_QMCPACK");
    assert_eq!(class.bases, ["CMakeNinja"]);
    for key in ["mpi", "complex", "mixed"] {
        assert!(
            class.extra_options.iter().any(|option| option == key),
            "missing extra_options key {key}: {:?}",
            class.extra_options
        );
    }
    assert!(text.contains("-DQMC_COMPLEX"), "{text}");
    assert!(
        text.contains("from easybuild.easyblocks.generic.cmakeninja import CMakeNinja"),
        "{text}"
    );
    for (index, line) in text.lines().enumerate() {
        assert!(
            line.chars().count() <= 120,
            "line {} is longer than 120 characters: {line}",
            index + 1
        );
        assert_eq!(
            line,
            line.trim_end(),
            "trailing whitespace on line {}",
            index + 1
        );
    }
    assert!(text.is_ascii());
}

fn satisfying_version(requirement: Option<&str>) -> String {
    let requirement = requirement.unwrap_or("*");
    let mut candidates = requirement
        .split([',', '|'])
        .map(|term| {
            term.trim()
                .trim_start_matches(['=', '>', '<', '~', '^', '@', ' '])
                .split(':')
                .next()
                .unwrap_or("")
                .trim_end_matches('*')
                .to_string()
        })
        .filter(|candidate| !candidate.is_empty())
        .collect::<Vec<_>>();
    candidates.extend(["1.0".into(), "2.0".into(), "2026.1".into(), "9999.0".into()]);
    candidates
        .into_iter()
        .find(|candidate| matches_req(candidate, requirement))
        .unwrap_or_else(|| "1.0".into())
}

fn synthetic_robot(plan: &PackagePlan, root: &Path) {
    let mut written = BTreeSet::new();
    for dependency in &plan.dependencies {
        if dependency.virtual_capability.is_some() {
            continue;
        }
        let name = dependency.eb_name.as_deref().unwrap_or(&dependency.name);
        let version = satisfying_version(dependency.constraint.as_deref());
        if !written.insert((name.to_string(), version.clone())) {
            continue;
        }
        std::fs::write(
            root.join(format!("{name}-{version}-foss-2026.1.eb")),
            format!(
                "name = {name:?}\nversion = {version:?}\ntoolchain = {{'name': 'foss', 'version': '2026.1'}}\n"
            ),
        )
        .expect("synthetic robot candidate");
    }
}

fn qmcpack_request(robot: PathBuf) -> NewPackageRequest {
    let package = PackageConfigLayer::from_path(&repo().join("examples/packages/qmcpack.toml"))
        .expect("QMCPACK package config");
    NewPackageRequest {
        source: repo().join("fixtures/foreign_ingest/spack_qmcpack/package.py"),
        format: Some(ForeignFormat::Spack),
        toolchain: toolchain(),
        source_checksums: vec![
            "511d5f368db002f2f77504619e1ada8d4a3034200d25feef6773d12a6ed6d18e".into(),
        ],
        package_layers: vec![package],
        package_index: Default::default(),
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    }
}

#[test]
fn planning_without_the_flag_raises_the_residual_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    synthetic_robot(&qmcpack_plan(), &robot);

    let bundle = plan_new_package(&qmcpack_request(robot)).expect("plan QMCPACK");

    let residuals = missing_easyblock_residuals(&bundle.plan);
    assert_eq!(residuals.len(), 1, "{:?}", bundle.plan.residuals);
    assert_eq!(residuals[0].id, "missing-easyblock:EB_QMCPACK");
    assert!(bundle.plan.build.easyblocks.is_empty());
}

#[test]
fn emit_easyblock_ships_the_module_and_the_recipes_lean_on_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    synthetic_robot(&qmcpack_plan(), &robot);
    let output = temp.path().join("bundle");

    let options = PackageEmitOptions {
        easyblock_skeleton_root: Some(output.clone()),
    };
    let bundle = plan_new_package_with(&qmcpack_request(robot), &options).expect("plan QMCPACK");

    assert_eq!(bundle.plan.build.easyblock.as_deref(), Some("EB_QMCPACK"));
    assert_eq!(bundle.plan.build.easyblocks.len(), 1);
    let artifact = &bundle.plan.build.easyblocks[0];
    assert_eq!(artifact.filename, "qmcpack.py");
    assert_eq!(artifact.sha256.as_deref().map(str::len), Some(64));
    assert!(
        missing_easyblock_residuals(&bundle.plan).is_empty(),
        "{:?}",
        bundle.plan.residuals
    );

    let written = write_package_bundle(&bundle, &output).expect("write bundle");
    let module = output.join("easyblocks/q/qmcpack.py");
    assert_eq!(written.easyblocks, vec![module.clone()]);
    let text = std::fs::read_to_string(&module).expect("read easyblock module");
    assert!(text.contains("class EB_QMCPACK(CMakeNinja):"), "{text}");
    assert!(text.contains("-DQMC_COMPLEX"), "{text}");

    assert_eq!(written.easyconfigs.len(), 2);
    for easyconfig in &written.easyconfigs {
        let recipe = std::fs::read_to_string(easyconfig).expect("read recipe");
        assert!(!recipe.contains("easyblock ="), "{recipe}");
        assert!(!recipe.contains("-DQMC_COMPLEX"), "{recipe}");
        let is_complex = easyconfig
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("complex"));
        assert_eq!(recipe.contains("complex = True"), is_complex, "{recipe}");
    }
}
