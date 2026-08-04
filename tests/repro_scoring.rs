//! The normalization behind `SEMANTIC` has to be build-preserving.
//!
//! `normalize_for_scoring` decides which reproduction differences are
//! forgiven, so a bug in it does not fail loudly: it makes two files that
//! build different things score as equivalent. The check that catches
//! that is the parser itself — normalizing a recipe must not change what
//! EasyBuild would read out of it — run over every real easyconfig
//! reachable from here: the checked-in reproduction fixtures always, and
//! the robot tree when the host has one.

use eb_stack::{normalize_for_scoring, resolve_easyconfig_str, score_reproduction, ReproScore};
use std::path::{Path, PathBuf};

mod common;

/// Every `.eb` file under `root`, in a stable order.
fn easyconfigs_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("eb") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Assert that normalizing every recipe in `files` leaves the resolved
/// easyconfig identical, and report how many were actually checked.
fn assert_normalization_preserves_meaning(files: &[PathBuf]) -> usize {
    let mut checked = 0usize;
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        // A recipe the parser cannot read in the first place says nothing
        // about normalization; the parser's own suites cover those.
        let Ok(original) = resolve_easyconfig_str(&text) else {
            continue;
        };
        let normalized_text = normalize_for_scoring(&text);
        let normalized = resolve_easyconfig_str(&normalized_text).unwrap_or_else(|error| {
            panic!(
                "normalization broke {}: the original parsed but the normalized text did not ({error})",
                path.display()
            )
        });
        assert_eq!(
            normalized,
            original,
            "normalizing {} changed what the recipe resolves to, so a SEMANTIC score over it \
             would forgive a real build difference",
            path.display()
        );
        checked += 1;
    }
    checked
}

#[test]
fn normalization_preserves_every_checked_in_reproduction_fixture() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/repro_fixtures");
    let files = easyconfigs_under(&root);
    assert!(
        files.len() > 20,
        "expected the reproduction fixture corpus, found {} files under {}",
        files.len(),
        root.display()
    );
    let checked = assert_normalization_preserves_meaning(&files);
    assert!(
        checked > 20,
        "only {checked} of {} fixtures parsed, which is too few to establish anything",
        files.len()
    );
}

#[test]
fn normalization_preserves_meaning_across_the_robot_tree() {
    let Some(root) =
        common::require_easyconfigs_tree("normalization_preserves_meaning_across_the_robot_tree")
    else {
        return;
    };
    let files = easyconfigs_under(&root);
    let checked = assert_normalization_preserves_meaning(&files);
    assert!(
        checked > 100,
        "only {checked} recipes of {} in {} parsed; a tree this thin does not exercise the \
         normalizer against real string content",
        files.len(),
        root.display()
    );
    eprintln!(
        "normalization preserved {checked} real easyconfigs under {}",
        root.display()
    );
}

#[test]
fn a_recipe_scores_exact_against_itself_across_the_fixture_corpus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/repro_fixtures");
    for path in easyconfigs_under(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        assert_eq!(
            score_reproduction(&text, &text),
            ReproScore::Exact,
            "{} must score EXACT against itself",
            path.display()
        );
    }
}

/// The scoreboard's consumer: `eb-stack repro summary` over the artifacts a
/// collected run wrote.
mod summary_cli {
    use eb_stack::{write_case_score, ReproCaseScore, ReproScore};
    use std::path::Path;
    use std::process::Command;

    fn score(case: &str, allowance: Vec<String>, score: ReproScore) -> ReproCaseScore {
        ReproCaseScore {
            case: case.to_string(),
            pull_request: None,
            source: "source.eb".into(),
            target: "target.eb".into(),
            score,
            allowance,
            stale_allowance: Vec::new(),
            residual_lines: 0,
        }
    }

    fn seed(dir: &Path) {
        write_case_score(
            dir,
            &score(
                "reproduces_gromacs",
                vec!["    ('pybind11', '2.12.0'),".into()],
                ReproScore::Exact,
            ),
        )
        .expect("write gromacs artifact");
        write_case_score(
            dir,
            &score("reproduces_scafacos", Vec::new(), ReproScore::Semantic),
        )
        .expect("write scafacos artifact");
    }

    /// Written outside the score directory on purpose: every `.json` in
    /// there is read as a case artifact, and a file that is not one is an
    /// error rather than something to skip past.
    fn ratchet_file(dir: &Path, gromacs: usize, total: usize) -> std::path::PathBuf {
        let path = dir.join("ratchet.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"note":"test","cases":{{"reproduces_gromacs":{gromacs},"reproduces_scafacos":0}},"total":{total}}}"#
            ),
        )
        .expect("write ratchet");
        path
    }

    fn summary(scores: &Path, ratchet: Option<&Path>) -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_eb-stack"));
        command.args(["repro", "summary", "--scores", scores.to_str().unwrap()]);
        if let Some(path) = ratchet {
            command.args(["--ratchet", path.to_str().unwrap()]);
        }
        command.output().expect("run eb-stack repro summary")
    }

    #[test]
    fn the_summary_prints_the_scoreboard_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path());
        let output = summary(dir.path(), None);
        let text = String::from_utf8(output.stdout).expect("utf8 stdout");
        assert!(output.status.success(), "{text}");
        assert!(
            text.contains("| reproduces_gromacs | - | EXACT | 1 | 0 |"),
            "{text}"
        );
        assert!(
            text.contains("| reproduces_scafacos | - | SEMANTIC | 0 | 0 |"),
            "{text}"
        );
        assert!(text.contains("allowance total 1"), "{text}");
    }

    #[test]
    fn the_summary_holds_a_run_to_the_committed_ratchet() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path());
        let elsewhere = tempfile::tempdir().expect("ratchet tempdir");
        let matching = ratchet_file(elsewhere.path(), 1, 1);
        let output = summary(dir.path(), Some(&matching));
        let text = String::from_utf8(output.stdout).expect("utf8 stdout");
        assert!(output.status.success(), "{text}");
        assert!(text.contains("ratchet: 2 cases hold at 1"), "{text}");
    }

    #[test]
    fn the_summary_fails_when_a_run_exceeds_the_ratchet() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path());
        let elsewhere = tempfile::tempdir().expect("ratchet tempdir");
        let tightened = ratchet_file(elsewhere.path(), 0, 0);
        let output = summary(dir.path(), Some(&tightened));
        let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
        assert!(
            !output.status.success(),
            "a run that forgives more than the ratchet records must fail the command, not just \
             print about it"
        );
        assert!(
            stderr.contains("allowance grew to 1, ratchet records 0"),
            "{stderr}"
        );
    }

    #[test]
    fn an_empty_score_directory_is_an_error_not_an_empty_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let output = summary(dir.path(), None);
        let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
        assert!(
            !output.status.success(),
            "an empty directory means the suite did not run with EB_REPRO_SCORES set, and a \
             zero-row table would read as a clean run"
        );
        assert!(stderr.contains("EB_REPRO_SCORES"), "{stderr}");
    }
}
