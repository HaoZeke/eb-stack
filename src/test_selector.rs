//! Which build-log lines count as an OpenBLAS test command when a proof
//! round decides whether a build is binary-verified.
//!
//! The proof-easyblock harness picks the test command with a regex (the
//! `select` of its `env_for_command` checks H4, H9 and S1) and then looks
//! for the thread limit on that line or in the lines before it. No
//! selected line means no evidence, and the trial is not binary-verified.
//!
//! Each round keeps the selector it was scored with, so its results can be
//! scored again to the same numbers. Round 6 used [`TestSelector::R6`],
//! which needs `make` and the target one space apart. A test step that
//! passes the build's parallelism to make (`make -j 2 tests`, as r6 A t3's
//! easyblock did) ran the tests, but r6 selected nothing in its log.
//! [`TestSelector::R7`] also takes one job-count option between `make` and
//! the target; any other word there still selects nothing.
//!
//! Both patterns use only syntax that Python's `re` reads the same way, so
//! [`TestSelector::pattern`] can be pasted into an acceptance file.

use regex::Regex;
use std::sync::OnceLock;

/// A versioned selector for the OpenBLAS test commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestSelector {
    /// The selector frozen in openblas-3936's `accept-r3.json`, used
    /// through round 6.
    R6,
    /// Round 7 on: `make tests` or `make lapack-test`, optionally with
    /// one `-j N`, `-jN` or `--jobs=N` (N at least 1) after `make`.
    R7,
}

const R6_PATTERN: &str = r"^\s*.*\bmake (tests|lapack-test)\b";
const R7_PATTERN: &str =
    r"^\s*.*\bmake(?: (?:-j ?[1-9][0-9]*|--jobs=[1-9][0-9]*))? (tests|lapack-test)\b";

impl TestSelector {
    /// The selector a proof round is scored with.
    pub fn for_round(round: u32) -> Self {
        if round <= 6 {
            Self::R6
        } else {
            Self::R7
        }
    }

    /// The regex, as an acceptance file writes it.
    pub fn pattern(self) -> &'static str {
        match self {
            Self::R6 => R6_PATTERN,
            Self::R7 => R7_PATTERN,
        }
    }

    fn regex(self) -> &'static Regex {
        static R6: OnceLock<Regex> = OnceLock::new();
        static R7: OnceLock<Regex> = OnceLock::new();
        let cell = match self {
            Self::R6 => &R6,
            Self::R7 => &R7,
        };
        cell.get_or_init(|| Regex::new(self.pattern()).expect("test selector regex"))
    }

    /// The make target `line` runs (`tests` or `lapack-test`), or `None`
    /// when the line is not a test command for this selector.
    pub fn target(self, line: &str) -> Option<&str> {
        self.regex()
            .captures(line)
            .and_then(|found| found.get(1))
            .map(|target| target.as_str())
    }

    /// Whether `line` is a test command for this selector.
    pub fn is_match(self, line: &str) -> bool {
        self.regex().is_match(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test-step lines from the r6 logs: the trace line of a build and the
    // dry run's command line.
    const PLAIN: &str = "        make tests  BINARY='64'  CC='gcc'  FC='gfortran'  MAKE_NB_JOBS='-1'  USE_OPENMP='1'  USE_THREAD='1'";
    const RECIPE: &str = "  running shell command \"export OMP_NUM_THREADS=1 &&  make tests  BINARY='64'  CC='gcc'\"";
    const A_T3_BUILD: &str = "        make -j 2 tests  BINARY='64'  CC='gcc'  FC='gfortran'  MAKE_NB_JOBS='-1'  USE_OPENMP='1'  USE_THREAD='1'";
    const A_T3_DRY: &str =
        "  running shell command \"make -j 3 tests  BINARY='64'  CC='gcc'  FC='gfortran'\"";

    #[test]
    fn r6_is_the_frozen_pattern_and_misses_a_job_count() {
        assert_eq!(TestSelector::for_round(6), TestSelector::R6);
        assert_eq!(
            TestSelector::R6.pattern(),
            r"^\s*.*\bmake (tests|lapack-test)\b"
        );
        assert!(TestSelector::R6.is_match(PLAIN));
        assert!(TestSelector::R6.is_match(RECIPE));
        assert!(!TestSelector::R6.is_match(A_T3_BUILD));
        assert!(!TestSelector::R6.is_match(A_T3_DRY));
    }

    #[test]
    fn r7_takes_one_job_count_option() {
        assert_eq!(TestSelector::for_round(7), TestSelector::R7);
        for line in [PLAIN, RECIPE, A_T3_BUILD, A_T3_DRY] {
            assert_eq!(TestSelector::R7.target(line), Some("tests"), "{line}");
        }
        for line in [
            "make -j2 tests",
            "make -j 16 tests",
            "make --jobs=4 tests",
            "export OPENBLAS_NUM_THREADS=2 && make -j 2 lapack-test",
        ] {
            assert!(TestSelector::R7.is_match(line), "{line}");
        }
        assert_eq!(
            TestSelector::R7.target("make -j 2 lapack-test"),
            Some("lapack-test")
        );
    }

    #[test]
    fn r7_rejects_anything_else_between_make_and_the_target() {
        for line in [
            "make -j tests",
            "make -j0 tests",
            "make -j 2 -k tests",
            "make -k tests",
            "make --jobs 2 tests",
            "make --jobs= tests",
            "make -j2  tests",
            "make  tests",
            "make all",
            "make -j 2",
            "cmake -j 2 tests",
        ] {
            assert!(!TestSelector::R7.is_match(line), "{line}");
        }
    }

    #[test]
    fn r7_selects_every_line_r6_selects() {
        for line in [PLAIN, RECIPE, "make lapack-test", "\tmake tests"] {
            assert!(TestSelector::R6.is_match(line));
            assert!(TestSelector::R7.is_match(line), "{line}");
        }
    }
}
