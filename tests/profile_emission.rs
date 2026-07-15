use eb_stack::package::{
    materialize_profile, ConditionExpr, LockedDependency, OutputRequest, ProductProfile,
    ProfileEnvironment, ProfileLock, StackPin, StackPinMode, StackPolicy,
    PROFILE_LOCK_SCHEMA_VERSION, STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::{
    emit_profile_easyconfigs, lint_style, package_plan_from_foreign, parse_foreign_path,
    resolve_easyconfig_str, solve_package_profile, Candidate, DepReq, ForeignFormat, Toolchain,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/foreign_ingest/spack_qmcpack/package.py")
}

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

fn qmcpack_profiles() -> Vec<ProductProfile> {
    let common_features = BTreeMap::from([
        ("mpi".into(), true),
        ("phdf5".into(), false),
        ("complex".into(), false),
        ("mixed".into(), false),
    ]);
    let common_toolchain = BTreeMap::from([("usempi".into(), true), ("openmp".into(), true)]);
    vec![
        ProductProfile {
            name: "default".into(),
            default: true,
            versionsuffix: Vec::new(),
            features: common_features.clone(),
            parameters: BTreeMap::from([("build_type".into(), "Release".into())]),
            toolchain_options: common_toolchain.clone(),
            config_options: vec![
                "-DQMC_MPI=ON".into(),
                "-DQMC_OMP=ON".into(),
                "-DQMC_COMPLEX=OFF".into(),
                "-DQMC_MIXED_PRECISION=OFF".into(),
            ],
            verification_commands: Vec::new(),
        },
        ProductProfile {
            name: "complex".into(),
            default: false,
            versionsuffix: vec!["-complex".into()],
            features: common_features
                .into_iter()
                .map(|(name, enabled)| {
                    let enabled = if name == "complex" { true } else { enabled };
                    (name, enabled)
                })
                .collect(),
            parameters: BTreeMap::from([("build_type".into(), "Release".into())]),
            toolchain_options: common_toolchain,
            config_options: vec![
                "-DQMC_MPI=ON".into(),
                "-DQMC_OMP=ON".into(),
                "-DQMC_COMPLEX=ON".into(),
                "-DQMC_MIXED_PRECISION=OFF".into(),
            ],
            verification_commands: Vec::new(),
        },
    ]
}

fn locked_dependency(name: &str, version: &str, build: bool) -> LockedDependency {
    LockedDependency {
        name: name.into(),
        version: version.into(),
        versionsuffix: None,
        toolchain: toolchain(),
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        build,
    }
}

fn profile_lock(profile: &str, versionsuffix: &str) -> ProfileLock {
    ProfileLock {
        schema_version: PROFILE_LOCK_SCHEMA_VERSION,
        package: "QMCPACK".into(),
        version: "4.3.0".into(),
        profile: profile.into(),
        toolchain: toolchain(),
        versionsuffix: versionsuffix.into(),
        dependencies: vec![
            locked_dependency("CMake", "4.2.1", true),
            locked_dependency("HDF5", "2.1.1", false),
            locked_dependency("Boost", "1.90.0", false),
            locked_dependency("libxml2", "2.15.1", false),
            locked_dependency("Python", "3.14.2", false),
        ],
        pin_outcomes: Vec::new(),
        exclusions: Vec::new(),
        solver: "resolvo".into(),
    }
}

fn hdf5_candidate(version: &str) -> Candidate {
    Candidate {
        name: "HDF5".into(),
        version: version.into(),
        toolchain: toolchain(),
        versionsuffix: None,
        easyconfig_path: format!("HDF5-{version}-foss-2026.1.eb"),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    }
}

#[test]
fn materialization_filters_conditional_dependencies_per_profile() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();

    let default = materialize_profile(&plan, "default", &ProfileEnvironment::default())
        .expect("default profile");
    let hdf5: Vec<_> = default
        .dependencies
        .iter()
        .filter(|dependency| dependency.name == "hdf5")
        .collect();
    assert_eq!(hdf5.len(), 1, "only the ~phdf5 edge is active: {hdf5:?}");
    assert!(hdf5[0]
        .provenance
        .iter()
        .any(|provenance| provenance.original.contains("hdf5~mpi")));

    let complex = materialize_profile(&plan, "complex", &ProfileEnvironment::default())
        .expect("complex profile");
    assert_eq!(complex.profile.features.get("complex"), Some(&true));
    assert_eq!(complex.versionsuffix, "-complex");
}

