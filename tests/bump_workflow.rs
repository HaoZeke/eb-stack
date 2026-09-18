use eb_stack::package::{
    PackageOrigin, ResidualSeverity, StackPin, StackPinMode, StackPolicy,
    STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::{
    plan_package_bump, resolve_easyconfig_str, write_package_bundle, BumpPackageRequest, Toolchain,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("format digest");
            output
        })
}

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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
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
    let portable_bytes = b"portable fix\n";
    let new_fix_bytes = b"new version fix\n";
    let old_fix_bytes = b"old version fix\n";
    fs::write(temp.path().join("Beta-1.0_old-fix.patch"), old_fix_bytes).expect("old patch");
    fs::write(robot.join("portable-fix.patch"), portable_bytes).expect("portable patch");
    fs::write(robot.join("Beta-2.0_new-fix.patch"), new_fix_bytes).expect("new patch");
    let portable_hash = sha256_hex(portable_bytes);
    let new_fix_hash = sha256_hex(new_fix_bytes);
    fs::write(
        robot.join("Beta-2.0-GCCcore-13.3.0.eb"),
        format!(
            "easyblock = 'ConfigureMake'\nname = 'Beta'\nversion = '2.0'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
             toolchain = {{'name': 'GCCcore', 'version': '13.3.0'}}\n\
             sources = ['beta-2.0.tar.gz']\n\
             checksums = [\n    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',\n    '{portable_hash}',\n    '{new_fix_hash}',\n]\n\
             patches = [\n    'portable-fix.patch',\n    ('Beta-2.0_new-fix.patch', 1),\n]\n\
             moduleclass = 'tools'\n"
        ),
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
        foreign_sources: Vec::new(),
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

    let out = temp.path().join("bundle");
    let written = write_package_bundle(&bundle, &out).expect("write adopted overlay");
    let names: Vec<String> = written
        .patches
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        names.iter().any(|name| name == "Beta-2.0_new-fix.patch"),
        "adopted sibling patch missing from overlay: {names:?}"
    );
    assert!(
        names.iter().any(|name| name == "portable-fix.patch"),
        "carried patch missing from overlay: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name == "Beta-1.0_old-fix.patch"),
        "source-version patch still copied: {names:?}"
    );
    let overlay_dir = written.easyconfigs[0].parent().expect("recipe parent");
    assert!(overlay_dir.join("Beta-2.0_new-fix.patch").is_file());
    assert!(overlay_dir.join("portable-fix.patch").is_file());
    assert!(!overlay_dir.join("Beta-1.0_old-fix.patch").is_file());
    assert_eq!(
        fs::read(overlay_dir.join("Beta-2.0_new-fix.patch")).expect("read new patch"),
        new_fix_bytes
    );
    assert_eq!(
        fs::read(overlay_dir.join("portable-fix.patch")).expect("read portable patch"),
        portable_bytes
    );
}

#[test]
fn version_bump_pins_mpi_test_ranks_on_a_cuda_usempi_policy_package() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("GROMACS-2024.3-foss-2024a-CUDA-12.6.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'GROMACS'\nversion = '2024.3'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         toolchainopts = {'openmp': True, 'usempi': True}\n\
         versionsuffix = '-CUDA-12.6.0'\n\
         sources = ['gromacs-2024.3.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('CUDA', '12.6.0', '', SYSTEM)]\n\
         moduleclass = 'bio'\n",
    )
    .expect("source recipe");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("2024.4".into()),
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
        foreign_sources: Vec::new(),
    })
    .expect("cuda usempi version bump");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains("mpi_numprocs = 2"),
        "GPU MPI tests must not inherit $parallel as NUMPROC:\n{text}"
    );
    assert!(
        !text.contains("skipsteps"),
        "a missing rank pin is not a reason to drop the test step:\n{text}"
    );
}

#[test]
fn version_bump_does_not_invent_mpi_test_ranks_off_policy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-foss-2024a-CUDA-12.6.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         toolchainopts = {'openmp': True, 'usempi': True}\n\
         versionsuffix = '-CUDA-12.6.0'\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('CUDA', '12.6.0', '', SYSTEM)]\n\
         moduleclass = 'bio'\n",
    )
    .expect("source recipe");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
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
        foreign_sources: Vec::new(),
    })
    .expect("off-policy cuda bump");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        !text.contains("mpi_numprocs"),
        "only overlay-policy names get a rank pin:\n{text}"
    );
}

