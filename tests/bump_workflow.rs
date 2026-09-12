use eb_stack::package::{
    PackageOrigin, StackPin, StackPinMode, StackPolicy, STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::{
    find_named_easyconfig, find_sibling_package_config, plan_package_bump, resolve_easyconfig_str,
    with_outdir_overlay, write_package_bundle, BumpPackageRequest, Toolchain,
};
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
        strict_patches: false,
        package_layers: Vec::new(),
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
        strict_patches: false,
        package_layers: Vec::new(),
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
        strict_patches: false,
        package_layers: Vec::new(),
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
        strict_patches: false,
        package_layers: Vec::new(),
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

#[test]
fn version_bump_adopts_the_same_version_siblings_patch_block() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Beta-1.0-GCCcore-14.3.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Beta'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
         sources = ['beta-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         patches = ['Beta-1.0_old-fix.patch', 'portable-fix.patch']\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    // The maintainer already ships 2.0 under another toolchain, with an
    // evolved patch set carrying a level tuple that must survive verbatim.
    fs::write(
        robot.join("Beta-2.0-GCCcore-13.3.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'Beta'\nversion = '2.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
         sources = ['beta-2.0.tar.gz']\n\
         checksums = ['bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb']\n\
         patches = [\n    'portable-fix.patch',\n    ('Beta-2.0_new-fix.patch', 1),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("sibling recipe");
    let toolchain = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("2.0".into()),
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
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("bump with sibling evidence");

    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains("('Beta-2.0_new-fix.patch', 1)"),
        "sibling tuple entry adopted verbatim: {text}"
    );
    assert!(
        !text.contains("Beta-1.0_old-fix.patch"),
        "dropped patch removed: {text}"
    );

    let decisions: Vec<&str> = bundle
        .plan
        .residuals
        .iter()
        .filter(|r| r.category == "patch-decision")
        .map(|r| r.summary.as_str())
        .collect();
    assert!(
        decisions
            .iter()
            .any(|s| s.starts_with("carry portable-fix.patch")),
        "{decisions:?}"
    );
    assert!(
        decisions
            .iter()
            .any(|s| s.starts_with("adopt Beta-2.0_new-fix.patch")),
        "{decisions:?}"
    );
    assert!(
        decisions
            .iter()
            .any(|s| s.starts_with("drop Beta-1.0_old-fix.patch")),
        "{decisions:?}"
    );
    // The blanket review warning is superseded by per-patch evidence.
    assert!(
        !bundle
            .plan
            .residuals
            .iter()
            .any(|r| r.summary.contains("patches were not modified")),
        "blanket warning still present"
    );
}

#[test]
fn strict_patches_fails_on_a_version_pinned_patch_without_sibling_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gamma-1.0-GCCcore-14.3.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Gamma'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
         sources = ['gamma-1.0.tar.gz']\n\
         checksums = ['cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc']\n\
         patches = ['Gamma-1.0_fix.patch']\nmoduleclass = 'tools'\n",
    )
    .expect("source recipe");
    // The tree knows nothing about 2.0; the pinned patch name is undecidable.
    fs::write(
        robot.join("Unrelated-9.9-GCCcore-15.2.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'Unrelated'\nversion = '9.9'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Filler'\n\
         toolchain = {'name': 'GCCcore', 'version': '15.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'tools'\n",
    )
    .expect("filler recipe");
    let toolchain = Toolchain {
        name: "GCCcore".into(),
        version: "15.2.0".into(),
    };
    let request = BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("2.0".into()),
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
        strict_patches: true,
        package_layers: Vec::new(),
    };
    let error = plan_package_bump(&request).expect_err("undecided patch must fail strict mode");
    assert!(
        error.to_string().contains("Gamma-1.0_fix.patch"),
        "error names the patch: {error}"
    );
}

/// Moving a recipe onto an older generation must not carry the newer
/// generation's dependency pins over as floors. QMCPACK 4.3.0 at foss/2026.1
/// pins Boost 1.90.0 because that is what 2026.1 ships; retargeting it onto
/// foss/2025a, where the newest Boost is 1.88.0, used to fail the solve with
/// "unresolved dependency Boost >=1.90.0" even though QMCPACK itself asks for
/// 1.70.0.
#[test]
fn easybuild_bump_onto_an_older_generation_drops_the_source_generation_pins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-foss-2026.1.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2026.1'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('RuntimeLib', '1.90.0')]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("RuntimeLib-1.88.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'RuntimeLib'\nversion = '1.88.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Older generation candidate'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("older generation candidate");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
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
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("retarget onto the older generation");
    let dependency = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "RuntimeLib")
        .expect("RuntimeLib lock");
    assert_eq!(dependency.version, "1.88.0");
    assert_eq!(dependency.toolchain.version, "2025a");
}

