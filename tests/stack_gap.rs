//! `stack gap` routes a site listing and writes one TSV per route.

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn routes_are_written_one_file_per_set() {
    let out = tempfile::tempdir().expect("temp");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "stack",
            "gap",
            "--site",
            "tests/gap_routes/site.txt",
            "--installed",
            "tests/gap_routes/installed.txt",
            "--easyconfigs",
            "tests/gap_routes/easyconfigs",
            "--generation",
            "2026.1",
            "--exclude-file",
            "tests/gap_routes/site-only.tsv",
            "--filter-deps",
            "zlib",
            "--open-layer",
            "tests/gap_routes/open-layer.json",
            "--open-easyconfigs",
            "tests/gap_routes/open-easyconfigs.json",
            "--out-dir",
        ])
        .arg(out.path())
        .output()
        .expect("stack gap");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "site=9 in-stack=1 filtered=1 open-layer=1 recipe=1 open-easyconfigs=1 no-recipe=3 excluded=1"
        ),
        "{stdout}"
    );
    let gap = std::fs::read_to_string(out.path().join("gap.tsv")).unwrap();
    assert!(
        gap.contains("LAMMPS\topen-layer\tLAMMPS-2025.0-foss-2026.1.eb\t-\tsoftware-layer#1787\n"),
        "{gap}"
    );
    assert!(
        gap.contains(
            "QuantumESPRESSO\topen-easyconfigs\tQuantumESPRESSO-7.4-foss-2026.1.eb\t-\teasyconfigs#25001\n"
        ),
        "{gap}"
    );
    assert!(
        gap.contains("BLAST+\trecipe\tBLAST+-2.17.0-gompi-2026.1.eb\t-\t-\n"),
        "{gap}"
    );
    assert!(
        gap.contains("CP2K\tno-recipe\t-\tCP2K-2025.2-foss-2025b.eb\t-\n"),
        "{gap}"
    );
    assert!(gap.contains("OldTool\tno-recipe\t-\t-\t-\n"), "{gap}");
    assert!(gap.contains("Julia\tno-recipe\t-\t-\t-\n"), "{gap}");
    assert!(gap.contains("MATLAB\texcluded\t-\t-\tlicensed\n"), "{gap}");
    assert!(
        gap.contains("zlib\tfiltered\t-\t-\tfiltered to the compatibility layer\n"),
        "{gap}"
    );
    assert!(gap.contains("FFTW\tin-stack\t-\t-\t-\n"), "{gap}");
    let layer = std::fs::read_to_string(out.path().join("open-layer.tsv")).unwrap();
    assert!(layer.contains("LAMMPS\t"), "{layer}");
    assert!(!layer.contains("BLAST+"), "{layer}");
    let bumps = std::fs::read_to_string(out.path().join("bump.tsv")).unwrap();
    assert!(bumps.contains("CP2K\t"), "{bumps}");
    assert!(bumps.contains("Julia\t"), "{bumps}");
    assert!(bumps.contains("OldTool\t"), "{bumps}");
    let roots = std::fs::read_to_string(out.path().join("roots.txt")).unwrap();
    assert_eq!(roots, "BLAST+-2.17.0-gompi-2026.1.eb\n");
    let sources = std::fs::read_to_string(out.path().join("bump-sources.txt")).unwrap();
    assert_eq!(sources, "CP2K-2025.2-foss-2025b.eb\n");
    for name in [
        "in-stack.tsv",
        "site-only.tsv",
        "filtered.tsv",
        "recipe.tsv",
        "open-easyconfigs.tsv",
    ] {
        let text = std::fs::read_to_string(out.path().join(name)).unwrap();
        assert!(text.starts_with("name\troute\t"), "{name}: {text}");
    }
}