#[test]
fn version_bump_keeps_cli_source_checksum_when_adopting_sibling_checksums() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Beta-1.0-GCCcore-14.3.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    let cli = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let sib = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let patch = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Beta'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
         sources = ['beta-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         patches = ['old.patch']\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("Beta-2.0-GCCcore-13.3.0.eb"),
        format!(
            "easyblock = 'ConfigureMake'\nname = 'Beta'\nversion = '2.0'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
             toolchain = {{'name': 'GCCcore', 'version': '13.3.0'}}\n\
             sources = ['beta-2.0.tar.gz']\n\
             checksums = ['{sib}', '{patch}']\n\
             patches = ['new.patch']\n\
             moduleclass = 'tools'\n"
        ),
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
        source_checksum: Some(cli.into()),
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
        foreign_sources: Vec::new(),
    })
    .expect("bump with CLI digest and sibling checksums");

    assert_eq!(bundle.plan.sources[0].sha256.as_deref(), Some(cli));
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains(&format!("'{cli}'")) || text.contains(&format!("\"{cli}\"")),
        "CLI digest must keep the source slot:\n{text}"
    );
    assert!(
        !text.contains(sib),
        "sibling source digest must not overwrite CLI:\n{text}"
    );
    assert!(
        text.contains(patch),
        "sibling patch hash may still be adopted:\n{text}"
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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
    })
    .expect("same generation bump");
    assert!(
        bundle
            .plan
            .residuals
            .iter()
            .all(|residual| residual.category != "unresolved-generation-dep"),
        "same-generation version bump must not invent generation holes: {:?}",
        bundle.plan.residuals
    );
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
        foreign_sources: Vec::new(),
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
                && residual.severity == ResidualSeverity::Blocking
                && residual.summary.contains("VanishedLib")),
        "missing unresolved-generation-dep residual: {:?}",
        bundle.plan.residuals
    );
}

#[test]
fn version_bump_drops_a_one_line_vanished_dep() {
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
         dependencies = [('KeptLib', '1.0'), ('VanishedLib', '20211028')]\n\
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
        foreign_sources: Vec::new(),
    })
    .expect("version bump with a one-line vanished dep");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        !text.contains("VanishedLib"),
        "vanished dep still emitted:\n{text}"
    );
    assert!(
        text.contains("('KeptLib', '1.0')"),
        "kept dep missing:\n{text}"
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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
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
    assert!(
        bundle.plan.residuals.iter().any(|residual| {
            residual.id == "source:missing-sha256"
                && residual.severity == ResidualSeverity::Judgment
        }),
        "EasyBuild cleared digest is judgment, not blocking: {:?}",
        bundle.plan.residuals
    );
    assert!(
        bundle
            .plan
            .sources
            .iter()
            .all(|source| source.sha256.is_none()),
        "plan must not keep the 0.1 sha256 after a version bump: {:?}",
        bundle
            .plan
            .sources
            .iter()
            .map(|source| source.sha256.as_deref())
            .collect::<Vec<_>>()
    );
    let sbom = serde_json::to_string(&bundle.sbom).expect("sbom json");
    assert!(
        !sbom.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        "SBOM must not publish the previous tarball digest:\n{sbom}"
    );
}

#[test]
fn version_bump_applies_package_config_source_checksum() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("SynthPy-0.1-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'PythonPackage'\nname = 'SynthPy'\nversion = '0.1'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic python'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['synthpy-0.1.tar.gz']\n\
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
    let digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let config_path = temp.path().join("synthpy.toml");
    fs::write(
        &config_path,
        format!("schema_version = 1\nsource_checksums = [\"{digest}\"]\n"),
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
        foreign_sources: Vec::new(),
    })
    .expect("version bump with inject digest");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains(&format!("checksums = ['{digest}']"))
            || text.contains(&format!("checksums = [\"{digest}\"]")),
        "package-config inject digest missing:\n{text}"
    );
}

/// CLI `--source-checksum` and a layer `source_checksums` must not write two
/// different digests. The CLI value is the later word; plan, SBOM, and recipe
/// have to carry that same digest.
#[test]
fn bump_cli_source_checksum_agrees_with_plan_and_sbom_when_a_layer_also_sets_one() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("SynthPy-0.1-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'PythonPackage'\nname = 'SynthPy'\nversion = '0.1'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic python'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['synthpy-0.1.tar.gz']\n\
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
    let cli = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let layer = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let config_path = temp.path().join("synthpy.toml");
    fs::write(
        &config_path,
        format!("schema_version = 1\nsource_checksums = [\"{layer}\"]\n"),
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
        source_checksum: Some(cli.into()),
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
        foreign_sources: Vec::new(),
    })
    .expect("bump with CLI and layer checksums");
    assert_eq!(bundle.plan.sources[0].sha256.as_deref(), Some(cli));
    assert_eq!(
        bundle.sbom["metadata"]["component"]["hashes"][0]["content"],
        cli
    );
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains(&format!("checksums = ['{cli}']"))
            || text.contains(&format!("checksums = [\"{cli}\"]")),
        "emitted recipe must use the CLI digest, not the layer:\n{text}"
    );
    assert!(
        !text.contains(layer),
        "layer digest leaked into the recipe:\n{text}"
    );
}

