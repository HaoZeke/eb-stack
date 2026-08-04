//! Machine-readable scores for the reproduction grind, and the ratchet
//! that keeps them moving one way.
//!
//! The reproduction harness compares an emitted bump against the file a
//! maintainer merged, byte for byte, except for an allowance: the lines
//! a maintainer added by hand that no mechanical bump can invent. The
//! length of that allowance is the miss count for the case, so it is the
//! number the grind reports and the number that has to shrink as the
//! tool improves.
//!
//! Two things follow. The allowance count leaves the test binary as a
//! JSON artifact the scoreboard reads instead of a human transcribing
//! it, and a checked-in ratchet file pins every count, so an allowance
//! that grows fails the suite and an allowance that shrinks fails until
//! the smaller number is recorded.
//!
//! Scoring itself is not repeated here: the ladder and its normalization
//! live in [`crate::miner`], and this module applies the allowance and
//! then asks that code for the rung.

use crate::miner::{compare_reproduction, ReproScore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Reading or writing a score artifact or ratchet file failed.
#[derive(Debug, thiserror::Error)]
pub enum ReproReportError {
    /// The artifact directory or file could not be read or written.
    #[error("repro score io at {path}: {source}")]
    Io {
        /// The path being read or written.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// A score artifact or ratchet file did not parse as its JSON shape.
    #[error("repro score json at {path}: {source}")]
    Json {
        /// The file that failed to parse.
        path: PathBuf,
        /// The underlying parse error.
        source: serde_json::Error,
    },
}

/// An allowance entry that no longer earns its place.
///
/// Both shapes mean the same thing for the ratchet: the count for this
/// case can come down, and the entry should be deleted rather than left
/// to hide a future regression behind an unused exception.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StaleAllowance {
    /// The line is not in the merged target at all, so the allowance
    /// describes a maintainer addition that is no longer there.
    AbsentFromTarget {
        /// The allowance entry, as declared.
        line: String,
    },
    /// The bump emits the line itself now, so nothing needs forgiving.
    AlreadyEmitted {
        /// The allowance entry, as declared.
        line: String,
    },
}

impl StaleAllowance {
    /// The allowance entry this refers to.
    pub fn line(&self) -> &str {
        match self {
            StaleAllowance::AbsentFromTarget { line } => line,
            StaleAllowance::AlreadyEmitted { line } => line,
        }
    }

    /// What to do about it, in one sentence.
    pub fn explain(&self) -> String {
        match self {
            StaleAllowance::AbsentFromTarget { line } => format!(
                "allowance {line:?} is not a line of the merged target; delete it and lower the ratchet"
            ),
            StaleAllowance::AlreadyEmitted { line } => format!(
                "the bump now emits {line:?} itself; delete the allowance and lower the ratchet"
            ),
        }
    }
}

/// One reproduction case, scored.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReproCaseScore {
    /// Case name. Stable across runs, and the key the ratchet is keyed by.
    pub case: String,
    /// The upstream pull request, when the case was mined from one.
    /// `None` for the frozen fixture pairs, whose PR numbers are not
    /// recorded in the repository.
    pub pull_request: Option<u32>,
    /// Source recipe the bump started from.
    pub source: String,
    /// The merged file the emit is scored against.
    pub target: String,
    /// The rung, decided after the allowance is applied.
    pub score: ReproScore,
    /// Lines the harness forgives, as declared by the case.
    pub allowance: Vec<String>,
    /// Allowance entries that no longer earn their place.
    pub stale_allowance: Vec<StaleAllowance>,
    /// How many raw diff lines remain once the allowance is applied.
    /// Zero for `EXACT`; nonzero for `SEMANTIC` says how much comment and
    /// whitespace difference the score forgave.
    pub residual_lines: usize,
}

impl ReproCaseScore {
    /// The miss count for the scoreboard: how many hand-added lines this
    /// case still cannot reproduce.
    pub fn miss_count(&self) -> usize {
        self.allowance.len()
    }
}

/// A scored comparison before it becomes an artifact.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ScoredReproduction {
    /// The rung, from [`crate::miner::score_reproduction`], comparing the
    /// emitted text against the target with the allowance removed.
    pub score: ReproScore,
    /// Allowance entries that no longer earn their place.
    pub stale: Vec<StaleAllowance>,
    /// The raw diff that remains, one `-`/`+` line per difference.
    pub residual: Vec<String>,
    /// The target with each allowance line removed once, in order: the
    /// text the emit is actually compared against.
    pub target_without_allowance: String,
}

