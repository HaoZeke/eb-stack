//! The live-module dump helper isolates and records all three inventories.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A `module` stub that loads each EESSI version and lists one module per tree.
/// `purge`, `load_target` and `is_loaded_target` are the shell bodies of the
/// purge branch and the target's load and is-loaded branches, so a case can
/// make any one of them fail.
fn module_stub(purge: &str, load_target: &str, is_loaded_target: &str) -> String {
    format!(
        r#"
module() {{
  case "$1 $2" in
    "--force purge") {purge} ;;
    "load EESSI/2025.06") export TEST_MODULE=older ;;
    "load EESSI/2026.06") {load_target} ;;
    "is-loaded EESSI/2025.06") [[ ${{TEST_MODULE:-}} == older ]] ;;
    "is-loaded EESSI/2026.06") {is_loaded_target} ;;
    "-t avail")
      case "${{TEST_MODULE:-site}}" in
        site) printf 'SiteApp/1.0\n' ;;
        older) printf 'LAMMPS/2024.0\n' ;;
        target) printf 'FFTW/3.3.10-gompi-2026.1\n' ;;
      esac
      ;;
    *) return 2 ;;
  esac
}}
"#
    )
}

const PURGE: &str = "unset TEST_MODULE";
const LOAD_TARGET: &str = "export TEST_MODULE=target";
const IS_LOADED_TARGET: &str = "[[ ${TEST_MODULE:-} == target ]]";
/// The helper's exit status when no `module` function can be initialized.
const EXIT_USAGE: i32 = 2;

/// Run the helper with a clean environment, so a `module` function exported
/// by the calling shell cannot stand in for the stub.
fn run_helper<K: AsRef<OsStr>, V: AsRef<OsStr>>(
    output_dir: &Path,
    env: impl IntoIterator<Item = (K, V)>,
) -> Output {
    let mut command = Command::new(root().join("scripts/dump-module-trees.sh"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command
        .envs(env)
        .arg(output_dir)
        .args(["-", "EESSI/2025.06", "EESSI/2026.06"])
        .output()
        .expect("dump helper")
}

fn assert_all_trees(output: &Output, output_dir: &Path) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(output_dir.join("site.txt")).unwrap(),
        "SiteApp/1.0\n"
    );
    assert_eq!(
        std::fs::read_to_string(output_dir.join("eessi-older.txt")).unwrap(),
        "LAMMPS/2024.0\n"
    );
    assert_eq!(
        std::fs::read_to_string(output_dir.join("eessi-target.txt")).unwrap(),
        "FFTW/3.3.10-gompi-2026.1\n"
    );
}

fn assert_target_refused(output: &Output, output_dir: &Path, reason: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("EESSI/2026.06"), "{stderr}");
    assert!(stderr.contains(reason), "{stderr}");
    assert!(
        !output_dir.join("eessi-target.txt").exists(),
        "a target inventory exists after a failed load"
    );
    assert!(!output_dir.join("eessi-target.txt.tmp").exists());
}

#[test]
fn helper_dumps_site_older_and_target_module_trees() {
    let temporary = tempfile::tempdir().expect("temp");
    let bash_env = temporary.path().join("module-init.sh");
    std::fs::write(&bash_env, module_stub(PURGE, LOAD_TARGET, IS_LOADED_TARGET))
        .expect("module init");
    let output_dir = temporary.path().join("dumps");
    let output = run_helper(&output_dir, [("BASH_ENV", &bash_env)]);
    assert_all_trees(&output, &output_dir);
}

#[test]
fn helper_sources_the_lmod_init_under_moduleshome() {
    let temporary = tempfile::tempdir().expect("temp");
    let modules_home = temporary.path().join("lmod");
    std::fs::create_dir_all(modules_home.join("init")).expect("init dir");
    std::fs::write(
        modules_home.join("init/bash"),
        module_stub(PURGE, LOAD_TARGET, IS_LOADED_TARGET),
    )
    .expect("module init");
    let output_dir = temporary.path().join("dumps");
    let output = run_helper(&output_dir, [("MODULESHOME", &modules_home)]);
    assert_all_trees(&output, &output_dir);
}