#[test]
fn package_config_locals_derive_binary_and_interpolated_configopts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("SeisSol-1.1.4-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'SeisSol'\nversion = '1.1.4'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['seissol-1.1.4.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         configopts = '-DORDER=4 -DHOST_ARCH=hsw -DEQUATIONS=elastic'\n\
         sanity_check_paths = {'files': ['bin/SeisSol_Release_dhsw_4_elastic'], 'dirs': []}\n\
         sanity_check_commands = ['SeisSol_Release_dhsw_4_elastic --help |grep help']\n\
         moduleclass = 'geo'\n",
    )
    .expect("source recipe");
    let config_path = temp.path().join("seissol.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[build]\n\
         config_options = [\n\
         \"-DCMAKE_BUILD_TYPE=Release\",\n\
         \"-DHOST_ARCH=hsw\",\n\
         \"-DORDER=6\",\n\
         \"-DEQUATIONS=elastic\",\n\
         \"-DPRECISION=double\",\n\
         ]\n\
         [build.easyconfig_parameters]\n\
         local_host_arch = \"hsw\"\n\
         local_order = 6\n\
         local_equations = \"elastic\"\n",
    )
    .expect("package config");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    };
    let bundle = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("1.3.2".into()),
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
        foreign_sources: Vec::new(),
    })
    .expect("derived locals");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains("local_binary = 'SeisSol_Release_d%s_%s_%s' % (local_host_arch, local_order, local_equations)"),
        "local_binary missing:\n{text}"
    );
    assert!(
        text.contains("% (local_host_arch, local_order, local_equations)"),
        "configopts placeholders must follow flag order:\n{text}"
    );
    assert!(
        text.contains("-DORDER=%s"),
        "ORDER not parameterized:\n{text}"
    );
    assert!(
        text.contains("'bin/%s' % local_binary"),
        "old binary path must be rewritten:\n{text}"
    );
    assert!(
        !text.contains("SeisSol_Release_dhsw_4_elastic"),
        "source-generation binary name leaked:\n{text}"
    );
}

#[test]
fn bump_inspects_a_spack_recipe_for_deps_the_easyconfig_never_declared() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gromacsish-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Gromacsish'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['gromacsish-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('Python', '3.11.3'),\n]\n\
         moduleclass = 'chem'\n",
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
    fs::write(
        robot.join("pybind11-2.12.0-GCCcore-14.2.0.eb"),
        "easyblock = 'PythonPackage'\nname = 'pybind11'\nversion = '2.12.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'pybind11'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("pybind11 candidate");
    fs::write(
        robot.join("CMake-3.31.0-GCCcore-14.2.0.eb"),
        "easyblock = 'CMakeMake'\nname = 'CMake'\nversion = '3.31.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'CMake'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'tools'\n",
    )
    .expect("cmake candidate");
    let foreign = temp.path().join("package.py");
    fs::write(
        &foreign,
        "from spack.package import *\n\n\
         class Gromacsish(CMakePackage):\n\
         \thomepage = 'https://example.invalid/'\n\
         \turl = 'https://example.invalid/gromacsish-1.0.tar.gz'\n\
         \tversion('1.0', sha256='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')\n\
         \tdepends_on('python')\n\
         \tdepends_on('py-pybind11')\n\
         \tdepends_on('cmake', type='build')\n",
    )
    .expect("spack recipe");
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
        foreign_sources: vec![foreign],
    })
    .expect("bump with package inspect");
    let text = &bundle.easyconfigs[0].text;
    assert!(
        text.contains("('pybind11'"),
        "inspect of the Spack recipe must add pybind11:\n{text}"
    );
    assert!(
        text.contains("builddependencies") && text.contains("('CMake'"),
        "inspect of a Spack type=build dep must emit builddependencies:\n{text}"
    );
    assert!(
        bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "foreign-inspect-added-dep"
                && residual.summary.contains("pybind11")),
        "missing inspect residual: {:?}",
        bundle.plan.residuals
    );
}