/// Apply `allowance` to `target` and score `emitted` against the rest.
///
/// Each allowance line is removed from the target once, at its first
/// occurrence, which is the rule the harness has always used. An entry
/// that is missing from the target, or that the emit now carries itself,
/// comes back as stale rather than silently forgiving nothing.
pub fn score_with_allowance(emitted: &str, target: &str, allowance: &[&str]) -> ScoredReproduction {
    let emitted_lines: Vec<&str> = emitted.lines().collect();
    let mut target_lines: Vec<&str> = target.lines().collect();
    let mut stale = Vec::new();

    for entry in allowance {
        match target_lines.iter().position(|line| line == entry) {
            Some(index) => {
                target_lines.remove(index);
                if emitted_lines.contains(entry) {
                    stale.push(StaleAllowance::AlreadyEmitted {
                        line: (*entry).to_string(),
                    });
                }
            }
            None => stale.push(StaleAllowance::AbsentFromTarget {
                line: (*entry).to_string(),
            }),
        }
    }

    // Keep the target's final line terminator. Rebuilding the text from
    // lines drops it, and a target that ends in a newline would then
    // never compare equal to an emit that also does, putting every case
    // one rung below what it earned.
    let mut stripped = target_lines.join("\n");
    if target.ends_with('\n') {
        stripped.push('\n');
    }
    let comparison = compare_reproduction(emitted, &stripped);
    ScoredReproduction {
        score: comparison.score,
        stale,
        residual: comparison
            .render_raw_diff()
            .lines()
            .map(str::to_string)
            .collect(),
        target_without_allowance: stripped,
    }
}

/// Write one case score as `<dir>/<case>.json`, creating `dir`.
///
/// One file per case rather than one shared file: the harness writes
/// these from tests that run in parallel, and a per-case path needs no
/// lock and no merge.
pub fn write_case_score(dir: &Path, score: &ReproCaseScore) -> Result<PathBuf, ReproReportError> {
    std::fs::create_dir_all(dir).map_err(|source| ReproReportError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let path = dir.join(format!("{}.json", score.case));
    let text = serde_json::to_string_pretty(score).map_err(|source| ReproReportError::Json {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, format!("{text}\n")).map_err(|source| ReproReportError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Read every case score in `dir`, ordered by case name.
pub fn read_case_scores(dir: &Path) -> Result<Vec<ReproCaseScore>, ReproReportError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ReproReportError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut scores = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|source| ReproReportError::Io {
                path: dir.to_path_buf(),
                source,
            })?
            .path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ReproReportError::Io {
            path: path.clone(),
            source,
        })?;
        let score: ReproCaseScore =
            serde_json::from_str(&text).map_err(|source| ReproReportError::Json {
                path: path.clone(),
                source,
            })?;
        scores.push(score);
    }
    scores.sort_by(|left, right| left.case.cmp(&right.case));
    Ok(scores)
}

/// The committed allowance counts: how many hand-added lines each case is
/// still permitted to miss.
///
/// This file only ever moves down. Its diff is the record of every
/// allowance change the grind has made, which is why the counts live
/// here rather than only in test source.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReproRatchet {
    /// Why the file exists and which way it moves, for whoever opens it
    /// first.
    pub note: String,
    /// Allowance count per case name.
    pub cases: BTreeMap<String, usize>,
    /// Sum over `cases`, so a single number tracks the whole grind and a
    /// hand edit that forgets to update it is caught.
    pub total: usize,
}