#[test]
fn route_fixture_has_one_row_for_every_route() {
    let out = tempfile::tempdir().expect("temp");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "stack",
            "gap",
            "--site",
            "tests/gap_routes/one-per-route.txt",
            "--installed",
            "tests/gap_routes/installed.txt",
            "--easyconfigs",
            "tests/gap_routes/easyconfigs",
            "--generation",
            "2026.1",
            "--exclude-file",
            "tests/gap_routes/site-only.tsv",
            "--filter-deps",
            "zlib",
            "--open-layer",
            "tests/gap_routes/open-layer.json",
            "--open-easyconfigs",
            "tests/gap_routes/open-easyconfigs.json",
            "--out-dir",
        ])
        .arg(out.path())
        .output()
        .expect("stack gap");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "site=7 in-stack=1 filtered=1 open-layer=1 recipe=1 open-easyconfigs=1 no-recipe=1 excluded=1"
        ),
        "{stdout}"
    );
    for name in [
        "in-stack.tsv",
        "site-only.tsv",
        "filtered.tsv",
        "open-layer.tsv",
        "recipe.tsv",
        "open-easyconfigs.tsv",
        "bump.tsv",
    ] {
        let text = std::fs::read_to_string(out.path().join(name)).unwrap();
        assert_eq!(text.lines().count(), 2, "{name}:\n{text}");
    }
}

#[test]
fn live_module_trees_union_the_site_an_older_eessi_and_both_target_dirs() {
    let out = tempfile::tempdir().expect("temp");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-stack"))
        .current_dir(root())
        .env_remove("EASYBUILD_FILTER_DEPS")
        .args([
            "stack",
            "gap",
            "--site",
            "tests/gap_live/site/modules",
            "--site",
            "tests/gap_live/avail.txt",
            "--older",
            "tests/gap_live/older/modules",
            "--installed",
            "tests/gap_live/target-cpu/modules",
            "--installed",
            "tests/gap_live/target-cc90/modules",
            "--easyconfigs",
            "tests/gap_routes/easyconfigs",
            "--generation",
            "2026.1",
            "--eessi-version",
            "2026.06",
            "--exclude-file",
            "tests/gap_live/exclude.tsv",
            "--filter-deps",
            "binutils",
            "--open-layer",
            "tests/gap_live/open-layer.json",
            "--out-dir",
        ])
        .arg(out.path())
        .output()
        .expect("stack gap");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "site=7 in-stack=2 filtered=1 open-layer=1 recipe=0 open-easyconfigs=0 no-recipe=1 excluded=2"
        ),
        "{stdout}"
    );
    let gap = std::fs::read_to_string(out.path().join("gap.tsv")).unwrap();
    assert!(gap.contains("CUDA\tin-stack\t"), "{gap}");
    assert!(gap.contains("FFTW\tin-stack\t"), "{gap}");
    assert!(gap.contains("MATLAB\texcluded\t-\t-\tlicensed\n"), "{gap}");
    assert!(
        gap.contains("code-server\texcluded\t-\t-\tportal\n"),
        "{gap}"
    );
    assert!(
        gap.contains("binutils\tfiltered\t-\t-\tfiltered to the compatibility layer\n"),
        "{gap}"
    );
    assert!(
        gap.contains("LAMMPS\topen-layer\tLAMMPS-2025.0-foss-2026.1.eb\t-\tsoftware-layer#1700\n"),
        "{gap}"
    );
    assert!(gap.contains("QuantumESPRESSO\tno-recipe\t"), "{gap}");
    assert!(!gap.contains("software-layer#1600"), "{gap}");
    let changes = std::fs::read_to_string(out.path().join("version-change.tsv")).unwrap();
    assert!(
        changes.starts_with("name\told_version\teessi_version\tstatus\n"),
        "{changes}"
    );
    assert!(
        changes.contains("CUDA\t12.6.0\t12.6.0\tmoved\n"),
        "{changes}"
    );
    assert!(
        changes.contains("FFTW\t3.3.10\t3.3.10\tmoved\n"),
        "{changes}"
    );
    assert!(
        changes.contains("LAMMPS\t2023.0\t-\tgone\n")
            && changes.contains("LAMMPS\t2024.0\t-\tgone\n"),
        "{changes}"
    );
    assert!(
        changes.contains("MATLAB\t2024b\t-\tsite-only\n")
            && changes.contains("code-server\t4.96.2\t-\tsite-only\n"),
        "{changes}"
    );
}