#[test]
fn bump_inspect_maps_spack_hdf5_onto_robot_hdf5() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gromacsish-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Gromacsish'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['gromacsish-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         moduleclass = 'chem'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("HDF5-1.14.6-foss-2025a.eb"),
        "easyblock = 'CMakeMake'\nname = 'HDF5'\nversion = '1.14.6'\n\
         homepage = 'https://example.invalid/'\ndescription = 'HDF5'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'data'\n",
    )
    .expect("HDF5 candidate");
    let foreign = temp.path().join("package.py");
    fs::write(
        &foreign,
        "from spack.package import *\n\n\
         class Gromacsish(CMakePackage):\n\
         \thomepage = 'https://example.invalid/'\n\
         \turl = 'https://example.invalid/gromacsish-1.0.tar.gz'\n\
         \tversion('1.0', sha256='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')\n\
         \tdepends_on('hdf5')\n\
         \tdepends_on('boost')\n",
    )
    .expect("spack recipe");
    fs::write(
        robot.join("Boost-1.85.0-GCC-14.2.0.eb"),
        "easyblock = 'EB_Boost'\nname = 'Boost'\nversion = '1.85.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Boost'\n\
         toolchain = {'name': 'GCC', 'version': '14.2.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'devel'\n",
    )
    .expect("Boost candidate");
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
        foreign_sources: vec![foreign],
    })
    .expect("bump with hdf5 inspect");
    let hdf5 = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "HDF5")
        .expect("HDF5 locked");
    assert_eq!(hdf5.version, "1.14.6");
    assert!(
        !bundle.locks[0]
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "hdf5"),
        "lowercase hdf5 must not be the lock identity: {:?}",
        bundle.locks[0].dependencies
    );
    assert!(
        bundle.easyconfigs[0].text.contains("('HDF5'"),
        "inspect of Spack hdf5 must emit HDF5:\n{}",
        bundle.easyconfigs[0].text
    );
    assert!(
        !bundle
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category.contains("unresolved")
                && residual.summary.to_ascii_lowercase().contains("hdf5")),
        "hdf5 must not become an unresolved hole: {:?}",
        bundle.plan.residuals
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
        foreign_sources: Vec::new(),
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
        foreign_sources: Vec::new(),
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

/// A name in both lists keeps two stated versions. A last-wins map would see
/// the build version, decide the lock already matches, and leave the runtime
/// line on the old pin.
#[test]
fn dual_role_stated_versions_still_rewrite_the_runtime_line() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-GCCcore-13.3.0.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic package'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('zlib', '1.2-GCCcore-14.3.0')]\n\
         builddependencies = [('zlib', '1.3-GCCcore-14.3.0')]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("zlib-1.3-GCCcore-14.3.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'zlib'\nversion = '1.3'\n\
         homepage = 'https://example.invalid/'\ndescription = 'zlib'\n\
         toolchain = {'name': 'GCCcore', 'version': '14.3.0'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("zlib candidate");
    let toolchain = Toolchain {
        name: "GCCcore".into(),
        version: "14.3.0".into(),
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
        foreign_sources: Vec::new(),
    })
    .expect("dual-role bump");
    let lock_zlib: Vec<_> = bundle.locks[0]
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "zlib")
        .map(|dependency| {
            format!(
                "{} {} {} build={}",
                dependency.version,
                dependency.toolchain.name,
                dependency.toolchain.version,
                dependency.build
            )
        })
        .collect();
    let recipe = resolve_easyconfig_str(&bundle.easyconfigs[0].text).expect("parse bumped recipe");
    assert!(
        recipe
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "zlib" && dependency.version == "1.3"),
        "runtime zlib must follow the lock, not the last-wins build pin:\nlock zlib: {lock_zlib:?}\n{}",
        bundle.easyconfigs[0].text
    );
}

#[test]
fn bump_emits_the_plan_default_profile_even_when_it_is_not_named_default() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Alpha-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Alpha'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['alpha-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let config_path = temp.path().join("gpu.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n\
         [[profiles]]\nname = \"default\"\ndefault = false\n\n\
         [[profiles]]\nname = \"gpu\"\ninherits = \"default\"\ndefault = true\n",
    )
    .expect("layer");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
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
        foreign_sources: Vec::new(),
    })
    .expect("gpu default bump");
    assert_eq!(
        bundle.easyconfigs[0].profile, "gpu",
        "emitted recipe must be the plan default profile"
    );
    assert_eq!(bundle.locks[0].profile, "gpu");
}

