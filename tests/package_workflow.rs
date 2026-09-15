use eb_stack::package::{StackPin, StackPinMode, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::{
    inspect_new_package, plan_new_package, plan_package_bump, prepare_package_bump,
    resolve_easyconfig_file, write_package_bundle, BumpPackageRequest, ForeignFormat,
    NewPackageRequest, Toolchain,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;

fn toolchain() -> Toolchain {
    Toolchain {
        name: "foss".into(),
        version: "2026.1".into(),
    }
}

#[test]
fn foreign_source_becomes_sbom_manifest_lock_and_easyconfig_set() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("recipe.yaml");
    std::fs::write(
        &source,
        r#"
package:
  name: eon
  version: 2.16.0
source:
  url: https://example.invalid/eon-2.16.0.tar.gz
build:
  script: meson setup build
requirements:
  host:
    - zlib >=1.2
"#,
    )
    .expect("write source");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot dir");
    for version in ["1.2", "1.3"] {
        std::fs::write(
            robot.join(format!("zlib-{version}-foss-2026.1.eb")),
            format!(
                "name = 'zlib'\nversion = '{version}'\ntoolchain = {{'name': 'foss', 'version': '2026.1'}}\n"
            ),
        )
        .expect("write candidate");
    }
    let profile = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1
[package]
name = "eOn"
[[profiles]]
name = "default"
default = true
config_options = ["-Dwith_cli=true"]
"#,
    )
    .expect("package config");
    let stack_policy = StackPolicy {
        schema_version: STACK_POLICY_SCHEMA_VERSION,
        name: "site".into(),
        toolchain: toolchain(),
        pins: vec![StackPin {
            name: "zlib".into(),
            version_requirement: "==1.2".into(),
            toolchain: None,
            versionsuffix: None,
            mode: StackPinMode::Preferred,
            source: Some("site-stack.toml".into()),
        }],
        exclusions: Vec::new(),
    };

    let missing_checksum = plan_new_package(&NewPackageRequest {
        source: source.clone(),
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: vec![profile.clone()],
        package_index: Default::default(),
        easyconfig_roots: vec![robot.clone()],
        stack_policy: stack_policy.clone(),
    })
    .expect_err("planning must reject a source without a packaging checksum");
    assert!(
        missing_checksum.to_string().contains("source checksum"),
        "{missing_checksum}"
    );

    let checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let patch_without_checksum = PackageConfigLayer::from_toml_str(
        r#"
schema_version = 1
[package]
name = "eOn"
[[build.patches]]
filename = "eOn-2.16.0-portability.patch"
[[profiles]]
name = "default"
default = true
config_options = ["-Dwith_cli=true"]
"#,
    )
    .expect("unchecked patch config");
    let missing_patch_checksum = plan_new_package(&NewPackageRequest {
        source: source.clone(),
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: vec![checksum.into()],
        package_layers: vec![patch_without_checksum],
        package_index: Default::default(),
        easyconfig_roots: vec![robot.clone()],
        stack_policy: stack_policy.clone(),
    })
    .expect_err("planning must reject a patch without a packaging checksum");
    assert!(
        missing_patch_checksum
            .to_string()
            .contains("patch checksum"),
        "{missing_patch_checksum}"
    );

    let patch_source = temp.path().join("eOn-2.16.0-portability.patch");
    std::fs::write(&patch_source, "portable patch\n").expect("write patch asset");
    let patch_checksum = "a35aa78c890e616e051e84c9df0a1187cd88b852610748804c97388c3a9cb2c5";
    let profile_with_patch = PackageConfigLayer::from_toml_str(&format!(
        r#"
schema_version = 1
[package]
name = "eOn"
[[build.patches]]
filename = "eOn-2.16.0-portability.patch"
sha256 = "{patch_checksum}"
source = "{patch_source}"
[[profiles]]
name = "default"
default = true
config_options = ["-Dwith_cli=true"]
"#,
        patch_source = patch_source.display(),
    ))
    .expect("checksummed patch config");
    let bundle = plan_new_package(&NewPackageRequest {
        source: source.clone(),
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: vec![checksum.into()],
        package_layers: vec![profile_with_patch],
        package_index: Default::default(),
        easyconfig_roots: vec![robot],
        stack_policy,
    })
    .expect("plan new package");
    assert_eq!(bundle.plan.package.name, "eOn");
    assert_eq!(bundle.sbom["bomFormat"], "CycloneDX");
    assert_eq!(bundle.locks.len(), 1);
    assert_eq!(bundle.locks[0].dependencies[0].name, "zlib");
    assert_eq!(bundle.locks[0].dependencies[0].version, "1.2");
    assert_eq!(bundle.easyconfigs.len(), 1);
    assert_eq!(bundle.plan.sources[0].sha256.as_deref(), Some(checksum));
    assert!(!bundle
        .plan
        .residuals
        .iter()
        .any(|residual| residual.id == "source:missing-sha256"));
    assert_eq!(
        bundle.sbom["metadata"]["component"]["hashes"][0]["content"],
        checksum
    );

    let out = temp.path().join("bundle");
    let written = write_package_bundle(&bundle, &out).expect("write bundle");
    assert!(written.manifest.is_file());
    assert!(written.sbom.is_file());
    assert_eq!(written.locks.len(), 1);
    assert_eq!(written.easyconfigs.len(), 1);
    assert_eq!(written.patches.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&written.patches[0]).expect("copied patch"),
        "portable patch\n"
    );
    assert_eq!(written.patches[0].parent(), written.easyconfigs[0].parent());
    let parsed = resolve_easyconfig_file(&written.easyconfigs[0]).expect("parse emitted recipe");
    assert_eq!(parsed.name, "eOn");
    assert_eq!(parsed.dependencies[0].name, "zlib");
    assert_eq!(parsed.dependencies[0].version, "1.2");
    assert_eq!(
        parsed.checksums,
        vec![checksum.to_string(), patch_checksum.to_string()]
    );
    assert_eq!(parsed.patch_names, ["eOn-2.16.0-portability.patch"]);
    assert!(parsed
        .configopts
        .as_deref()
        .is_some_and(|options| options.contains("-Dwith_cli=true")));
}

