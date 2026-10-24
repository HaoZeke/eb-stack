//! `stack order` drops the same dependencies `stack solve` drops.

use std::path::PathBuf;
use std::process::Command;

fn easyconfigs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/gromacs_2025_to_next/easyconfigs")
}

fn order(extra: &[&str], env_filter: Option<&str>) -> String {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("order.txt");
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
    command
        .args(["stack", "order", "--easyconfigs"])
        .arg(easyconfigs())
        .args(["--root", "GROMACS", "--out"])
        .arg(&out)
        .args(extra)
        .env_remove("EASYBUILD_FILTER_DEPS");
    if let Some(filter) = env_filter {
        command.env("EASYBUILD_FILTER_DEPS", filter);
    }
    let status = command.output().expect("run stack order");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    std::fs::read_to_string(&out).expect("order file")
}

fn names(list: &str) -> Vec<String> {
    list.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            std::path::Path::new(line)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(line)
                .to_string()
        })
        .collect()
}

#[test]
fn filter_deps_flag_and_env_drop_the_same_builds_solve_drops() {
    let plain = order(&[], None);
    let plain_names = names(&plain);
    assert!(
        plain_names.iter().any(|line| line.starts_with("OpenBLAS-")),
        "unfiltered order should build OpenBLAS:\n{plain}"
    );
    assert!(
        plain_names.iter().any(|line| line.starts_with("FFTW-")),
        "unfiltered order should build FFTW:\n{plain}"
    );

    let flagged = order(&["--filter-deps", "OpenBLAS"], None);
    let flagged_names = names(&flagged);
    assert!(
        flagged_names
            .iter()
            .all(|line| !line.starts_with("OpenBLAS-")),
        "--filter-deps OpenBLAS should leave it out:\n{flagged}"
    );
    assert!(
        flagged_names.iter().any(|line| line.starts_with("FFTW-")),
        "a filter for OpenBLAS should keep FFTW:\n{flagged}"
    );
    assert!(
        flagged_names.len() < plain_names.len(),
        "filtering should shorten the order:\nplain {plain_names:?}\nflagged {flagged_names:?}"
    );

    let from_env = order(&[], Some("FFTW"));
    let env_names = names(&from_env);
    assert!(
        env_names.iter().all(|line| !line.starts_with("FFTW-")),
        "EASYBUILD_FILTER_DEPS=FFTW should leave FFTW out:\n{from_env}"
    );
    assert!(
        env_names.iter().any(|line| line.starts_with("OpenBLAS-")),
        "the environment filter for FFTW should keep OpenBLAS:\n{from_env}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let build_list = tmp.path().join("build.list");
    let solved = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "solve", "--easyconfigs"])
        .arg(easyconfigs())
        .args([
            "--toolchain",
            "foss/2025b",
            "--root",
            "GROMACS",
            "--filter-deps",
            "OpenBLAS",
            "--lock-out",
        ])
        .arg(tmp.path().join("stack.lock.json"))
        .args(["--build-list-out"])
        .arg(&build_list)
        .env_remove("EASYBUILD_FILTER_DEPS")
        .output()
        .expect("run stack solve");
    assert!(
        solved.status.success(),
        "{}",
        String::from_utf8_lossy(&solved.stderr)
    );
    let solved_names = names(&std::fs::read_to_string(&build_list).expect("build list"));
    assert!(
        solved_names
            .iter()
            .all(|line| !line.starts_with("OpenBLAS-")),
        "stack solve should drop OpenBLAS too:\n{solved_names:?}"
    );
    assert!(
        solved_names.iter().any(|line| line.starts_with("GROMACS-")),
        "the root stays in the solved build list:\n{solved_names:?}"
    );
}