#[test]
fn each_product_profile_emits_a_conventional_easyconfig() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.package.name = "QMCPACK".into();
    plan.build.config_options = vec!["-DCMAKE_BUILD_TYPE=Release".into()];
    plan.build.moduleclass = Some("chem".into());
    plan.profiles = qmcpack_profiles();
    plan.outputs = vec![
        OutputRequest {
            profile: "default".into(),
            stack: "foss-2026.1".into(),
        },
        OutputRequest {
            profile: "complex".into(),
            stack: "foss-2026.1".into(),
        },
    ];

    let emitted = emit_profile_easyconfigs(
        &plan,
        &[
            profile_lock("default", ""),
            profile_lock("complex", "-complex"),
        ],
    )
    .expect("emit profile recipe set");
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[0].profile, "default");
    assert_eq!(emitted[0].filename, "QMCPACK-4.3.0-foss-2026.1.eb");
    assert_eq!(emitted[1].profile, "complex");
    assert_eq!(emitted[1].filename, "QMCPACK-4.3.0-foss-2026.1-complex.eb");

    let default = resolve_easyconfig_str(&emitted[0].text).expect("parse default easyconfig");
    assert!(default.versionsuffix.is_none());
    assert_eq!(default.moduleclass.as_deref(), Some("chem"));
    assert!(default
        .configopts
        .as_deref()
        .is_some_and(|options| options.contains("-DQMC_COMPLEX=OFF")));
    assert!(emitted[0]
        .text
        .contains("toolchainopts = {'openmp': True, 'usempi': True}"));

    let complex = resolve_easyconfig_str(&emitted[1].text).expect("parse complex easyconfig");
    assert_eq!(complex.versionsuffix.as_deref(), Some("-complex"));
    assert!(complex
        .configopts
        .as_deref()
        .is_some_and(|options| options.contains("-DQMC_COMPLEX=ON")));
}

#[test]
fn automatic_easyblock_is_omitted_from_the_easyconfig() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.package.name = "QMCPACK".into();
    plan.build.easyblock = None;
    plan.profiles = qmcpack_profiles();
    plan.outputs.truncate(1);

    let emitted = emit_profile_easyconfigs(&plan, &[profile_lock("default", "")])
        .expect("emit automatic easyblock recipe");
    assert!(!emitted[0].text.contains("easyblock ="));
}

#[test]
fn empty_sanity_paths_are_omitted_from_the_easyconfig() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.package.name = "QMCPACK".into();
    plan.profiles = qmcpack_profiles();
    plan.outputs.truncate(1);

    let emitted = emit_profile_easyconfigs(&plan, &[profile_lock("default", "")])
        .expect("emit conventional recipe");

    assert!(!emitted[0].text.contains("sanity_check_paths"));
}

#[test]
fn conditional_spack_resources_follow_the_selected_profile() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/foreign_ingest/spack_lammps/package.py");
    let recipe = parse_foreign_path(&source, Some(ForeignFormat::Spack)).expect("parse LAMMPS");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.outputs.truncate(1);
    let lock = ProfileLock {
        schema_version: PROFILE_LOCK_SCHEMA_VERSION,
        package: plan.package.name.clone(),
        version: plan.package.version.clone(),
        profile: "default".into(),
        toolchain: toolchain(),
        versionsuffix: String::new(),
        dependencies: Vec::new(),
        pin_outcomes: Vec::new(),
        exclusions: Vec::new(),
        solver: "resolvo".into(),
    };

    let without_mesont = emit_profile_easyconfigs(&plan, std::slice::from_ref(&lock))
        .expect("emit profile without MESONT");
    assert!(!without_mesont[0].text.contains("C_10_10.mesocnt"));

    plan.profiles[0].features.insert("mesont".into(), true);
    let with_mesont = emit_profile_easyconfigs(&plan, &[lock]).expect("emit profile with MESONT");
    assert!(with_mesont[0].text.contains("C_10_10.mesocnt"));
}