/// The floor still applies when the generation does not move: a plain version
/// bump within one generation must not silently regress a dependency.
#[test]
fn easybuild_bump_within_one_generation_keeps_the_dependency_floor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-foss-2025a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('RuntimeLib', '2.0')]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    for version in ["1.0", "2.0"] {
        fs::write(
            robot.join(format!("RuntimeLib-{version}-foss-2025a.eb")),
            format!(
                "easyblock = 'ConfigureMake'\nname = 'RuntimeLib'\nversion = '{version}'\n\
                 homepage = 'https://example.invalid/'\ndescription = 'Candidate'\n\
                 toolchain = {{'name': 'foss', 'version': '2025a'}}\n\
                 sources = []\nchecksums = []\nmoduleclass = 'lib'\n"
            ),
        )
        .expect("candidate");
    }
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    };

    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("1.1".into()),
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
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("same generation bump");
    let dependency = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "RuntimeLib")
        .expect("RuntimeLib lock");
    assert_eq!(dependency.version, "2.0");
}

#[test]
fn version_bump_drops_a_direct_dep_with_no_candidate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gamma-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Gamma'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['gamma-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('KeptLib', '1.0'),\n    ('VanishedLib', '20211028'),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("KeptLib-1.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'KeptLib'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Kept'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("kept candidate");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("1.7.0".into()),
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
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("version bump with a vanished dep");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        !text.contains("VanishedLib"),
        "vanished dep still emitted:\n{text}"
    );
    assert!(
        text.contains("('KeptLib', '1.0')"),
        "kept dep missing:\n{text}"
    );
    assert!(
        bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "unresolved-generation-dep"
                && residual.summary.contains("VanishedLib")),
        "missing dropped-dep residual: {:?}",
        bundle.plan.residuals
    );
}

#[test]
fn package_config_exclude_drops_dep_on_toolchain_only_bump() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Delta-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Delta'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['delta-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('KeptLib', '1.0'),\n    ('VanishedLib', '20211028'),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("KeptLib-1.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'KeptLib'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Kept'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("kept candidate");
    let config_path = temp.path().join("delta.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[dependencies]\nexclude_from_solve = [\"VanishedLib\"]\n",
    )
    .expect("package config");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
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
        strict_patches: false,
        package_layers: vec![PackageConfigLayer::from_path(&config_path).expect("load layer")],
    })
    .expect("toolchain bump with excluded dep");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        !text.contains("VanishedLib"),
        "excluded dep still emitted:\n{text}"
    );
    assert!(
        text.contains("('KeptLib', '1.0')"),
        "kept dep missing:\n{text}"
    );
    assert!(
        !bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "unresolved-generation-dep"
                && residual.summary.contains("VanishedLib")),
        "excluded drop must not be blocking: {:?}",
        bundle.plan.residuals
    );
}

#[test]
fn package_config_upserts_modulename_and_commit_on_version_bump() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("SynthPy-0.1-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'PythonPackage'\nname = 'SynthPy'\n\
         local_commit_id = 'deadbeef'\nversion = '0.1'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic python'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = [{'git_config': {'commit': local_commit_id}}]\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('Python', '3.11.3'),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("Python-3.13.1-GCCcore-14.2.0.eb"),
        "easyblock = 'Python'\nname = 'Python'\nversion = '3.13.1'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Python'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lang'\n",
    )
    .expect("python candidate");
    let config_path = temp.path().join("synthpy.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[build.easyconfig_parameters]\n\
         local_commit_id = \"cafebabe0123\"\n\
         options = { modulename = \"synth\" }\n",
    )
    .expect("package config");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("0.3.1".into()),
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
        strict_patches: false,
        package_layers: vec![PackageConfigLayer::from_path(&config_path).expect("load layer")],
    })
    .expect("version bump with package extras");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains("local_commit_id = 'cafebabe0123'"),
        "commit not restored from package config:\n{text}"
    );
    assert!(
        text.contains("'modulename': 'synth'"),
        "modulename missing:\n{text}"
    );
    assert!(
        text.contains("checksums = ['']") || text.contains("checksums = [\"\"]"),
        "stale checksum must be cleared:\n{text}"
    );
}

#[test]
fn toolchain_only_bump_copies_a_sibling_patch_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir).expect("source directory");
    let source = src_dir.join("Epsilon-1.0-foss-2023a.eb");
    let patch = src_dir.join("Epsilon-1.0_fix.patch");
    fs::write(&patch, "--- a/x\n+++ b/x\n").expect("patch file");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Epsilon'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['epsilon-1.0.tar.gz']\n\
         patches = ['Epsilon-1.0_fix.patch']\n\
         checksums = [\n    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\n    '922dd52a81ff1c3d456cb861de7ad959496295c809971e848d414d3cdfe3fb23',\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
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
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("toolchain bump");
    let out = temp.path().join("out");
    let written = write_package_bundle(&bundle, &out).expect("write bundle");
    assert!(
        written
            .patches
            .iter()
            .any(|path| path.ends_with("Epsilon-1.0_fix.patch")),
        "patch not copied: {:?}",
        written.patches
    );
    assert!(
        bundle.easyconfigs[0].text.contains("Epsilon-1.0_fix.patch"),
        "patch list dropped:\n{}",
        bundle.easyconfigs[0].text
    );
}

