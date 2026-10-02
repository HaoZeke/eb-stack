//! The Jenkins build-list format, in and out of `stack solve`.

use std::path::PathBuf;
use std::process::Command;

fn easyconfigs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next/easyconfigs")
}

#[test]
fn roots_come_from_one_list_and_site_flags_carry_into_the_next() {
    let tmp = tempfile::tempdir().unwrap();
    let previous = tmp.path().join("previous.list");
    std::fs::write(
        &previous,
        "#### MPI ####\n\
OpenMPI-4.1.5-foss-2025a.eb --hooks=/sw/hooks/mpi_hook.py --from-commit=953321cef7def8a07ac690c9c528ce859f3e9702\n\
##\n\
GROMACS-2024.1-foss-2025a.eb --ignore-test-failure\n",
    )
    .unwrap();
    let out = tmp.path().join("next.list");
    let st = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(easyconfigs())
        .args(["--toolchain", "foss/2025b", "--roots-from-build-list"])
        .arg(&previous)
        .arg("--lock-out")
        .arg(tmp.path().join("stack.lock.json"))
        .arg("--jenkins-build-list-out")
        .arg(&out)
        .arg("--jenkins-flags-from")
        .arg(&previous)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let text = std::fs::read_to_string(&out).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let openmpi = lines
        .iter()
        .position(|l| l.starts_with("OpenMPI-") && l.contains("-foss-2025b.eb"))
        .unwrap_or_else(|| panic!("no OpenMPI line:\n{text}"));
    let gromacs = lines
        .iter()
        .position(|l| l.starts_with("GROMACS-2025.0-foss-2025b.eb"))
        .unwrap_or_else(|| panic!("no GROMACS line:\n{text}"));
    assert!(openmpi < gromacs, "OpenMPI builds first:\n{text}");
    assert!(
        lines[openmpi].ends_with(" --hooks=/sw/hooks/mpi_hook.py"),
        "{text}"
    );
    assert!(!text.contains("--from-commit"), "{text}");
    assert!(!text.contains("--ignore-test-failure"), "{text}");
}
