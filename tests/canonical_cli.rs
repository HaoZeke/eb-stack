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

#[test]
fn package_bump_refuses_a_family_change() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("tests/repro_fixtures/gromacs/GROMACS-2024.4-foss-2023b.eb");
    let robot = root.join("tests/repro_fixtures/universe_foss_2024a");
    let temp = tempfile::tempdir().expect("tempdir");
    let result = Command::new(binary)
        .args([
            "package",
            "bump",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-name",
            "gfbf",
            "--toolchain-version",
            "2024a",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--out-dir",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .expect("package bump family change");
    assert!(!result.status.success());
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        err.contains("package mutate"),
        "expected mutate hint, got {err}"
    );
}

#[test]
fn package_mutate_accepts_a_family_change() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::write(
        robot.join("gfbf-2024a.eb"),
        "name = 'gfbf'\nversion = '2024a'\n\
         homepage = 'https://example.invalid/'\ndescription = 'stub'\n\
         toolchain = {'name': 'system', 'version': 'system'}\n",
    )
    .expect("gfbf toolchain stub");
    let hierarchy = temp.path().join("gfbf-2024a.json");
    std::fs::write(
        &hierarchy,
        r#"{"parent":{"name":"gfbf","version":"2024a"},"members":[{"name":"system","version":"system"},{"name":"gfbf","version":"2024a"}]}"#,
    )
    .expect("hierarchy fixture");
    let source = temp.path().join("Epsilon-1.0-foss-2023a.eb");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Epsilon'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['epsilon-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let output = temp.path().join("bundle");
    let result = Command::new(binary)
        .args([
            "package",
            "mutate",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-name",
            "gfbf",
            "--toolchain-version",
            "2024a",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--hierarchy-fixture",
            hierarchy.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package mutate");
    assert!(
        result.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("mode=mutate"), "{stdout}");
    assert!(output
        .join("easyconfigs/e/Epsilon/Epsilon-1.0-gfbf-2024a.eb")
        .is_file());
}

#[test]
fn package_bump_cli_prints_copied_sibling_patches() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("source directory");
    let patch_name = "Epsilon-1.0_fix.patch";
    std::fs::write(src_dir.join(patch_name), "--- a/x\n+++ b/x\n").expect("patch file");
    let source = src_dir.join("Epsilon-1.0-foss-2023a.eb");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'Epsilon'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['epsilon-1.0.tar.gz']\n\
         patches = ['Epsilon-1.0_fix.patch']\n\
         checksums = [\n    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',\n    '922dd52a81ff1c3d456cb861de7ad959496295c809971e848d414d3cdfe3fb23',\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
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
            "2025a",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package bump with sibling patch");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "stdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("patch=") && line.contains(patch_name)),
        "bump must print patch= for each copied file:\n{stdout}"
    );
}

#[test]
fn package_bump_same_generation_version_prints_no_companions() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("PLUMED-2.9.2-foss-2024a.eb");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'PLUMED'\nversion = '2.9.2'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         sources = ['plumed-2.9.2.tgz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('GSL', '2.8')]\n\
         moduleclass = 'chem'\n",
    )
    .expect("source recipe");
    std::fs::write(
        robot.join("GSL-2.8-foss-2024a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'GSL'\nversion = '2.8'\n\
         homepage = 'https://example.invalid/'\ndescription = 'GSL'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'numlib'\n",
    )
    .expect("gsl candidate");
    let output = temp.path().join("bundle");
    let result = Command::new(binary)
        .args([
            "package",
            "bump",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-version",
            "2024a",
            "--version",
            "2.9.3",
            "--source-checksum",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("same-generation version bump");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        result.status.success(),
        "same-generation version bump must exit 0:\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line == "generation_retarget=false"),
        "must label a same-generation bump before companions can be invented:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line == "version_only=true"),
        "must label a version-only bump:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line == "companion_count=0"),
        "must print companion_count=0:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("companion=")),
        "version-only bump must not emit companion=:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("re_run=")),
        "version-only bump must not emit re_run=:\n{stdout}"
    );
}

#[test]
fn package_bump_version_only_with_generation_hole_prints_no_companions() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("PLUMED-2.9.2-foss-2024a.eb");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'PLUMED'\nversion = '2.9.2'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         sources = [SOURCE_TGZ]\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('GSL', '2.8')]\n\
         moduleclass = 'chem'\n",
    )
    .expect("source recipe");
    let output = temp.path().join("bundle");
    let result = Command::new(binary)
        .args([
            "package",
            "bump",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-version",
            "2024a",
            "--version",
            "2.9.3",
            "--source-checksum",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("version-only bump with a generation hole");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        result.status.success(),
        "version-only parent exits 0 even with a generation hole:\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.lines().any(|line| line == "version_only=true"),
        "must stay version_only:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line == "companion_count=0"),
        "generation holes are not a companion campaign:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("companion=")),
        "version-only must not print companion=:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("re_run=")),
        "version-only must not print re_run=:\n{stdout}"
    );
}