#[test]
fn seissol_class_eigen_stays_on_3_4_unless_the_package_asks_for_5() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("SeisSol-1.1.4-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'SeisSol'\nversion = '1.1.4'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['seissol-1.1.4.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('Eigen', '3.4.0')]\n\
         moduleclass = 'geo'\n",
    )
    .expect("source recipe");
    fs::write(
        robot.join("Eigen-3.4.0-GCCcore-13.3.0.eb"),
        "name = 'Eigen'\nversion = '3.4.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Eigen 3.4'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
         moduleclass = 'lib'\n",
    )
    .expect("Eigen 3.4");
    fs::write(
        robot.join("Eigen-5.0.0-GCCcore-13.3.0.eb"),
        "name = 'Eigen'\nversion = '5.0.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Eigen 5'\n\
         toolchain = {'name': 'GCCcore', 'version': '13.3.0'}\n\
         moduleclass = 'lib'\n",
    )
    .expect("Eigen 5");
    let config_path = temp.path().join("seissol.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[[dependencies.requirements]]\n\
         name = \"Eigen\"\nconstraint = \">=3.4,<5\"\n",
    )
    .expect("layer");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
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
        foreign_sources: Vec::new(),
    })
    .expect("SeisSol Eigen bound");
    let eigen = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Eigen")
        .expect("Eigen locked");
    assert!(
        eigen.version.starts_with("3.4"),
        "package-config >=3.4,<5 must keep Eigen on 3.4, got {}",
        eigen.version
    );
}

#[test]
fn foreign_inspect_merge_applies_exclude_from_solve() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("App-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'App'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['app-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let foreign = temp.path().join("package.py");
    fs::write(
        &foreign,
        "from spack.package import *\n\n\
         class App(CMakePackage):\n\
         \thomepage = 'https://example.invalid/'\n\
         \turl = 'https://example.invalid/app-1.0.tar.gz'\n\
         \tversion('1.0', sha256='aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa')\n\
         \tdepends_on('py-pybind11')\n",
    )
    .expect("spack recipe");
    let config_path = temp.path().join("app.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[dependencies]\nexclude_from_solve = [\"pybind11\"]\n",
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
        foreign_sources: vec![foreign],
    })
    .expect("bump with excluded foreign inspect dep");
    let pybind11 = bundle
        .plan
        .dependencies
        .iter()
        .find(|dependency| {
            dependency.name.eq_ignore_ascii_case("pybind11")
                || dependency
                    .eb_name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case("pybind11"))
        })
        .expect("merged pybind11 intent");
    assert!(
        pybind11.solver_excluded,
        "exclude_from_solve must apply after foreign inspect merge: {:?}",
        bundle.plan.dependencies
    );
    assert!(
        !bundle.easyconfigs[0].text.contains("pybind11"),
        "excluded inspect dep still emitted:\n{}",
        bundle.easyconfigs[0].text
    );
}

#[test]
fn layer_version_and_request_version_agree_with_emit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("App-1.1.4-foss-2023a.eb");
    let robot = temp.path().join("robot");
    fs::create_dir_all(&robot).expect("robot directory");
    fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'App'\nversion = '1.1.4'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['app-1.1.4.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let config_path = temp.path().join("app.toml");
    fs::write(
        &config_path,
        "schema_version = 1\n\n[package]\nversion = \"1.3.2\"\n",
    )
    .expect("package config");
    let toolchain = Toolchain {
        name: "foss".into(),
        version: "2025a".into(),
    };
    let layer_only = plan_package_bump(&BumpPackageRequest {
        source: source.clone(),
        toolchain: toolchain.clone(),
        version: None,
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
        package_layers: vec![PackageConfigLayer::from_path(&config_path).expect("load layer")],
        foreign_sources: Vec::new(),
    })
    .expect("layer version bump");
    assert_eq!(layer_only.plan.package.version, "1.3.2");
    assert!(
        layer_only.easyconfigs[0].text.contains("version = '1.3.2'"),
        "layer version must reach emit:\n{}",
        layer_only.easyconfigs[0].text
    );

    let cli_wins = plan_package_bump(&BumpPackageRequest {
        source,
        toolchain: toolchain.clone(),
        version: Some("1.4.0".into()),
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
        foreign_sources: Vec::new(),
    })
    .expect("CLI version wins");
    assert_eq!(cli_wins.plan.package.version, "1.4.0");
    assert!(
        cli_wins.easyconfigs[0].text.contains("version = '1.4.0'"),
        "CLI version must win over the layer:\n{}",
        cli_wins.easyconfigs[0].text
    );
}
