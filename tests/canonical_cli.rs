use std::process::Command;

#[test]
fn command_surface_is_namespaced_without_flat_legacy_aliases() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let help = Command::new(binary)
        .arg("--help")
        .output()
        .expect("run help");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    for namespace in ["package", "recipe", "stack", "target", "campaign", "mcp"] {
        assert!(stdout.contains(namespace), "missing {namespace}: {stdout}");
    }

    let legacy = Command::new(binary)
        .args(["ingest", "--help"])
        .output()
        .expect("run legacy command");
    assert!(
        !legacy.status.success(),
        "legacy ingest alias must be removed"
    );
}

#[test]
fn package_plan_cli_writes_the_canonical_bundle() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
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
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  host:
    - zlib >=1.2
"#,
    )
    .expect("source");
    let package_config = temp.path().join("package.toml");
    std::fs::write(
        &package_config,
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
    let stack = temp.path().join("stack.toml");
    std::fs::write(
        &stack,
        r#"
schema_version = 1
name = "site"
[toolchain]
name = "foss"
version = "2026.1"
[[pins]]
name = "zlib"
version_requirement = "==1.2"
mode = "preferred"
"#,
    )
    .expect("stack");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    std::fs::write(
        robot.join("zlib-1.2-foss-2026.1.eb"),
        "name = 'zlib'\nversion = '1.2'\ntoolchain = {'name': 'foss', 'version': '2026.1'}\n",
    )
    .expect("candidate");
    let output = temp.path().join("output");

    let result = Command::new(binary)
        .args([
            "package",
            "plan",
            "--source",
            source.to_str().unwrap(),
            "--format",
            "conda-forge",
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2026.1",
            "--package-config",
            package_config.to_str().unwrap(),
            "--source-checksum",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--stack-policy",
            stack.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package plan");
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("package.plan.json").is_file());
    assert!(output.join("package.sbom.cdx.json").is_file());
    assert!(output.join("locks/default.lock.json").is_file());
    assert!(output
        .join("easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb")
        .is_file());
    let recipe =
        std::fs::read_to_string(output.join("easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb"))
            .expect("emitted recipe");
    assert!(recipe.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
}

#[test]
fn package_plan_reuses_robot_roots_for_cross_generation_bumps() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("alpha.yaml");
    std::fs::write(
        &source,
        r#"package:
  name: alpha
  version: "1.0"
source:
  url: https://example.invalid/alpha-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  host:
    - bravo >=1.5
"#,
    )
    .expect("source");
    let stack = temp.path().join("stack.toml");
    std::fs::write(
        &stack,
        r#"schema_version = 1
name = "site"
[toolchain]
name = "foss"
version = "2026.1"
"#,
    )
    .expect("stack");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    std::fs::write(
        robot.join("bravo-1.5-foss-2023b.eb"),
        "name = 'bravo'\n\
         version = '1.5'\n\
         homepage = 'https://example.invalid/bravo'\n\
         description = 'synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023b'}\n\
         sources = ['bravo-1.5.tar.gz']\n\
         checksums = ['bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb']\n\
         moduleclass = 'lib'\n",
    )
    .expect("source easyconfig");
    let output = temp.path().join("output");
    let result = Command::new(binary)
        .args([
            "package",
            "plan",
            "--source",
            source.to_str().unwrap(),
            "--format",
            "conda-forge",
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2026.1",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--stack-policy",
            stack.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package plan");
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("closure.plan.json").is_file());
    assert!(output
        .join("easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb")
        .is_file());
}

#[test]
fn package_plan_cli_closes_catalog_backed_robot_holes() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let root_source = temp.path().join("alpha.yaml");
    std::fs::write(
        &root_source,
        r#"
package:
  name: alpha
  version: "1.0"
source:
  url: https://example.invalid/alpha-1.0.tar.gz
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
requirements:
  host:
    - bravo >=1.0
"#,
    )
    .expect("root source");
    let companion_source = temp.path().join("bravo.yaml");
    std::fs::write(
        &companion_source,
        r#"
package:
  name: bravo
  version: "1.5"
source:
  url: https://example.invalid/bravo-1.5.tar.gz
  sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
"#,
    )
    .expect("companion source");
    let catalog = temp.path().join("catalog.toml");
    std::fs::write(
        &catalog,
        r#"
schema_version = 1

[[packages]]
name = "bravo"
version = "1.5"
source = "bravo.yaml"
format = "conda-forge"
profile = "default"
toolchain = { name = "foss", version = "2026.1" }
"#,
    )
    .expect("catalog");
    let stack = temp.path().join("stack.toml");
    std::fs::write(
        &stack,
        r#"
schema_version = 1
name = "site"
[toolchain]
name = "foss"
version = "2026.1"
"#,
    )
    .expect("stack");
    let robot = temp.path().join("robot");
    std::fs::create_dir(&robot).expect("robot");
    let output = temp.path().join("output");

    let result = Command::new(binary)
        .args([
            "package",
            "plan",
            "--source",
            root_source.to_str().unwrap(),
            "--format",
            "conda-forge",
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2026.1",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--stack-policy",
            stack.to_str().unwrap(),
            "--package-catalog",
            catalog.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package closure plan");
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );

    let root_recipe = "easyconfigs/a/alpha/alpha-1.0-foss-2026.1.eb";
    let companion_recipe = "easyconfigs/b/bravo/bravo-1.5-foss-2026.1.eb";
    assert!(output.join(root_recipe).is_file());
    assert!(output.join(companion_recipe).is_file());
    assert!(output.join("package.plan.json").is_file());
    assert!(output.join("package.sbom.cdx.json").is_file());
    assert!(output.join("locks/default.lock.json").is_file());
    assert!(output
        .join("packages/bravo-1.5-foss-2026.1/package.plan.json")
        .is_file());
    assert!(output
        .join("packages/bravo-1.5-foss-2026.1/package.sbom.cdx.json")
        .is_file());
    assert!(output
        .join("packages/bravo-1.5-foss-2026.1/locks/default.lock.json")
        .is_file());
    assert!(output.join("closure.plan.json").is_file());
    assert!(output.join("closure.sbom.cdx.json").is_file());

    let build_order: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(output.join("build-order.json")).expect("build order"),
    )
    .expect("build-order JSON");
    assert_eq!(
        build_order["recipes"],
        serde_json::json!([companion_recipe, root_recipe])
    );
    let aggregate_sbom: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(output.join("closure.sbom.cdx.json")).expect("aggregate SBOM"),
    )
    .expect("aggregate SBOM JSON");
    let component_names = aggregate_sbom["components"]
        .as_array()
        .expect("aggregate components")
        .iter()
        .filter_map(|component| component["name"].as_str())
        .collect::<Vec<_>>();
    assert!(component_names.contains(&"alpha"));
    assert!(component_names.contains(&"bravo"));
}

#[test]
fn package_bump_cli_writes_an_sbom_resolvo_bundle() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb");
    let robot = root.join("tests/repro_fixtures/universe_foss_2024a");
    let temp = tempfile::tempdir().expect("tempdir");
    let output = temp.path().join("bundle");

    let result = Command::new(binary)
        .args([
            "package",
            "bump",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-name",
            "foss",
            "--toolchain-version",
            "2024a",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package bump");
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("package.plan.json").is_file());
    assert!(output.join("package.sbom.cdx.json").is_file());
    assert!(output.join("locks/default.lock.json").is_file());
    assert!(output
        .join("easyconfigs/g/GROMACS/GROMACS-2024.4-foss-2024a.eb")
        .is_file());
}