#[test]
fn helper_exits_when_no_lmod_init_exists() {
    let temporary = tempfile::tempdir().expect("temp");
    let output_dir = temporary.path().join("dumps");
    let missing = temporary.path().join("missing");
    let output = run_helper(
        &output_dir,
        [("LMOD_PKG", &missing), ("MODULESHOME", &missing)],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(EXIT_USAGE), "{stderr}");
    assert!(stderr.contains("not initialized"), "{stderr}");
    assert!(!output_dir.exists());
}

#[test]
fn helper_fails_with_the_lmod_error_when_a_load_fails() {
    let temporary = tempfile::tempdir().expect("temp");
    let bash_env = temporary.path().join("module-init.sh");
    let failing_load = "echo 'Lmod has detected the following error: no such module' >&2; return 1";
    std::fs::write(
        &bash_env,
        module_stub(PURGE, failing_load, IS_LOADED_TARGET),
    )
    .expect("module init");
    let output_dir = temporary.path().join("dumps");
    std::fs::create_dir_all(&output_dir).expect("dump dir");
    std::fs::write(output_dir.join("eessi-target.txt"), "Stale/1.0\n").expect("stale inventory");
    let output = run_helper(&output_dir, [("BASH_ENV", &bash_env)]);
    assert_target_refused(&output, &output_dir, "no such module");
}

#[test]
fn helper_fails_when_the_module_is_not_loaded_after_load() {
    let temporary = tempfile::tempdir().expect("temp");
    let bash_env = temporary.path().join("module-init.sh");
    std::fs::write(&bash_env, module_stub(PURGE, "true", IS_LOADED_TARGET)).expect("module init");
    let output_dir = temporary.path().join("dumps");
    let output = run_helper(&output_dir, [("BASH_ENV", &bash_env)]);
    assert_target_refused(&output, &output_dir, "is not loaded");
}

#[test]
fn helper_prefers_the_lmod_init_under_lmod_pkg() {
    let temporary = tempfile::tempdir().expect("temp");
    let lmod_pkg = temporary.path().join("lmod-pkg");
    std::fs::create_dir_all(lmod_pkg.join("init")).expect("init dir");
    std::fs::write(
        lmod_pkg.join("init/bash"),
        module_stub(PURGE, LOAD_TARGET, IS_LOADED_TARGET),
    )
    .expect("module init");
    let modules_home = temporary.path().join("modules-home");
    std::fs::create_dir_all(modules_home.join("init")).expect("init dir");
    std::fs::write(
        modules_home.join("init/bash"),
        "echo 'MODULESHOME init sourced' >&2\nmodule() { return 1; }\n",
    )
    .expect("module init");
    let output_dir = temporary.path().join("dumps");
    let output = run_helper(
        &output_dir,
        [("LMOD_PKG", &lmod_pkg), ("MODULESHOME", &modules_home)],
    );
    assert_all_trees(&output, &output_dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("MODULESHOME init sourced"), "{stderr}");
}

#[test]
fn helper_fails_with_the_lmod_error_when_a_purge_fails() {
    let temporary = tempfile::tempdir().expect("temp");
    let bash_env = temporary.path().join("module-init.sh");
    let failing_purge = "echo 'purge refused by the stub' >&2; return 1";
    std::fs::write(
        &bash_env,
        module_stub(failing_purge, LOAD_TARGET, IS_LOADED_TARGET),
    )
    .expect("module init");
    let output_dir = temporary.path().join("dumps");
    std::fs::create_dir_all(&output_dir).expect("dump dir");
    std::fs::write(output_dir.join("site.txt"), "Stale/1.0\n").expect("stale inventory");
    let output = run_helper(&output_dir, [("BASH_ENV", &bash_env)]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("module --force purge failed"), "{stderr}");
    assert!(stderr.contains("purge refused by the stub"), "{stderr}");
    assert!(!output_dir.join("site.txt").exists());
    assert!(!output_dir.join("site.txt.tmp").exists());
}