#[test]
fn foreign_recipe_local_patches_are_hashed_and_resolved() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe_dir = temp.path().join("recipe");
    let patch = recipe_dir.join("patches/fix.patch");
    std::fs::create_dir_all(patch.parent().expect("patch parent")).expect("patch directory");
    let patch_bytes = b"authoritative patch bytes\n";
    std::fs::write(&patch, patch_bytes).expect("write patch");
    let source = recipe_dir.join("meta.yaml");
    std::fs::write(
        &source,
        r#"package:
  name: patch-fixture
  version: "1.0"
source:
  url: https://example.invalid/patch-fixture-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  patches:
    - patches/fix.patch
"#,
    )
    .expect("write recipe");

    let (plan, _) =
        inspect_new_package(&source, Some(ForeignFormat::CondaForge), &toolchain(), &[])
            .expect("inspect recipe with a local patch");

    let artifact = plan.build.patches.first().expect("patch artifact");
    let expected_sha256 =
        Sha256::digest(patch_bytes)
            .iter()
            .fold(String::new(), |mut output, byte| {
                write!(&mut output, "{byte:02x}").expect("format digest");
                output
            });
    assert_eq!(artifact.filename, "fix.patch");
    assert_eq!(artifact.source.as_deref(), Some("patches/fix.patch"));
    assert_eq!(artifact.resolved_source.as_deref(), Some(patch.as_path()));
    assert_eq!(artifact.sha256.as_deref(), Some(expected_sha256.as_str()));
    assert!(!plan.residuals.iter().any(|residual| {
        matches!(
            residual.id.as_str(),
            "patch:missing-sha256" | "patch:missing-source"
        )
    }));
}

#[test]
fn a_declared_patch_missing_beside_the_recipe_is_a_residual() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe_dir = temp.path().join("recipe");
    std::fs::create_dir_all(&recipe_dir).expect("recipe directory");
    let source = recipe_dir.join("meta.yaml");
    std::fs::write(
        &source,
        r#"package:
  name: patch-fixture
  version: "1.0"
source:
  url: https://example.invalid/patch-fixture-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  patches:
    - patches/fix.patch
"#,
    )
    .expect("write recipe");
    let cwd_decoy = std::env::current_dir()
        .expect("cwd")
        .join("patches/fix.patch");
    assert!(
        !cwd_decoy.is_file(),
        "this test must not hash a CWD decoy at {}",
        cwd_decoy.display()
    );

    let (plan, _) =
        inspect_new_package(&source, Some(ForeignFormat::CondaForge), &toolchain(), &[])
            .expect("inspect recipe with a missing local patch");

    assert!(
        plan.residuals
            .iter()
            .any(|residual| residual.id == "patch:missing-source"),
        "{:?}",
        plan.residuals
    );
    let artifact = plan.build.patches.first().expect("patch artifact");
    assert!(artifact.resolved_source.is_none(), "{artifact:?}");
    assert!(artifact.source.is_none(), "{artifact:?}");
    assert!(artifact.sha256.is_none(), "{artifact:?}");
}

