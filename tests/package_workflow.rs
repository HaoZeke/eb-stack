use eb_stack::package::{StackPin, StackPinMode, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::{
    inspect_new_package, plan_new_package, resolve_easyconfig_file, write_package_bundle,
    ForeignFormat, NewPackageRequest, Toolchain,
};
use sha2::{Digest, Sha256};
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
