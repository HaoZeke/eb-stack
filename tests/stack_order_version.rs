//! Two runtime pins of one dependency at one toolchain.
//!
//! Eigen 3.4.1 and Eigen 5.0.0 are the versions in
//! `a_runtime_dependency_still_takes_one_version_per_stack`: a runtime
//! dependency has to agree on one version, so `stack solve` rejects the
//! closure. The names are the Snellius 2026 site-layer pair, LAMMPS and
//! SciPy-bundle. `stack order` must refuse that closure and name both
//! requesters, rather than writing an order `stack solve` cannot install.

use std::path::PathBuf;
use std::process::Command;

fn easyconfigs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/stack_order_version_conflict/easyconfigs")
}

#[test]
fn two_runtime_versions_in_one_toolchain_are_refused() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("order.txt");
    let status = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "order", "--easyconfigs"])
        .arg(easyconfigs())
        .args(["--root", "LAMMPS", "--root", "SciPy-bundle", "--out"])
        .arg(&out)
        .env_remove("EASYBUILD_FILTER_DEPS")
        .output()
        .expect("run stack order");
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        !status.status.success(),
        "two Eigen versions were written into the order:\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&status.stdout)
    );
    assert_eq!(status.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("Eigen"), "{stderr}");
    assert!(stderr.contains("3.4.1"), "{stderr}");
    assert!(stderr.contains("5.0.0"), "{stderr}");
    assert!(stderr.contains("LAMMPS"), "{stderr}");
    assert!(stderr.contains("SciPy-bundle"), "{stderr}");
    assert!(
        !out.exists(),
        "stack order wrote {} after refusing the closure",
        out.display()
    );
}
