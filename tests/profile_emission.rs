use eb_stack::package::{
    materialize_profile, LockedDependency, OutputRequest, ProductProfile, ProfileEnvironment,
    ProfileLock, PROFILE_LOCK_SCHEMA_VERSION,
};
use eb_stack::{
    emit_profile_easyconfigs, package_plan_from_foreign, parse_foreign_path,
    resolve_easyconfig_str, ForeignFormat, Toolchain,
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
