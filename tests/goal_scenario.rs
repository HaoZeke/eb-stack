//! End-to-end acceptance for the two motivating foreign package workflows.

use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::version::matches_req;
use eb_stack::{
    inspect_new_package, lint_style, plan_new_package, resolve_easyconfig_file,
    write_package_bundle, ForeignFormat, NewPackageRequest, Toolchain,
};
use std::collections::BTreeSet;
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
        name: "acceptance".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    }
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

fn synthetic_robot(plan: &eb_stack::package::PackagePlan, root: &Path) {
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
        let filename = format!("{name}-{version}-foss-2026.1.eb");
        std::fs::write(
            root.join(filename),
            format!(
                "name = {name:?}\nversion = {version:?}\ntoolchain = {{'name': 'foss', 'version': '2026.1'}}\n"
            ),
        )
        .expect("synthetic robot candidate");
    }
}

fn run_package(
    source: &str,
    format: ForeignFormat,
    package_config: &str,
    expected_name: &str,
    expected_profiles: usize,
    source_checksums: Vec<String>,
) {
    let source = repo().join(source);
    let package =
        PackageConfigLayer::from_path(&repo().join(package_config)).expect("package config");
    let (inspected, inspected_sbom) = inspect_new_package(
        &source,
        Some(format),
        &toolchain(),
        std::slice::from_ref(&package),
    )
    .expect("inspect package");
    assert_eq!(inspected.package.name, expected_name);
    assert_eq!(inspected.profiles.len(), expected_profiles);
    assert_eq!(inspected_sbom["bomFormat"], "CycloneDX");

    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    synthetic_robot(&inspected, &robot);
    let bundle = plan_new_package(&NewPackageRequest {
        source,
        format: Some(format),
        toolchain: toolchain(),
        source_checksums,
        package_layers: vec![package],
        easyconfig_roots: vec![robot],
        stack_policy: policy(),
    })
    .expect("plan package");
    assert_eq!(bundle.locks.len(), expected_profiles);
    assert_eq!(bundle.easyconfigs.len(), expected_profiles);
    assert_eq!(bundle.sbom["bomFormat"], "CycloneDX");
    let rendered_sbom = bundle.sbom.to_string();
    for source in &bundle.plan.sources {
        if let Some(checksum) = source.sha256.as_deref() {
            assert!(
                rendered_sbom.contains(checksum),
                "source checksum missing from planned SBOM: {checksum}"
            );
        }
    }

    let output = temp.path().join("bundle");
    let written = write_package_bundle(&bundle, &output).expect("write bundle");
    assert!(written.manifest.is_file());
    assert!(written.sbom.is_file());
    assert_eq!(written.locks.len(), expected_profiles);
    assert_eq!(written.easyconfigs.len(), expected_profiles);
    for easyconfig in written.easyconfigs {
        let text = std::fs::read_to_string(&easyconfig).expect("read emitted recipe");
        let style_findings = lint_style(&text);
        assert!(
            style_findings.is_empty(),
            "{} has style findings: {style_findings:?}",
            easyconfig.display()
        );
        let recipe = resolve_easyconfig_file(&easyconfig).expect("reparse emitted recipe");
        assert_eq!(recipe.name, expected_name);
        assert_eq!(recipe.toolchain, toolchain());
        assert!(!recipe.checksums.is_empty(), "packaging checksum required");
    }
}

#[test]
fn conda_eon_becomes_a_resolved_package_bundle() {
    run_package(
        "fixtures/foreign_ingest/conda_eon/recipe.yaml",
        ForeignFormat::CondaForge,
        "examples/packages/eon.toml",
        "eOn",
        1,
        Vec::new(),
    );
}

#[test]
fn spack_qmcpack_becomes_two_resolved_variant_recipes() {
    run_package(
        "fixtures/foreign_ingest/spack_qmcpack/package.py",
        ForeignFormat::Spack,
        "examples/packages/qmcpack.toml",
        "QMCPACK",
        2,
        vec!["511d5f368db002f2f77504619e1ada8d4a3034200d25feef6773d12a6ed6d18e".into()],
    );
}
