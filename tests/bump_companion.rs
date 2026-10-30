//! A version bump whose recipe derives a dependency version from its own
//! version: the dependency is pinned at the derived version, and when the tree
//! lacks it the dependency is bumped too, into the same overlay.

use eb_stack::bump_companion::{plan_package_bump_with_companions, write_companions};
use eb_stack::package::{StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_workflow::{version_derived_dependencies, PackageEmitOptions};
use eb_stack::{plan_package_bump, resolve_easyconfig_str, BumpPackageRequest, Toolchain};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const CHECKSUM: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn toolchain(name: &str, version: &str) -> Toolchain {
    Toolchain {
        name: name.into(),
        version: version.into(),
    }
}

fn request(source: PathBuf, robot: &Path, version: &str) -> BumpPackageRequest {
    let target = toolchain("foss", "2025a");
    BumpPackageRequest {
        source,
        toolchain: target.clone(),
        version: Some(version.into()),
        source_checksum: Some("b".repeat(64)),
        easyconfig_roots: vec![robot.to_path_buf()],
        hierarchy_fixture: None,
        overrides: HashMap::new(),
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "default".into(),
            toolchain: target,
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
        strict_patches: false,
        package_layers: Vec::new(),
    }
}

/// A Boost.Python-NumPy shape: the bindings follow the library's version,
/// and the library sits one level down at GCCcore.
fn write_tree(root: &Path) -> (PathBuf, PathBuf) {
    let robot = root.join("robot");
    fs::create_dir_all(robot.join("c/Core")).expect("robot");
    fs::create_dir_all(robot.join("w/Wrap")).expect("robot");
    fs::write(
        robot.join("c/Core/Core-1.90.0-GCCcore-14.2.0.eb"),
        format!(
            "easyblock = 'ConfigureMake'\nname = 'Core'\nversion = '1.90.0'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Library'\n\
             toolchain = {{'name': 'GCCcore', 'version': '14.2.0'}}\n\
             source_urls = ['https://example.invalid/%(version)s/']\n\
             sources = ['core-%(version)s.tar.gz']\nchecksums = ['{CHECKSUM}']\n\
             moduleclass = 'lib'\n"
        ),
    )
    .expect("Core recipe");
    fs::write(
        robot.join("w/Wrap/Wrap-1.90.0-foss-2025a.eb"),
        format!(
            "easyblock = 'ConfigureMake'\nname = 'Wrap'\nversion = '1.90.0'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Bindings'\n\
             toolchain = {{'name': 'foss', 'version': '2025a'}}\n\
             sources = ['wrap-%(version)s.tar.gz']\nchecksums = ['{CHECKSUM}']\n\
             dependencies = [\n    ('Core', version),\n]\n\
             moduleclass = 'lib'\n"
        ),
    )
    .expect("Wrap recipe");
    (robot.join("w/Wrap/Wrap-1.90.0-foss-2025a.eb"), robot)
}

#[test]
fn a_dependency_written_as_the_package_version_moves_with_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (source, robot) = write_tree(temp.path());
    let resolved = eb_stack::eb_parse::resolve_easyconfig_file(&source).expect("source");
    let derived = version_derived_dependencies(&request(source, &robot, "1.92.0"), &resolved);
    assert_eq!(derived, vec![("Core".to_string(), "1.92.0".to_string())]);
}

#[test]
fn a_sliced_version_expression_is_derived_at_the_new_version() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("protobuf-python-7.35.0-GCCcore-14.2.0.eb");
    fs::write(
        &source,
        format!(
            "easyblock = 'PythonBundle'\nname = 'protobuf-python'\nversion = '7.35.0'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Bindings'\n\
             toolchain = {{'name': 'GCCcore', 'version': '14.2.0'}}\n\
             builddependencies = [('binutils', '2.42')]\n\
             dependencies = [\n    ('Python', '3.13.1'),\n    ('protobuf', version[2:]),\n]\n\
             exts_list = [('protobuf', version, {{'checksums': ['{CHECKSUM}']}})]\n\
             moduleclass = 'devel'\n"
        ),
    )
    .expect("protobuf-python recipe");
    let resolved = eb_stack::eb_parse::resolve_easyconfig_file(&source).expect("source");
    let mut bump = request(source, temp.path(), "7.36.2");
    bump.toolchain = toolchain("GCCcore", "14.2.0");
    let derived = version_derived_dependencies(&bump, &resolved);
    assert_eq!(
        derived,
        vec![("protobuf".to_string(), "36.2".to_string())],
        "only the version-derived dependency moves; Python and binutils are literals"
    );
}