#[test]
fn emitted_dependencies_preserve_locked_toolchain_and_versionsuffix_identity() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.package.name = "QMCPACK".into();
    plan.profiles = qmcpack_profiles();
    plan.outputs = vec![OutputRequest {
        profile: "default".into(),
        stack: "foss-2026.1".into(),
    }];

    let mut lock = profile_lock("default", "");
    lock.dependencies = vec![
        LockedDependency {
            name: "PyTorch".into(),
            version: "2.9.1".into(),
            versionsuffix: None,
            toolchain: Toolchain {
                name: "foss".into(),
                version: "2024a".into(),
            },
            easyconfig_path: "PyTorch-2.9.1-foss-2024a.eb".into(),
            build: false,
        },
        LockedDependency {
            name: "PETSc".into(),
            version: "3.24.0".into(),
            versionsuffix: Some("-complex".into()),
            toolchain: toolchain(),
            easyconfig_path: "PETSc-3.24.0-foss-2026.1-complex.eb".into(),
            build: false,
        },
    ];

    let emitted = emit_profile_easyconfigs(&plan, &[lock]).expect("emit locked identities");
    assert!(emitted[0]
        .text
        .contains("('PyTorch', '2.9.1', '', ('foss', '2024a'))"));
    assert!(emitted[0].text.contains("('PETSc', '3.24.0', '-complex')"));

    let resolved = resolve_easyconfig_str(&emitted[0].text).expect("parse emitted easyconfig");
    let pytorch = resolved
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "PyTorch")
        .expect("PyTorch dependency");
    assert_eq!(
        pytorch.toolchain,
        Some(Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        })
    );
}

#[test]
fn conda_target_directories_emit_rattler_compatible_source_staging() {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/foreign_ingest/conda_eon/recipe.yaml");
    let recipe = parse_foreign_path(&source, Some(ForeignFormat::CondaForge)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.package.name = "eOn".into();
    plan.build.config_options = vec![
        "-Dbuildtype=release".into(),
        "-Dwith_tests=false".into(),
        "-Dwith_fortran=true".into(),
        "-Dwith_metatomic=true".into(),
        "-Dwith_xtb=true".into(),
        "-Dwith_serve=true".into(),
        "-Dwith_rgpot=true".into(),
        "-Dwith_mpi=false".into(),
        "-Dnative_arch=false".into(),
    ];
    plan.profiles = vec![ProductProfile {
        name: "default".into(),
        default: true,
        versionsuffix: Vec::new(),
        features: BTreeMap::new(),
        parameters: BTreeMap::new(),
        toolchain_options: BTreeMap::new(),
        config_options: Vec::new(),
        verification_commands: Vec::new(),
    }];
    plan.outputs = vec![OutputRequest {
        profile: "default".into(),
        stack: "foss-2026.1".into(),
    }];
    let lock = ProfileLock {
        schema_version: PROFILE_LOCK_SCHEMA_VERSION,
        package: "eOn".into(),
        version: "2.16.0".into(),
        profile: "default".into(),
        toolchain: toolchain(),
        versionsuffix: String::new(),
        dependencies: Vec::new(),
        pin_outcomes: Vec::new(),
        exclusions: Vec::new(),
        solver: "resolvo".into(),
    };

    let emitted = emit_profile_easyconfigs(&plan, &[lock]).expect("emit");
    let text = &emitted[0].text;
    assert!(text
        .contains("'source_urls': ['https://github.com/OmniPotentRPC/rgpot/archive/refs/tags/']"));
    assert!(text.contains("'filename': 'v2.2.1.tar.gz'"));
    let normalized = text.replace("' +\n            '", "");
    assert!(normalized.contains(
        "'extract_cmd': 'mkdir -p %(builddir)s/subprojects/rgpot && tar -xf %s -C \
         %(builddir)s/subprojects/rgpot --strip-components=1'"
    ));
    assert!(normalized.contains("%(builddir)s/readcon-core-src --strip-components=1"));
    assert!(lint_style(text).is_empty(), "{:?}", lint_style(text));
}

#[test]
fn profile_lock_is_created_by_resolvo_with_stack_preferences() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();
    plan.dependencies
        .retain(|dependency| dependency.name == "hdf5");
    let stack = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "eessi".into(),
        toolchain: toolchain(),
        pins: vec![StackPin {
            name: "HDF5".into(),
            version_requirement: "==1.14.2".into(),
            toolchain: None,
            versionsuffix: None,
            mode: StackPinMode::Preferred,
            source: Some("eessi.cdx.json".into()),
        }],
        exclusions: Vec::new(),
    };

    let lock = solve_package_profile(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &[hdf5_candidate("1.14.2"), hdf5_candidate("1.14.3")],
        &stack,
    )
    .expect("profile solve");
    assert_eq!(lock.solver, "resolvo");
    assert_eq!(lock.package, "qmcpack");
    assert_eq!(lock.profile, "default");
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0].name, "HDF5");
    assert_eq!(lock.dependencies[0].version, "1.14.2");
    assert!(!lock.dependencies[0].build);
    assert!(!lock.pin_outcomes[0].fallback);
}

