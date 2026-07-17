use eb_stack::package::{
    PackageOrigin, StackPin, StackPinMode, StackPolicy, STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::{plan_package_bump, resolve_easyconfig_str, BumpPackageRequest, Toolchain};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
}

#[test]
fn easybuild_bump_produces_sbom_resolvo_lock_and_recipe() {
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source: fixture("tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb"),
        toolchain: toolchain.clone(),
        version: None,
        source_checksum: None,
        easyconfig_roots: vec![fixture("tests/repro_fixtures/universe_foss_2024a")],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect("canonical bump");
    assert_eq!(bundle.plan.origin, PackageOrigin::EasyBuild);
    assert_eq!(bundle.plan.package.name, "GROMACS");
    assert_eq!(bundle.sbom["bomFormat"], "CycloneDX");
    assert_eq!(bundle.locks.len(), 1);
    assert_eq!(bundle.locks[0].solver, "resolvo");
    assert!(bundle.locks[0]
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "CMake" && dependency.version == "3.29.3"));
    assert_eq!(bundle.easyconfigs.len(), 1);
    let recipe = resolve_easyconfig_str(&bundle.easyconfigs[0].text).expect("parse bumped recipe");
    assert_eq!(recipe.toolchain.name, "foss");
    assert_eq!(recipe.toolchain.version, "2024a");
    assert!(recipe
        .builddependencies
        .iter()
        .any(|dependency| dependency.name == "CMake" && dependency.version == "3.29.3"));
}

#[test]
fn easybuild_bump_does_not_select_newer_system_candidate_for_implicit_dependency() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-GCCcore-14.3.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         builddependencies = [('BuildTool', '2.0')]\nmoduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("BuildTool-3.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'BuildTool'\nversion = '3.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'System build tool'\n\
         toolchain = SYSTEM\nsources = []\nchecksums = []\nmoduleclass = 'tools'\n",
    )
    .expect("system candidate");
    fs::write(
        robot.join("BuildTool-2.0-GCCcore-15.2.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'BuildTool'\nversion = '2.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Toolchain build tool'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'tools'\n",
    )
    .expect("toolchain candidate");
    let toolchain = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };

    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: None,
        source_checksum: None,
        easyconfig_roots: vec![robot],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect("canonical bump");
    let dependency = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "BuildTool")
        .expect("BuildTool lock");
    assert_eq!(dependency.version, "2.0");
    assert_eq!(dependency.toolchain.name, "GCCcore");
    assert_eq!(dependency.toolchain.version, "15.2.0");
}

#[test]
fn easybuild_bump_retargets_explicit_dependency_toolchain_family() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-foss-2025b.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2025b'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('RuntimeLib', '1.0', '', ('gfbf', '2025b'))]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("RuntimeLib-2.0-foss-2026.1.eb"),
        "easyblock = 'ConfigureMake'\nname = 'RuntimeLib'\nversion = '2.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Composite candidate'\n\
         toolchain = {'name': 'foss', 'version': '2026.1'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("composite candidate");
    fs::write(
        robot.join("RuntimeLib-1.1-gfbf-2026.1.eb"),
        "easyblock = 'ConfigureMake'\nname = 'RuntimeLib'\nversion = '1.1'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Subtoolchain candidate'\n\
         toolchain = {'name': 'gfbf', 'version': '2026.1'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("subtoolchain candidate");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    };

    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: None,
        source_checksum: None,
        easyconfig_roots: vec![robot],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect("canonical bump");
    let dependency = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "RuntimeLib")
        .expect("RuntimeLib lock");
    assert_eq!(dependency.version, "1.1");
    assert_eq!(dependency.toolchain.name, "gfbf");
    assert_eq!(dependency.toolchain.version, "2026.1");
    assert!(
        bundle.easyconfigs[0]
            .text
            .contains("('RuntimeLib', '1.1', '', ('gfbf', '2026.1'))"),
        "emitted recipe did not retarget the explicit tuple:\n{}",
        bundle.easyconfigs[0].text
    );
}

#[test]
fn easybuild_bump_makes_cross_generation_stack_selection_explicit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-foss-2025b.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2025b'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('PinnedLib', '1.0')]\nmoduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("PinnedLib-1.2-gfbf-2024a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'PinnedLib'\nversion = '1.2'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Pinned candidate'\n\
         toolchain = {'name': 'gfbf', 'version': '2024a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("pinned candidate");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    };

    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: None,
        source_checksum: None,
        easyconfig_roots: vec![robot],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "site".into(),
            toolchain,
            pins: vec![StackPin {
                name: "PinnedLib".into(),
                version_requirement: "==1.2".into(),
                toolchain: Some(Toolchain {
                    name: "gfbf".into(),
                    version: "2024a".into(),
                }),
                versionsuffix: None,
                mode: StackPinMode::Preferred,
                source: Some("site stack".into()),
            }],
            exclusions: Vec::new(),
        },
    })
    .expect("canonical bump");
    assert!(
        bundle.easyconfigs[0]
            .text
            .contains("('PinnedLib', '1.2', '', ('gfbf', '2024a'))"),
        "emitted recipe did not encode the cross-generation lock:\n{}",
        bundle.easyconfigs[0].text
    );
}
