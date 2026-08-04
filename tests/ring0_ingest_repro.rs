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

/// Top-level easyconfig parameter names assigned in a recipe, in file order.
fn parameter_names(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.starts_with([' ', '\t', '#', ')', ']', '}']))
        .filter_map(|line| line.split_once('='))
        .map(|(key, _)| key.trim().to_string())
        .filter(|key| !key.is_empty() && !key.contains(char::is_whitespace))
        .collect()
}

/// Parameters the merged recipe sets that the emitted one does not.
fn maintainer_remainder(emitted: &str, target: &str) -> Vec<String> {
    let have = parameter_names(emitted);
    parameter_names(target)
        .into_iter()
        .filter(|key| !have.contains(key))
        .collect()
}

#[test]
fn ring0_eon_reproduces_what_a_foreign_recipe_can_express() {
    // The part ingest is answerable for. A Spack definition carries identity,
    // sources, checksums, dependency edges and build switches, so those must
    // come out right without hand-editing the emitted text.
    let emitted = emit_eon_from_spack();
    let parsed = resolve_easyconfig_str(&emitted).expect("emitted recipe must parse");
    let merged = resolve_easyconfig_str(&merged_eon()).expect("merged recipe must parse");

    assert_eq!(parsed.name, merged.name);
    assert_eq!(parsed.version, merged.version);
    assert_eq!(parsed.toolchain, merged.toolchain);
    assert_eq!(parsed.versionsuffix, merged.versionsuffix);

    let names = |deps: &[eb_stack::ResolvedDep]| -> Vec<String> {
        let mut out: Vec<String> = deps.iter().map(|dep| dep.name.clone()).collect();
        out.sort();
        out
    };
    assert_eq!(
        names(&parsed.dependencies),
        names(&merged.dependencies),
        "runtime dependency set"
    );
    assert_eq!(
        names(&parsed.builddependencies),
        names(&merged.builddependencies),
        "build dependency set"
    );
}

#[test]
fn ring0_eon_records_the_maintainer_remainder() {
    // The honest ring-0 number is not a single bucket: no foreign recipe encodes
    // EasyBuild's own vocabulary, so some parameters can only come from the
    // maintainer. Recording exactly which ones keeps the benchmark falsifiable
    // -- the list shrinks only when the generation path genuinely learns to
    // produce one of them, and this test fails when it does, on purpose.
    let emitted = emit_eon_from_spack();
    let target = merged_eon();

    let remainder = maintainer_remainder(&emitted, &target);
    let expected = [
        "github_account",
        "source_urls",
        "runtest",
        "testopts",
        "sanity_check_paths",
        "sanity_check_commands",
        "modextrapaths",
    ];
    let unexpected: Vec<&String> = remainder
        .iter()
        .filter(|key| !expected.contains(&key.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "ingest lost parameters beyond the recorded EasyBuild-only remainder: {unexpected:?}\n\
         full remainder: {remainder:?}\n\
         raw diff:\n{}",
        compare_reproduction(&emitted, &target).render_raw_diff()
    );

    // The whole-file bucket is Material for as long as the remainder is
    // non-empty, and saying so here keeps the scoreboard row and the test from
    // drifting apart. When the remainder empties, this assertion is what tells
    // you the bucket can be raised.
    let score = score_reproduction(&emitted, &target);
    if remainder.is_empty() {
        assert!(
            matches!(score, ReproScore::Exact | ReproScore::Semantic),
            "nothing is missing yet the score is {}: the scorer and the \
             parameter comparison disagree, which is itself the finding",
            score.as_str()
        );
    } else {
        assert_eq!(
            score,
            ReproScore::Material,
            "a non-empty maintainer remainder must score Material, not {}",
            score.as_str()
        );
    }
}