#[test]
fn non_utf8_local_patch_is_copied_into_the_bundle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let recipe_dir = temp.path().join("recipe");
    let patch = recipe_dir.join("patches/fix.patch");
    std::fs::create_dir_all(patch.parent().expect("patch parent")).expect("patch directory");
    let patch_bytes: &[u8] = b"binary\xffpatch\n";
    std::fs::write(&patch, patch_bytes).expect("write patch");
    let patch_checksum =
        Sha256::digest(patch_bytes)
            .iter()
            .fold(String::new(), |mut output, byte| {
                write!(&mut output, "{byte:02x}").expect("format digest");
                output
            });
    let source = recipe_dir.join("meta.yaml");
    std::fs::write(
        &source,
        format!(
            r#"package:
  name: patch-fixture
  version: "1.0"
source:
  url: https://example.invalid/patch-fixture-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  patches:
    - patches/fix.patch
"#
        ),
    )
    .expect("write recipe");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot directory");
    let bundle = plan_new_package(&NewPackageRequest {
        source,
        format: Some(ForeignFormat::CondaForge),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        package_index: Default::default(),
        easyconfig_roots: vec![robot],
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "test".into(),
            toolchain: toolchain(),
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect("plan binary patch");
    assert_eq!(
        bundle.plan.build.patches[0].sha256.as_deref(),
        Some(patch_checksum.as_str())
    );
    let written = write_package_bundle(&bundle, &temp.path().join("bundle")).expect("write bundle");
    assert_eq!(written.patches.len(), 1);
    assert_eq!(
        std::fs::read(&written.patches[0]).expect("read overlay patch"),
        patch_bytes
    );
}

#[test]
fn remote_spack_patches_need_no_local_bundle_asset() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("package.py");
    std::fs::write(
        &source,
        r#"
class Orbit(Package):
    homepage = "https://example.invalid/orbit"
    url = "https://example.invalid/orbit-2.0.tar.gz"
    version("2.0", sha256="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    patch(
        "https://example.invalid/commits/fix.patch?full_index=1",
        sha256="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
"#,
    )
    .expect("write Spack package");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot directory");
    let bundle = plan_new_package(&NewPackageRequest {
        source,
        format: Some(ForeignFormat::Spack),
        toolchain: toolchain(),
        source_checksums: Vec::new(),
        package_layers: Vec::new(),
        package_index: Default::default(),
        easyconfig_roots: vec![robot],
        stack_policy: StackPolicy {
            schema_version: STACK_POLICY_SCHEMA_VERSION,
            name: "test".into(),
            toolchain: toolchain(),
            pins: Vec::new(),
            exclusions: Vec::new(),
        },
    })
    .expect("plan remote patch");

    let written = write_package_bundle(&bundle, &temp.path().join("bundle"))
        .expect("write remote-patch bundle");
    assert!(written.patches.is_empty());
    let easyconfig = std::fs::read_to_string(&written.easyconfigs[0]).expect("easyconfig");
    // One patch that fits on a line is written on a line, which is how
    // upstream writes a single patch.
    assert!(
        easyconfig.contains("patches = ['https://example.invalid/commits/fix.patch?full_index=1']"),
        "{easyconfig}"
    );
}

#[test]
fn same_system_spelling_bump_keeps_exact_dep_pins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("App-1.0.eb");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot directory");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'App'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'SYSTEM app'\n\
         toolchain = {'name': 'dummy', 'version': ''}\n\
         sources = ['app-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('Lib', '1.0')]\nmoduleclass = 'tools'\n",
    )
    .expect("source recipe");
    for version in ["1.0", "2.0"] {
        std::fs::write(
            robot.join(format!("Lib-{version}.eb")),
            format!(
                "easyblock = 'ConfigureMake'\nname = 'Lib'\nversion = '{version}'\n\
                 homepage = 'https://example.invalid/'\ndescription = 'SYSTEM lib'\n\
                 toolchain = SYSTEM\nsources = []\nchecksums = []\nmoduleclass = 'lib'\n"
            ),
        )
        .expect("lib candidate");
    }
    let toolchain = Toolchain {
        name: "system".into(),
        version: "system".into(),
    };
    let request = BumpPackageRequest {
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
    };

    let (plan, _) = prepare_package_bump(&request).expect("prepare same-SYSTEM bump");
    let lib = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Lib")
        .expect("Lib intent");
    assert_eq!(lib.constraint.as_deref(), Some("==1.0"));

    let bundle = plan_package_bump(&request).expect("plan same-SYSTEM bump");
    let locked = bundle.locks[0]
        .dependencies
        .iter()
        .find(|dependency| dependency.name == "Lib")
        .expect("Lib lock");
    assert_eq!(locked.version, "1.0");
}