impl ReproRatchet {
    /// Parse a ratchet file.
    pub fn from_json(text: &str, path: &Path) -> Result<Self, ReproReportError> {
        serde_json::from_str(text).map_err(|source| ReproReportError::Json {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Read a ratchet file from disk.
    pub fn read(path: &Path) -> Result<Self, ReproReportError> {
        let text = std::fs::read_to_string(path).map_err(|source| ReproReportError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&text, path)
    }

    /// The recorded count for `case`, if it has one.
    pub fn allowance_for(&self, case: &str) -> Option<usize> {
        self.cases.get(case).copied()
    }

    /// `total` against the sum of the per-case counts.
    pub fn total_violation(&self) -> Option<RatchetViolation> {
        let sum: usize = self.cases.values().sum();
        (sum != self.total).then_some(RatchetViolation::TotalMismatch {
            recorded: self.total,
            summed: sum,
        })
    }
}

/// A way the live suite and the committed ratchet disagree.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RatchetViolation {
    /// The case forgives more lines than the ratchet records.
    Grew {
        /// Case name.
        case: String,
        /// Allowance the case declared.
        declared: usize,
        /// Count the ratchet records.
        recorded: usize,
    },
    /// The case forgives fewer lines than the ratchet records, which is
    /// progress that has to be written down before the suite goes green
    /// again -- otherwise the recorded number drifts upward of reality
    /// and stops catching the next regression.
    Improved {
        /// Case name.
        case: String,
        /// Allowance the case declared.
        declared: usize,
        /// Count the ratchet records.
        recorded: usize,
    },
    /// The case is not in the ratchet at all.
    Unrecorded {
        /// Case name.
        case: String,
        /// Allowance the case declared.
        declared: usize,
    },
    /// An allowance entry that forgives nothing.
    Stale {
        /// Case name.
        case: String,
        /// What is stale and what to do about it.
        detail: String,
    },
    /// `total` disagrees with the sum of the per-case counts.
    TotalMismatch {
        /// The `total` field as committed.
        recorded: usize,
        /// The sum of the per-case counts.
        summed: usize,
    },
}

impl std::fmt::Display for RatchetViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RatchetViolation::Grew {
                case,
                declared,
                recorded,
            } => write!(
                f,
                "{case}: allowance grew to {declared}, ratchet records {recorded}; \
                 reproduce the line instead, or record the regression deliberately"
            ),
            RatchetViolation::Improved {
                case,
                declared,
                recorded,
            } => write!(
                f,
                "{case}: allowance is down to {declared}, ratchet still records {recorded}; \
                 lower it, so the next regression is caught against the new number"
            ),
            RatchetViolation::Unrecorded { case, declared } => write!(
                f,
                "{case}: allowance {declared} is not in the ratchet; add the case"
            ),
            RatchetViolation::Stale { case, detail } => write!(f, "{case}: {detail}"),
            RatchetViolation::TotalMismatch { recorded, summed } => write!(
                f,
                "ratchet total records {recorded}, per-case counts sum to {summed}"
            ),
        }
    }
}

/// Check one scored case against the committed ratchet.
pub fn check_case_against_ratchet(
    ratchet: &ReproRatchet,
    score: &ReproCaseScore,
) -> Vec<RatchetViolation> {
    let mut violations = Vec::new();
    let declared = score.miss_count();
    match ratchet.allowance_for(&score.case) {
        Some(recorded) if declared > recorded => violations.push(RatchetViolation::Grew {
            case: score.case.clone(),
            declared,
            recorded,
        }),
        Some(recorded) if declared < recorded => violations.push(RatchetViolation::Improved {
            case: score.case.clone(),
            declared,
            recorded,
        }),
        Some(_) => {}
        None => violations.push(RatchetViolation::Unrecorded {
            case: score.case.clone(),
            declared,
        }),
    }
    for stale in &score.stale_allowance {
        violations.push(RatchetViolation::Stale {
            case: score.case.clone(),
            detail: stale.explain(),
        });
    }
    violations
}

/// Check a whole collected run: every case, plus the ratchet's own total.
pub fn check_ratchet(ratchet: &ReproRatchet, scores: &[ReproCaseScore]) -> Vec<RatchetViolation> {
    let mut violations: Vec<RatchetViolation> = scores
        .iter()
        .flat_map(|score| check_case_against_ratchet(ratchet, score))
        .collect();
    violations.extend(ratchet.total_violation());
    violations
}

