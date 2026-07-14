use eb_stack::package::{
    materialize_profile, ConditionExpr, LockedDependency, OutputRequest, ProductProfile,
    ProfileEnvironment, ProfileLock, StackPin, StackPinMode, StackPolicy,
    PROFILE_LOCK_SCHEMA_VERSION, STACK_POLICY_SCHEMA_VERSION,
};
use eb_stack::{
    emit_profile_easyconfigs, package_plan_from_foreign, parse_foreign_path,
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
    assert_eq!(lock.package, "QMCPACK");
    assert_eq!(lock.profile, "default");
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0].name, "HDF5");
    assert_eq!(lock.dependencies[0].version, "1.14.2");
    assert!(!lock.dependencies[0].build);
    assert!(!lock.pin_outcomes[0].fallback);
}

#[test]
fn stack_pin_admits_a_cross_generation_runtime_closure() {
    let recipe = parse_foreign_path(&fixture(), Some(ForeignFormat::Spack)).expect("parse");
    let mut plan = package_plan_from_foreign(&recipe, &toolchain());
    plan.profiles = qmcpack_profiles();
    let mut dependency = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.eb_name.as_deref() == Some("HDF5"))
        .expect("dependency template")
        .clone();
    dependency.id = "pytorch".into();
    dependency.name = "pytorch".into();
    dependency.eb_name = Some("PyTorch".into());
    dependency.constraint = Some("2.9.1".into());
    dependency.condition = ConditionExpr::Always;
    plan.dependencies = vec![dependency];

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
    assert_eq!(lock.dependencies.len(), 1);
    assert_eq!(lock.dependencies[0].name, "PyTorch");
    assert_eq!(lock.dependencies[0].version, "2.9.1");
    assert_eq!(lock.dependencies[0].toolchain, foss_2024a);
    assert!(lock.dependencies[0].versionsuffix.is_none());
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
        .find(|dependency| dependency.eb_name.as_deref() == Some("HDF5"))
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