#[test]
fn profile_solver_matches_unmapped_foreign_names_to_robot_candidates() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();
    let mut dependency = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "hdf5")
        .expect("dependency template")
        .clone();
    dependency.id = "some-lib".into();
    dependency.name = "some-lib".into();
    dependency.eb_name = None;
    dependency.constraint = Some(">=2.0".into());
    dependency.condition = ConditionExpr::Always;
    plan.dependencies = vec![dependency];
    let candidate = Candidate {
        name: "SomeLib".into(),
        version: "2.1".into(),
        toolchain: toolchain(),
        versionsuffix: None,
        easyconfig_path: "SomeLib-2.1-foss-2026.1.eb".into(),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    };
    let stack = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "test".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    };

    let lock = solve_package_profile(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &[candidate],
        &stack,
    )
    .expect("match normalized foreign identity");
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0].name, "SomeLib");
}

#[test]
fn foreign_build_roles_create_easybuild_builddependencies() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();
    plan.dependencies
        .retain(|dependency| matches!(dependency.name.as_str(), "boost" | "cmake"));
    let candidate = |name: &str, version: &str| Candidate {
        name: name.into(),
        version: version.into(),
        toolchain: toolchain(),
        versionsuffix: None,
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        dependencies: Vec::new(),
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    };
    let stack = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "test".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    };

    let lock = solve_package_profile(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &[candidate("Boost", "1.90.0"), candidate("CMake", "4.2.1")],
        &stack,
    )
    .expect("classify foreign dependencies");
    assert!(lock
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "CMake")
        .is_some_and(|dependency| dependency.build));
    assert!(lock
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Boost")
        .is_some_and(|dependency| dependency.build));
}