#[test]
fn package_bump_cleared_checksum_exits_0() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("PLUMED-2.9.3-foss-2024a.eb");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::write(
        &source,
        "easyblock = 'ConfigureMake'\nname = 'PLUMED'\nversion = '2.9.3'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         sources = [SOURCE_TGZ]\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [('GSL', '2.8')]\n\
         moduleclass = 'chem'\n",
    )
    .expect("source recipe");
    std::fs::write(
        robot.join("GSL-2.8-foss-2024a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'GSL'\nversion = '2.8'\n\
         homepage = 'https://example.invalid/'\ndescription = 'GSL'\n\
         toolchain = {'name': 'foss', 'version': '2024a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'numlib'\n",
    )
    .expect("gsl candidate");
    let sibling_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    std::fs::write(
        robot.join("PLUMED-2.9.4-foss-2025a.eb"),
        format!(
            "easyblock = 'ConfigureMake'\nname = 'PLUMED'\nversion = '2.9.4'\n\
             homepage = 'https://example.invalid/'\ndescription = 'Sibling'\n\
             toolchain = {{'name': 'foss', 'version': '2025a'}}\n\
             sources = [SOURCE_TGZ]\n\
             checksums = ['{sibling_hash}']\n\
             dependencies = [('GSL', '2.8')]\n\
             moduleclass = 'chem'\n"
        ),
    )
    .expect("same-version sibling");
    let output = temp.path().join("bundle");
    let result = Command::new(binary)
        .args([
            "package",
            "bump",
            "--source",
            source.to_str().unwrap(),
            "--toolchain-version",
            "2024a",
            "--version",
            "2.9.4",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("version-only bump without --source-checksum");
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        result.status.success(),
        "cleared EasyBuild checksum is judgment, not a blocking exit:\nstdout={stdout}\nstderr={stderr}"
    );
    assert!(
        stdout.lines().any(|line| line == "version_only=true"),
        "must stay a version-only bump:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line == "companion_count=0"),
        "must print companion_count=0:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("companion=")),
        "cleared checksum must not invent companions:\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|line| line.starts_with("re_run=")),
        "cleared checksum must not invent re_run=:\n{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("residual=checksum")),
        "must still print the missing-sha256 residual:\n{stdout}"
    );
    let emitted =
        std::fs::read_to_string(output.join("easyconfigs/p/PLUMED/PLUMED-2.9.4-foss-2024a.eb"))
            .unwrap_or_else(|_| format!("missing recipe; stdout={stdout}"));
    assert!(
        emitted.contains("checksums = ['']") || emitted.contains("checksums = [\"\"]"),
        "cleared source slot must stay empty:\n{emitted}"
    );
    assert!(
        !emitted.contains(sibling_hash),
        "sibling generation hash must not fill the cleared slot:\n{emitted}"
    );
}

#[test]
fn package_bump_re_run_keeps_contributor() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gamma-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Gamma'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['gamma-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('KeptLib', '1.0'),\n    ('VanishedLib', '20211028'),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    std::fs::write(
        robot.join("KeptLib-1.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'KeptLib'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Kept'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("kept candidate");
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
            "2025a",
            "--version",
            "1.7.0",
            "--source-checksum",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--contributor",
            "Ada Lovelace",
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package bump with unresolved dep");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success(),
        "unresolved generation dep must fail: {combined}"
    );
    assert!(
        combined
            .lines()
            .any(|line| line == "generation_retarget=true"),
        "generation move must print generation_retarget=true: {combined}"
    );
    assert!(
        combined.lines().any(|line| line == "version_only=false"),
        "generation move is not version_only: {combined}"
    );
    let re_run = combined
        .lines()
        .find(|line| line.starts_with("re_run="))
        .expect("re_run= line");
    assert!(
        re_run.contains("--contributor") && re_run.contains("Ada Lovelace"),
        "re_run= must keep --contributor: {re_run}"
    );
}

#[test]
fn package_bump_companion_reprints_parent_eval_context() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("Gamma-1.0-foss-2023a.eb");
    let robot = temp.path().join("robot");
    let site = temp.path().join("site-robot");
    std::fs::create_dir_all(&robot).expect("robot");
    std::fs::create_dir_all(&site).expect("site robot");
    std::fs::write(
        &source,
        "easyblock = 'CMakeMake'\nname = 'Gamma'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Synthetic'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = ['gamma-1.0.tar.gz']\n\
         checksums = ['aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa']\n\
         dependencies = [\n    ('KeptLib', '1.0'),\n    ('VanishedLib', '20211028'),\n]\n\
         moduleclass = 'tools'\n",
    )
    .expect("source recipe");
    std::fs::write(
        robot.join("KeptLib-1.0-foss-2025a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'KeptLib'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Kept'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("kept candidate");
    std::fs::write(
        site.join("VanishedLib-20211028-foss-2023a.eb"),
        "easyblock = 'ConfigureMake'\nname = 'VanishedLib'\nversion = '20211028'\n\
         homepage = 'https://example.invalid/'\ndescription = 'Vanished'\n\
         toolchain = {'name': 'foss', 'version': '2023a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'lib'\n",
    )
    .expect("vanished source");
    let policy = temp.path().join("site.toml");
    std::fs::write(
        &policy,
        "schema_version = 1\nname = \"site\"\n[toolchain]\nname = \"foss\"\nversion = \"2025a\"\n",
    )
    .expect("stack policy");
    let hierarchy = temp.path().join("foss-2025a.json");
    std::fs::write(
        &hierarchy,
        r#"{"parent":{"name":"foss","version":"2025a"},"members":[{"name":"system","version":"system"},{"name":"foss","version":"2025a"}]}"#,
    )
    .expect("hierarchy fixture");
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
            "2025a",
            "--version",
            "1.7.0",
            "--source-checksum",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--easyconfigs",
            robot.to_str().unwrap(),
            "--easyconfigs",
            site.to_str().unwrap(),
            "--stack-policy",
            policy.to_str().unwrap(),
            "--hierarchy-fixture",
            hierarchy.to_str().unwrap(),
            "--contributor",
            "Ada Lovelace",
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .expect("package bump with unresolved dep");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success(),
        "unresolved generation dep must fail: {combined}"
    );
    let companions: Vec<&str> = combined
        .lines()
        .filter(|line| line.starts_with("companion="))
        .collect();
    assert!(
        !companions.is_empty(),
        "expected companion= lines: {combined}"
    );
    for line in &companions {
        assert!(
            line.contains(robot.to_str().unwrap()),
            "companion must keep first --easyconfigs: {line}"
        );
        assert!(
            line.contains(site.to_str().unwrap()),
            "companion must keep second --easyconfigs: {line}"
        );
        assert!(
            line.contains("--stack-policy") && line.contains(policy.to_str().unwrap()),
            "companion must keep --stack-policy: {line}"
        );
        assert!(
            line.contains("--hierarchy-fixture") && line.contains(hierarchy.to_str().unwrap()),
            "companion must keep --hierarchy-fixture: {line}"
        );
        assert!(
            line.contains("--contributor") && line.contains("Ada Lovelace"),
            "companion must keep --contributor: {line}"
        );
    }
}

#[test]
fn recipe_lint_style_finding_names_the_file() {
    let binary = env!("CARGO_BIN_EXE_eb-stack");
    let temp = tempfile::tempdir().expect("tempdir");
    let legal = temp.path().join("legal.eb");
    let long = temp.path().join("too-long.eb");
    std::fs::write(
        &legal,
        "easyblock = 'ConfigureMake'\nname = 'Legal'\nversion = '1.0'\n\
         homepage = 'https://example.invalid/'\ndescription = 'ok'\n\
         toolchain = {'name': 'foss', 'version': '2025a'}\n\
         sources = []\nchecksums = []\nmoduleclass = 'tools'\n",
    )
    .expect("legal recipe");
    let over = format!("description = '{}'\n", "x".repeat(130));
    std::fs::write(&long, over).expect("long recipe");
    let result = Command::new(binary)
        .args([
            "recipe",
            "lint",
            legal.to_str().unwrap(),
            long.to_str().unwrap(),
        ])
        .output()
        .expect("recipe lint two files");
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        !result.status.success(),
        "E501 must fail lint: {stdout}\nstderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("lint json");
    let style = parsed
        .get("style")
        .and_then(|value| value.as_array())
        .expect("style array");
    assert_eq!(style.len(), 1, "one E501 expected: {stdout}");
    let path = style[0]
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    assert!(
        path.contains("too-long.eb"),
        "style finding must name the long file, got {path:?} in {stdout}"
    );
    assert!(
        !path.contains("legal.eb"),
        "style finding must not name the legal file: {stdout}"
    );
}
