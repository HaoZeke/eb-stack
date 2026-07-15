use eb_stack::package::{StackPin, StackPinMode, StackPolicy, STACK_POLICY_SCHEMA_VERSION};
use eb_stack::package_config::PackageConfigLayer;
use eb_stack::{
    plan_new_package, resolve_easyconfig_file, write_package_bundle, ForeignFormat,
    NewPackageRequest, Toolchain,
};

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

    let patch_checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let profile_with_patch = PackageConfigLayer::from_toml_str(&format!(
        r#"
schema_version = 1
[package]
name = "eOn"
[[build.patches]]
filename = "eOn-2.16.0-portability.patch"
sha256 = "{patch_checksum}"
[[profiles]]
name = "default"
default = true
config_options = ["-Dwith_cli=true"]
"#
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