/// Render collected scores as the scoreboard's own table shape.
///
/// The grind pastes this under a scored entry rather than transcribing
/// numbers out of test output by hand.
pub fn render_scoreboard_table(scores: &[ReproCaseScore]) -> String {
    let mut out =
        String::from("| Case | PR | Score | Allowance | Residual |\n|---+---+---+---+---|\n");
    for score in scores {
        let pr = score
            .pull_request
            .map(|number| format!("#{number}"))
            .unwrap_or_else(|| "-".to_string());
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            score.case,
            pr,
            score.score,
            score.miss_count(),
            score.residual_lines
        ));
    }
    let exact = scores
        .iter()
        .filter(|score| score.score == ReproScore::Exact)
        .count();
    let semantic = scores
        .iter()
        .filter(|score| score.score == ReproScore::Semantic)
        .count();
    let material = scores
        .iter()
        .filter(|score| score.score == ReproScore::Material)
        .count();
    let allowance: usize = scores.iter().map(ReproCaseScore::miss_count).sum();
    out.push_str(&format!(
        "\n{} cases: {exact} EXACT, {semantic} SEMANTIC, {material} MATERIAL; \
         allowance total {allowance}\n",
        scores.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one allowance the frozen fixture matrix still carries: the
    /// `pybind11` dependency the maintainer hand-added to the GROMACS
    /// 2024.4 foss/2024a recipe, which no mechanical bump can invent.
    const PYBIND11: &str = "    ('pybind11', '2.12.0'),";

    fn target_with_pybind11() -> String {
        format!("dependencies = [\n    ('Python', '3.12.3'),\n{PYBIND11}\n]")
    }

    fn emitted_without_pybind11() -> String {
        "dependencies = [\n    ('Python', '3.12.3'),\n]".to_string()
    }

    #[test]
    fn a_file_that_ends_in_a_newline_can_still_score_exact() {
        // Both texts end in a newline, as real easyconfigs do. Rebuilding
        // the target from its lines without putting the terminator back
        // makes every case SEMANTIC, and a suite reproducing files byte
        // for byte then reports nothing on the top rung.
        let target = format!("{}\n", target_with_pybind11());
        let emitted = format!("{}\n", emitted_without_pybind11());
        let scored = score_with_allowance(&emitted, &target, &[PYBIND11]);
        assert_eq!(scored.score, ReproScore::Exact);
        assert!(scored.target_without_allowance.ends_with('\n'));
    }

    #[test]
    fn a_target_without_a_trailing_newline_does_not_gain_one() {
        let scored = score_with_allowance(
            &emitted_without_pybind11(),
            &target_with_pybind11(),
            &[PYBIND11],
        );
        assert!(!scored.target_without_allowance.ends_with('\n'));
    }

    #[test]
    fn an_allowance_that_forgives_a_real_addition_scores_exact() {
        let scored = score_with_allowance(
            &emitted_without_pybind11(),
            &target_with_pybind11(),
            &[PYBIND11],
        );
        assert_eq!(scored.score, ReproScore::Exact);
        assert!(
            scored.stale.is_empty(),
            "the allowance forgave a line the target really carries: {:?}",
            scored.stale
        );
        assert!(scored.residual.is_empty());
    }

    #[test]
    fn an_allowance_the_bump_now_emits_itself_is_stale() {
        let emitted = target_with_pybind11();
        let scored = score_with_allowance(&emitted, &target_with_pybind11(), &[PYBIND11]);
        assert_eq!(
            scored.stale,
            vec![StaleAllowance::AlreadyEmitted {
                line: PYBIND11.to_string()
            }],
            "an allowance for a line the tool learned to emit hides the next regression behind \
             an exception that forgives nothing"
        );
    }

    #[test]
    fn an_allowance_absent_from_the_target_is_stale() {
        let scored = score_with_allowance(
            &emitted_without_pybind11(),
            &emitted_without_pybind11(),
            &[PYBIND11],
        );
        assert_eq!(
            scored.stale,
            vec![StaleAllowance::AbsentFromTarget {
                line: PYBIND11.to_string()
            }]
        );
    }

    #[test]
    fn the_allowance_does_not_forgive_a_second_missing_line() {
        let target = format!("{}\n{PYBIND11}", target_with_pybind11());
        let scored = score_with_allowance(&emitted_without_pybind11(), &target, &[PYBIND11]);
        assert_eq!(
            scored.score,
            ReproScore::Material,
            "one allowance entry forgives one line; a second copy the emit lacks is a real miss"
        );
    }

    #[test]
    fn the_allowance_composes_with_the_normalized_ladder() {
        // Same allowance, plus a difference the ladder forgives on its own.
        let emitted = format!("# a dead comment\n{}", emitted_without_pybind11());
        let scored = score_with_allowance(&emitted, &target_with_pybind11(), &[PYBIND11]);
        assert_eq!(
            scored.score,
            ReproScore::Semantic,
            "after the allowance, what is left is a comment, and the ladder already knows what \
             that is worth"
        );
        assert!(
            scored
                .residual
                .iter()
                .any(|line| line.contains("a dead comment")),
            "the residual keeps the raw difference for display, got {:?}",
            scored.residual
        );
    }

    fn ratchet_with(case: &str, count: usize) -> ReproRatchet {
        let mut cases = BTreeMap::new();
        cases.insert(case.to_string(), count);
        ReproRatchet {
            note: "test".into(),
            cases,
            total: count,
        }
    }

    fn score_with(case: &str, allowance: Vec<String>) -> ReproCaseScore {
        ReproCaseScore {
            case: case.to_string(),
            pull_request: None,
            source: "source.eb".into(),
            target: "target.eb".into(),
            score: ReproScore::Exact,
            allowance,
            stale_allowance: Vec::new(),
            residual_lines: 0,
        }
    }

    #[test]
    fn a_grown_allowance_is_a_violation() {
        let ratchet = ratchet_with("gromacs", 1);
        let score = score_with(
            "gromacs",
            vec![PYBIND11.into(), "    ('extra', '1.0'),".into()],
        );
        assert_eq!(
            check_case_against_ratchet(&ratchet, &score),
            vec![RatchetViolation::Grew {
                case: "gromacs".into(),
                declared: 2,
                recorded: 1,
            }]
        );
    }

    #[test]
    fn an_unrecorded_improvement_is_a_violation_too() {
        let ratchet = ratchet_with("gromacs", 1);
        let score = score_with("gromacs", Vec::new());
        assert_eq!(
            check_case_against_ratchet(&ratchet, &score),
            vec![RatchetViolation::Improved {
                case: "gromacs".into(),
                declared: 0,
                recorded: 1,
            }],
            "a ratchet that keeps a number reality has left behind stops catching regressions"
        );
    }

    #[test]
    fn a_case_the_ratchet_never_heard_of_is_a_violation() {
        let ratchet = ratchet_with("gromacs", 1);
        let score = score_with("scafacos", Vec::new());
        assert_eq!(
            check_case_against_ratchet(&ratchet, &score),
            vec![RatchetViolation::Unrecorded {
                case: "scafacos".into(),
                declared: 0,
            }]
        );
    }

    #[test]
    fn a_matching_allowance_passes() {
        let ratchet = ratchet_with("gromacs", 1);
        let score = score_with("gromacs", vec![PYBIND11.into()]);
        assert!(check_case_against_ratchet(&ratchet, &score).is_empty());
        assert!(check_ratchet(&ratchet, &[score]).is_empty());
    }

    #[test]
    fn a_total_that_disagrees_with_the_cases_is_a_violation() {
        let mut ratchet = ratchet_with("gromacs", 1);
        ratchet.total = 4;
        assert_eq!(
            ratchet.total_violation(),
            Some(RatchetViolation::TotalMismatch {
                recorded: 4,
                summed: 1,
            })
        );
    }

    #[test]
    fn a_case_score_round_trips_through_its_artifact() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut score = score_with("gromacs", vec![PYBIND11.into()]);
        score.pull_request = Some(25009);
        score.score = ReproScore::Semantic;
        let path = write_case_score(directory.path(), &score).expect("write artifact");
        assert_eq!(path.file_name().unwrap(), "gromacs.json");
        let read = read_case_scores(directory.path()).expect("read artifacts");
        assert_eq!(read, vec![score]);
    }

    #[test]
    fn the_scoreboard_table_carries_score_and_allowance_per_case() {
        let scores = vec![
            score_with("gromacs", vec![PYBIND11.into()]),
            score_with("scafacos", Vec::new()),
        ];
        let table = render_scoreboard_table(&scores);
        assert!(table.contains("| gromacs | - | EXACT | 1 | 0 |"), "{table}");
        assert!(
            table.contains("| scafacos | - | EXACT | 0 | 0 |"),
            "{table}"
        );
        assert!(table.contains("2 cases: 2 EXACT"), "{table}");
        assert!(table.contains("allowance total 1"), "{table}");
    }
}
