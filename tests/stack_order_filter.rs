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

#[test]
fn installed_modules_are_leaves_and_filter_deps_still_drops_the_rest() {
    let plain = order(&[], None);
    let plain_names = names(&plain);
    assert!(
        plain_names.iter().any(|line| line.starts_with("OpenBLAS-")),
        "{plain}"
    );
    assert!(
        plain_names.iter().any(|line| line.starts_with("FFTW-")),
        "{plain}"
    );

    let installed = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/stack_order_installed/installed.txt");
    let listed = order(
        &[
            "--installed",
            installed.to_str().expect("utf8"),
            "--filter-deps",
            "FFTW",
        ],
        None,
    );
    let listed_names = names(&listed);
    assert!(
        listed_names
            .iter()
            .all(|line| !line.starts_with("OpenBLAS-")),
        "an installed OpenBLAS is a leaf:\n{listed}"
    );
    assert!(
        listed_names.iter().all(|line| !line.starts_with("FFTW-")),
        "filter-deps still drops FFTW when an installed list is set:\n{listed}"
    );
    assert!(
        listed_names.iter().any(|line| line.starts_with("GROMACS-")),
        "the root stays:\n{listed}"
    );
    assert!(
        listed_names.len() < plain_names.len(),
        "plain {plain_names:?}\nlisted {listed_names:?}"
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("order.txt");
    let ran = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "order", "--easyconfigs"])
        .arg(easyconfigs())
        .args(["--root", "GROMACS", "--installed"])
        .arg(&installed)
        .args(["--filter-deps", "FFTW", "--out"])
        .arg(&out)
        .env_remove("EASYBUILD_FILTER_DEPS")
        .output()
        .expect("run");
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    let stdout = String::from_utf8_lossy(&ran.stdout);
    assert!(
        stdout.contains("installed=1\n"),
        "OpenBLAS is the one leaf; FFTW was filtered:\n{stdout}"
    );

    let modules = tmp.path().join("modules/all/OpenBLAS");
    std::fs::create_dir_all(&modules).expect("modules");
    std::fs::write(modules.join("0.3.27-foss-2025b.lua"), "-- module\n").expect("lua");
    let from_tree = order(
        &[
            "--installed",
            tmp.path().join("modules").to_str().expect("utf8"),
        ],
        None,
    );
    assert!(
        names(&from_tree)
            .iter()
            .all(|line| !line.starts_with("OpenBLAS-")),
        "an Lmod modules/all tree is the same list:\n{from_tree}"
    );
    assert!(
        names(&from_tree)
            .iter()
            .any(|line| line.starts_with("FFTW-")),
        "the tree did not filter FFTW:\n{from_tree}"
    );

    let bare = tmp.path().join("bare.txt");
    std::fs::write(&bare, "OpenBLAS\n").expect("bare");
    let kept = order(&["--installed", bare.to_str().expect("utf8")], None);
    assert!(
        names(&kept)
            .iter()
            .any(|line| line.starts_with("OpenBLAS-")),
        "a bare name is not every version:\n{kept}"
    );

    let missing = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .args(["stack", "order", "--easyconfigs"])
        .arg(easyconfigs())
        .args([
            "--root",
            "GROMACS",
            "--installed",
            "/tmp/no-such-installed-list-es6.txt",
        ])
        .env_remove("EASYBUILD_FILTER_DEPS")
        .output()
        .expect("run");
    assert!(!missing.status.success());
    let err = String::from_utf8_lossy(&missing.stderr);
    assert!(
        err.contains("--installed") && err.contains("no-such-installed-list-es6.txt"),
        "{err}"
    );
}