#[test]
fn stack_pin_admits_a_cross_generation_runtime_closure() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();
    let mut dependency = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "hdf5")
        .expect("dependency template")
        .clone();
    dependency.id = "pytorch".into();
    dependency.name = "pytorch".into();
    dependency.eb_name = Some("PyTorch".into());
    dependency.constraint = Some("2.9.1".into());
    dependency.condition = ConditionExpr::Always;
    let mut target_python = dependency.clone();
    target_python.id = "python".into();
    target_python.name = "python".into();
    target_python.eb_name = Some("Python".into());
    target_python.constraint = Some("3.14.2".into());
    plan.dependencies = vec![dependency, target_python];

    let foss_2024a = Toolchain {
        name: "foss".into(),
        version: "2024a".into(),
    };
    let gcccore_2024a = Toolchain {
        name: "GCCcore".into(),
        version: "13.3.0".into(),
    };
    let candidate = |name: &str,
                     version: &str,
                     candidate_toolchain: Toolchain,
                     dependencies: Vec<DepReq>| Candidate {
        name: name.into(),
        version: version.into(),
        toolchain: candidate_toolchain.clone(),
        versionsuffix: None,
        easyconfig_path: format!(
            "{name}-{version}-{}-{}.eb",
            candidate_toolchain.name, candidate_toolchain.version
        ),
        dependencies,
        builddependencies: Vec::new(),
        exts_list: Vec::new(),
    };
    let python_312 = DepReq {
        name: "Python".into(),
        version_req: "==3.12.3".into(),
        versionsuffix: None,
        toolchain: None,
    };
    let mut pytorch_cuda = candidate("PyTorch", "2.9.1", foss_2024a.clone(), Vec::new());
    pytorch_cuda.versionsuffix = Some("-CUDA-12.6.0".into());
    let candidates = vec![
        candidate("PyTorch", "2.8.0", toolchain(), Vec::new()),
        candidate("PyTorch", "2.9.1", foss_2024a.clone(), vec![python_312]),
        pytorch_cuda,
        candidate(
            "PyTorch",
            "2.9.1",
            Toolchain {
                name: "foss".into(),
                version: "2025b".into(),
            },
            Vec::new(),
        ),
        candidate("Python", "3.12.3", gcccore_2024a, Vec::new()),
        candidate("Python", "3.14.2", toolchain(), Vec::new()),
    ];
    let stack = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "site".into(),
        toolchain: toolchain(),
        pins: vec![StackPin {
            name: "PyTorch".into(),
            version_requirement: "==2.9.1".into(),
            toolchain: Some(foss_2024a.clone()),
            versionsuffix: Some(String::new()),
            mode: StackPinMode::Preferred,
            source: Some("site stack".into()),
        }],
        exclusions: Vec::new(),
    };

    let lock = solve_package_profile(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &candidates,
        &stack,
    )
    .expect("cross-generation stack pin closure");
    assert_eq!(lock.dependencies.len(), 2);
    let pytorch = lock
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "PyTorch")
        .expect("PyTorch lock");
    assert_eq!(pytorch.version, "2.9.1");
    assert_eq!(pytorch.toolchain, foss_2024a);
    assert!(pytorch.versionsuffix.is_none());
    let python = lock
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Python")
        .expect("target Python lock");
    assert_eq!(python.version, "3.14.2");
    assert!(!lock.pin_outcomes[0].fallback);
}

#[test]
fn profile_solve_scopes_build_dependencies_of_existing_recipes() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();
    let template = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "hdf5")
        .expect("HDF5 dependency")
        .clone();
    plan.dependencies = [("openssl", "OpenSSL", "3"), ("git", "git", "2.45.1")]
        .into_iter()
        .map(|(id, name, version)| {
            let mut dependency = template.clone();
            dependency.id = id.into();
            dependency.name = name.into();
            dependency.eb_name = Some(name.into());
            dependency.constraint = Some(version.into());
            dependency.condition = ConditionExpr::Always;
            dependency
        })
        .collect();

    let candidate = |name: &str,
                     version: &str,
                     dependencies: Vec<DepReq>,
                     builddependencies: Vec<DepReq>| Candidate {
        name: name.into(),
        version: version.into(),
        toolchain: toolchain(),
        versionsuffix: None,
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        dependencies,
        builddependencies,
        exts_list: Vec::new(),
    };
    let requirement = |name: &str, version: &str| DepReq {
        name: name.into(),
        version_req: format!("=={version}"),
        versionsuffix: None,
        toolchain: None,
    };
    let candidates = vec![
        candidate(
            "OpenSSL",
            "3",
            Vec::new(),
            vec![requirement("Perl", "5.38.0")],
        ),
        candidate(
            "git",
            "2.45.1",
            vec![requirement("Perl", "5.38.2")],
            Vec::new(),
        ),
        candidate("Perl", "5.38.0", Vec::new(), Vec::new()),
        candidate("Perl", "5.38.2", Vec::new(), Vec::new()),
    ];
    let stack = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "test".into(),
        toolchain: toolchain(),
        pins: Vec::new(),
        exclusions: Vec::new(),
    };

    let lock = solve_package_profile(
        &plan,
        "default",
        &ProfileEnvironment::default(),
        &candidates,
        &stack,
    )
    .expect("existing recipe build dependencies have independent build contexts");
    assert_eq!(lock.dependencies.len(), 2);
    assert!(lock
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "OpenSSL"));
    assert!(lock
        .dependencies
        .iter()
        .any(|dependency| dependency.name == "git"));
}