#[test]
fn package_config_merge_keeps_the_source_patch_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src_dir = temp.path().join("src");
    fs::create_dir_all(&src_dir).expect("source directory");
    let source = src_dir.join("Zeta-1.0-foss-2023a.eb");
    let patch = src_dir.join("Zeta-1.0_fix.patch");
    fs::write(&patch, "--- a/x\n+++ b/x\n").expect("patch file");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Zeta'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['zeta-1.0.tar.gz']\n\
         patches = ['Zeta-1.0_fix.patch']\n\
         checksums = [\n    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\n    '922dd52a81ff1c3d456cb861de7ad959496295c809971e848d414d3cdfe3fb23',\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let config_path = temp.path().join("zeta.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[build]\npatches_mode = \"merge\"\n\n\
         [[build.patches]]\nfilename = \"Zeta-1.0_fix.patch\"\n\
         sha256 = \"922dd52a81ff1c3d456cb861de7ad959496295c809971e848d414d3cdfe3fb23\"\n",
    )
    .expect("package config");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
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
        strict_patches: false,
        package_layers: vec![PackageConfigLayer::from_path(&config_path).expect("load layer")],
    })
    .expect("toolchain bump with merged patch");
    let out = temp.path().join("out");
    let written = write_package_bundle(&bundle, &out).expect("write bundle");
    assert!(
        written
            .patches
            .iter()
            .any(|path| path.ends_with("Zeta-1.0_fix.patch")),
        "merged patch lost its source file: {:?}",
        written.patches
    );
}

#[test]
fn second_bump_sees_companions_already_in_outdir() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gamma-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Gamma'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['gamma-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('KeptLib', '1.0'),\n    ('VanishedLib', '1.0'),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("KeptLib-1.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'KeptLib'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Kept'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("kept candidate");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    };
    let first = plan_package_bump(&BumpPackageRequest {
        source: source.clone(),
        toolchain: toolchain.clone(),
        version: Some("1.7.0".into()),
        source_checksum: None,
        easyconfig_roots: vec![robot.clone()],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain: toolchain.clone(),
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("first bump");
    assert!(
        first
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "unresolved-generation-dep"
                && residual.summary.contains("VanishedLib")),
        "expected unresolved VanishedLib: {:?}",
        first.plan.residuals
    );
    let out = temp.path().join("out");
    write_package_bundle(&first, &out).expect("write first");
    let companion_dir = out.join("easyconfigs/v/VanishedLib");
    fs::create_dir_all(&companion_dir).expect("companion dir");
    fs::write(
        companion_dir.join("VanishedLib-1.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'VanishedLib'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Now present'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("companion recipe");
    let roots = with_outdir_overlay(vec![robot], &out);
    let second = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("1.7.0".into()),
        source_checksum: None,
        easyconfig_roots: roots,
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
        strict_patches: false,
        package_layers: Vec::new(),
    })
    .expect("second bump");
    assert!(
        !second
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "unresolved-generation-dep"
                && residual.summary.contains("VanishedLib")),
        "overlay companion still unresolved: {:?}",
        second.plan.residuals
    );
    assert!(
        second.easyconfigs[0].text.contains("VanishedLib"),
        "companion not restored:\n{}",
        second.easyconfigs[0].text
    );
}

#[test]
fn finds_robot_easyconfig_and_sibling_package_toml() {
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    let named = robot.join("k/KeptLib");
    fs::create_dir_all(&named).expect("named dir");
    let recipe = named.join("KeptLib-1.0-foss-2023a.eb");
    fs::write(&recipe, "name = 'KeptLib'\n").expect("recipe");
    let found = find_named_easyconfig(&[robot], "KeptLib").expect("find");
    assert_eq!(found, recipe);
    let cfg_dir = temp.path().join("cfg");
    fs::create_dir_all(&cfg_dir).expect("cfg");
    let sibling = cfg_dir.join("keptlib.toml");
    fs::write(&sibling, "schema_version = 1\n").expect("toml");
    let parent = cfg_dir.join("app.toml");
    fs::write(&parent, "schema_version = 1\n").expect("parent");
    let config = find_sibling_package_config(&[parent], "KeptLib").expect("sibling");
    assert_eq!(config, sibling);
}
