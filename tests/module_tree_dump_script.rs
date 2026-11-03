//! The live-module dump helper isolates and records all three inventories.

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn helper_dumps_site_older_and_target_module_trees() {
    let temporary = tempfile::tempdir().expect("temp");
    let bash_env = temporary.path().join("module-init.sh");
    std::fs::write(
        &bash_env,
        r#"
module() {
  case "$1 $2" in
    "--force purge") unset TEST_MODULE ;;
    "load EESSI/2025.06") export TEST_MODULE=older ;;
    "load EESSI/2026.06") export TEST_MODULE=target ;;
    "-t avail")
      case "${TEST_MODULE:-site}" in
        site) printf 'SiteApp/1.0\n' ;;
        older) printf 'LAMMPS/2024.0\n' ;;
        target) printf 'FFTW/3.3.10-gompi-2026.1\n' ;;
      esac
      ;;
    *) return 2 ;;
  esac
}
"#,
    )
    .expect("module init");
    let output_dir = temporary.path().join("dumps");
    let output = Command::new(root().join("scripts/dump-module-trees.sh"))
        .env("BASH_ENV", &bash_env)
        .arg(&output_dir)
        .args(["-", "EESSI/2025.06", "EESSI/2026.06"])
        .output()
        .expect("dump helper");
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
