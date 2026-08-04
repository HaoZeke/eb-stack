//! Ring 0 reproduction: does the ingest path plus a declared maintainer edit
//! reproduce the merged easyconfig for a brand-new package?
//!
//! The ring-1 benchmark asks whether `bump` reproduces a version bump. Ring 0
//! cannot use it: its packages are first-ever additions, so there is no prior
//! recipe to bump from and `emit_next_generation` has nothing to operate on. The
//! applicable path is ingest, foreign recipe to EasyBuild scaffold, and until
//! now nothing asserted that it reproduces anything.
//!
//! What counts as a fair reproduction here matters more than the score. The
//! edits below are the ones a maintainer genuinely supplies and a foreign recipe
//! cannot know: the target version and its checksum, the EasyBuild package name,
//! the product profile, and the solved dependency versions (the solver's job,
//! benchmarked separately as `resolves`). Everything else has to come out of the
//! ingested definition. Widening that edit set until the score improves would
//! measure nothing, so each field is listed explicitly and the score is whatever
//! it is.

use eb_stack::miner::{compare_reproduction, score_reproduction, ReproScore};
use eb_stack::package::{
    LockedDependency, OutputRequest, ProductProfile, ProfileLock, PROFILE_LOCK_SCHEMA_VERSION,
};
use eb_stack::{
    emit_profile_easyconfigs, package_plan_from_foreign, parse_foreign_path,
    resolve_easyconfig_str, ForeignFormat, Toolchain,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn foss_2026_1() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

/// A dependency as the solver would hand it over: name, version, and whether it
/// is build-only. Versions are the merged recipe's, because reproducing the
/// *file* is the question here and choosing versions is the solver benchmark.
fn dep(name: &str, version: &str, build: bool) -> LockedDependency {
    LockedDependency {
        name: name.into(),
        version: version.into(),
        versionsuffix: None,
        toolchain: foss_2026_1(),
        easyconfig_path: format!("{name}-{version}-foss-2026.1.eb"),
        build,
    }
}

fn eon_lock() -> ProfileLock {
    ProfileLock {
        schema_version: PROFILE_LOCK_SCHEMA_VERSION,
        package: "eOn".into(),
        version: "2.17.10".into(),
        profile: "default".into(),
        toolchain: foss_2026_1(),
        versionsuffix: String::new(),
        dependencies: vec![
            dep("CMake", "4.2.1", true),
            dep("Meson", "1.10.2", true),
            dep("Ninja", "1.13.2", true),
            dep("pkgconf", "2.5.1", true),
            dep("Python", "3.14.2", false),
            dep("SciPy-bundle", "2026.05", false),
            dep("PyYAML", "6.0.3", false),
            dep("Eigen", "5.0.0", false),
            dep("Highway", "1.4.0", false),
            dep("inih", "62", false),
            dep("nlohmann_json", "3.12.0", false),
            dep("quill", "11.1.0", false),
            dep("readcon-core", "0.13.1", false),
            dep("rgpot", "2.5.3", false),
        ],
        pin_outcomes: Vec::new(),
        exclusions: Vec::new(),
        solver: "resolvo".into(),
    }
}

/// The single product eOn ships, with the toolchain options and build switches
/// the maintainer chose.
fn eon_profile() -> ProductProfile {
    ProductProfile {
        name: "default".into(),
        default: true,
        versionsuffix: Vec::new(),
        platform: None,
        architecture: None,
        features: BTreeMap::from([
            ("tests".into(), true),
            ("rgpot".into(), true),
            ("fortran".into(), false),
            ("cuh2".into(), false),
        ]),
        parameters: BTreeMap::new(),
        toolchain_options: BTreeMap::from([("openmp".into(), true), ("pic".into(), true)]),
        config_options: vec![
            "-Dbuildtype=release".into(),
            "-Dwith_tests=true".into(),
            "-Dwith_rgpot=true".into(),
        ],
        easyconfig_parameters: BTreeMap::new(),
        verification_commands: Vec::new(),
    }
}

/// Ingest the Spack definition, apply the declared edits, and render the recipe.
fn emit_eon_from_spack() -> String {
    let path = fixtures().join("foreign_ingest/spack_eon/package.py");
    let recipe = parse_foreign_path(&path, Some(ForeignFormat::Spack)).expect("parse spack eOn");
    let mut plan = package_plan_from_foreign(&recipe, &foss_2026_1());

    // Maintainer-supplied, and unknowable from a Spack recipe pinned at 2.16.0:
    // the EasyBuild package name, the target version, and its checksum.
    plan.package.name = "eOn".into();
    plan.package.version = "2.17.10".into();
    if let Some(source) = plan.sources.first_mut() {
        source.sha256 =
            Some("1ade06d7a30afcd08f9f14194d7051478c48e7b8d11baaab8da830073e3d6f4a".into());
    }
    plan.profiles = vec![eon_profile()];
    plan.outputs = vec![OutputRequest {
        profile: "default".into(),
        stack: "foss-2026.1".into(),
    }];

    let emitted = emit_profile_easyconfigs(&plan, &[eon_lock()]).expect("emit eOn recipe");
    assert_eq!(emitted.len(), 1, "one product profile, one file");
    emitted[0].text.clone()
}

fn merged_eon() -> String {
    let path = fixtures().join("eon_foss_2026_1/easyconfigs/e/eOn/eOn-2.17.10-foss-2026.1.eb");
    std::fs::read_to_string(&path).expect("read merged eOn recipe")
}

#[test]
fn ingest_emits_a_parseable_recipe_for_a_brand_new_package() {
    // The floor: whatever the score turns out to be, ring 0 must at least
    // produce a file EasyBuild can read. An unparseable scaffold is the ring-0
    // equivalent of the ERROR bucket.
    let emitted = emit_eon_from_spack();
    let parsed = resolve_easyconfig_str(&emitted).expect("emitted recipe must parse");
    assert_eq!(parsed.name, "eOn");
    assert_eq!(parsed.version, "2.17.10");
}

#[test]
fn ring0_eon_scores_against_the_merged_recipe() {
    let emitted = emit_eon_from_spack();
    let target = merged_eon();
    let score = score_reproduction(&emitted, &target);

    // Ring 0's stated bar is SEMANTIC-or-better. Asserting it is how the bar
    // gets enforced rather than described; when it fails, the diff below is the
    // finding, and the number to record is in the message.
    assert!(
        matches!(score, ReproScore::Exact | ReproScore::Semantic),
        "ring 0 eOn scored {} against the merged recipe, below the SEMANTIC bar.\n\
         normalized diff:\n{}",
        score.as_str(),
        compare_reproduction(&emitted, &target).render_raw_diff()
    );
}