#[test]
fn without_companions_the_lock_never_names_the_old_derived_version() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (source, robot) = write_tree(temp.path());
    let bundle = plan_package_bump(&request(source, &robot, "1.92.0")).expect("bump");
    assert!(
        !bundle.locks[0]
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "Core" && dependency.version == "1.90.0"),
        "the recipe asks for Core 1.92.0, so the lock must not claim 1.90.0: {:?}",
        bundle.locks[0].dependencies
    );
    assert!(
        bundle.plan.residuals.iter().any(
            |residual| residual.summary.contains("Core") && residual.summary.contains("1.92.0")
        ),
        "the missing Core 1.92.0 is reported: {:?}",
        bundle.plan.residuals
    );
}

#[test]
fn a_missing_derived_dependency_is_bumped_into_the_same_overlay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (source, robot) = write_tree(temp.path());
    let planned = plan_package_bump_with_companions(
        &request(source, &robot, "1.92.0"),
        &PackageEmitOptions::default(),
        None,
    )
    .expect("bump with companions");

    assert_eq!(planned.companions.len(), 1, "one companion: Core");
    let companion = &planned.companions[0];
    assert_eq!(companion.required_by, "Wrap");
    assert_eq!(companion.bundle.plan.package.name, "Core");
    assert_eq!(companion.bundle.plan.package.version, "1.92.0");
    assert_eq!(
        companion.bundle.plan.build.toolchain,
        toolchain("GCCcore", "14.2.0"),
        "the companion stays at the level the tree has it"
    );
    let core = resolve_easyconfig_str(&companion.bundle.easyconfigs[0].text).expect("Core text");
    assert_eq!(core.version, "1.92.0");
    assert_eq!(
        companion.bundle.easyconfigs[0].filename,
        "Core-1.92.0-GCCcore-14.2.0.eb"
    );

    let locked = planned.root.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Core")
        .expect("the root still depends on Core");
    assert_eq!(locked.version, "1.92.0");
    assert_eq!(locked.toolchain, toolchain("GCCcore", "14.2.0"));
    assert!(
        !planned
            .root
            .plan
            .residuals
            .iter()
            .any(|residual| residual.category == "version-bump-dropped-dep"),
        "nothing is dropped once the companion exists: {:?}",
        planned.root.plan.residuals
    );
    let text = &planned.root.easyconfigs[0].text;
    assert!(
        text.contains("('Core', version)"),
        "the recipe keeps its expression:\n{text}"
    );
}

#[test]
fn a_literal_pin_the_tree_lacks_is_not_bumped() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (source, robot) = write_tree(temp.path());
    let text = fs::read_to_string(&source)
        .expect("source")
        .replace("('Core', version)", "('Core', '1.91.0')");
    fs::write(&source, text).expect("rewrite");
    let planned = plan_package_bump_with_companions(
        &request(source, &robot, "1.92.0"),
        &PackageEmitOptions::default(),
        None,
    )
    .expect("bump");
    assert!(
        planned.companions.is_empty(),
        "a literal version is the recipe author's, not one to invent"
    );
}

#[test]
fn companions_are_written_before_the_root_in_build_order() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (source, robot) = write_tree(temp.path());
    let planned = plan_package_bump_with_companions(
        &request(source, &robot, "1.92.0"),
        &PackageEmitOptions::default(),
        None,
    )
    .expect("bump with companions");
    let out = temp.path().join("bundle");
    let written = write_companions(&planned.root, &planned.companions, &out).expect("write");
    assert_eq!(
        written.easyconfigs,
        vec![out.join("easyconfigs/c/Core/Core-1.92.0-GCCcore-14.2.0.eb")]
    );
    assert!(out
        .join("packages/Core-1.92.0-GCCcore-14.2.0/package.plan.json")
        .is_file());
    let order: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&written.build_order).expect("order"))
            .expect("json");
    assert_eq!(
        order["recipes"],
        serde_json::json!([
            "easyconfigs/c/Core/Core-1.92.0-GCCcore-14.2.0.eb",
            "easyconfigs/w/Wrap/Wrap-1.92.0-foss-2025a.eb"
        ])
    );
    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&written.record).expect("record")).expect("json");
    assert_eq!(record[0]["name"], "Core");
    assert_eq!(record[0]["version"], "1.92.0");
    assert_eq!(record[0]["required_by"], "Wrap");
}
