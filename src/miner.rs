//! Ring-corpus miner helpers for the reproduction grind (surf-notes
//! `orchestration/2026-08-03-ebstack-reproduction-grind.md`, tracker issues
//! `ebstack-l8e9`/`ebstack-y5yq`).
//!
//! A mechanical miner that turns a merged upstream PR into a `bump`
//! reproduction fixture pair has to reject two real shapes found while
//! hand-mining ~10 merged easybuilders/easybuild-easyconfigs bumps:
//! batch "toolchain refresh" PRs that mix a toolchain-generation
//! definition with application recipes in one diff, and non-linear
//! version history where the PR's target version is not actually the
//! newest at that toolchain generation (a backfill, not a clean
//! next-version bump). Both checks operate on data the miner already has
//! once it has parsed a candidate file and listed the merge-base tree, so
//! neither needs new I/O.

use crate::eb_parse::ResolvedEasyconfig;
use crate::version::cmp_version;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

/// True when `recipe` defines a toolchain generation itself (for example
/// `gompi-2026.1.eb`, `easyblock = 'Toolchain'`, `moduleclass = 'toolchain'`)
/// rather than an application built with one.
///
/// Toolchain-definition recipes are not `bump` targets: bumping an
/// application recipe assumes a toolchain generation to move it to
/// already exists, and a toolchain-definition recipe *is* that
/// generation, not a consumer of it. A miner that does not filter these
/// out will try to "reproduce" a recipe that never had a
/// previous-generation counterpart to bump from in the first place.
///
/// Checks `easyblock` first since it is the more direct signal (the
/// literal easyblock class EasyBuild uses to build a toolchain
/// definition); `moduleclass` is a secondary check for recipes where the
/// parser did not resolve an `easyblock` value.
pub fn is_toolchain_meta_recipe(recipe: &ResolvedEasyconfig) -> bool {
    matches!(recipe.easyblock.as_deref(), Some("Toolchain"))
        || matches!(recipe.moduleclass.as_deref(), Some("toolchain"))
}

/// True when `candidate_version` is not the highest version already
/// present in `existing_versions` for the same package and toolchain
/// generation, at the PR's merge-base commit.
///
/// A miner mining "PR bumps package X to version Y" candidates has to
/// distinguish a clean next-version bump (Y is newer than anything
/// already in the tree at that generation) from a backfill (an older
/// release added alongside a newer one that already merged separately).
/// Scoring a backfill as a bump-reproduction target compares the wrong
/// two files. Version ordering uses [`cmp_version`], not a string sort,
/// so `"4.2.1"` correctly outranks `"3.31.11"` (a plain string compare
/// would rank them the other way around, since `'4' > '3'` only holds
/// for the leading character and a string sort does not parse dotted
/// version segments numerically).
pub fn is_backfill(candidate_version: &str, existing_versions: &[String]) -> bool {
    existing_versions
        .iter()
        .any(|v| cmp_version(v, candidate_version) == Ordering::Greater)
}

/// A multi-line string literal the normalizer is currently inside.
///
/// Only triple-quoted literals carry across a line boundary, so this is
/// the whole of the state the line scanner has to remember.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum OpenString {
    /// Opened with `'''`.
    Single,
    /// Opened with `"""`.
    Double,
}

impl OpenString {
    /// The three-character delimiter that closes this literal.
    fn delimiter(self) -> &'static str {
        match self {
            OpenString::Single => "'''",
            OpenString::Double => "\"\"\"",
        }
    }
}

/// What one line of easyconfig text contributes once its comment is gone.
struct ScannedLine {
    /// The line with any `#` comment removed. String content is kept
    /// verbatim, including a `#` that falls inside a literal.
    code: String,
    /// True when a comment was found and dropped.
    had_comment: bool,
    /// The multi-line literal still open at the end of the line, if any.
    open: Option<OpenString>,
}

/// The character starting at byte offset `at`, which is a char boundary
/// because the scanner only ever advances by whole characters.
fn next_char(line: &str, at: usize) -> char {
    line[at..]
        .chars()
        .next()
        .expect("scanner offsets stay on character boundaries")
}

/// Split one line into code and comment, tracking multi-line string state.
///
/// A `#` only starts a comment outside a string literal: `sources` and
/// `source_urls` routinely carry a `#` inside quotes (URL fragments,
/// `%(name)s` templates around anchors), and a scanner that cut on the
/// first `#` would silently truncate them. Escapes follow normal Python
/// rules, so `'don\'t'` stays one literal.
fn scan_line(line: &str, open: Option<OpenString>) -> ScannedLine {
    let bytes = line.as_bytes();
    let mut code = String::new();
    let mut had_comment = false;
    let mut open = open;
    let mut i = 0usize;

    while i < bytes.len() {
        if let Some(state) = open {
            let delimiter = state.delimiter();
            match line[i..].find(delimiter) {
                Some(offset) => {
                    let end = i + offset + delimiter.len();
                    code.push_str(&line[i..end]);
                    i = end;
                    open = None;
                }
                None => {
                    code.push_str(&line[i..]);
                    i = bytes.len();
                }
            }
            continue;
        }

        let c = next_char(line, i);
        if c == '#' {
            had_comment = true;
            break;
        }
        if c == '\'' || c == '"' {
            let triple = [c as u8, c as u8, c as u8];
            if bytes[i..].starts_with(&triple) {
                open = Some(if c == '\'' {
                    OpenString::Single
                } else {
                    OpenString::Double
                });
                code.push_str(&line[i..i + 3]);
                i += 3;
                continue;
            }
            // Single-line literal: copy through its closing quote, or to
            // the end of the line when the source is malformed.
            code.push(c);
            i += 1;
            while i < bytes.len() {
                let inner = next_char(line, i);
                code.push(inner);
                i += inner.len_utf8();
                if inner == '\\' && i < bytes.len() {
                    let escaped = next_char(line, i);
                    code.push(escaped);
                    i += escaped.len_utf8();
                    continue;
                }
                if inner == c {
                    break;
                }
            }
            continue;
        }

        code.push(c);
        i += c.len_utf8();
    }

    ScannedLine {
        code,
        had_comment,
        open,
    }
}

/// Strip from `text` everything that cannot change what EasyBuild builds:
/// comments, trailing whitespace, and runs of blank lines.
///
/// Two easyconfigs with the same normalization build the same thing, so
/// this is the function that draws the [`ReproScore::Semantic`] boundary,
/// and the reason scoring runs on it rather than on raw text: a raw diff
/// reports a deleted commented-out line and a lost blank line as loudly
/// as a dropped patch, which flattens the very distinction the ladder
/// exists to make.
///
/// What it removes:
///
/// - comment-only lines, dropped whole rather than left as blanks, so a
///   deleted commented-out block does not resurface as whitespace noise;
/// - the `# ...` tail of a code line, with the code before it kept;
/// - trailing whitespace on any line not ending inside a string literal;
/// - runs of blank lines, collapsed to one, and blank lines at either end.
///
/// What it keeps: everything inside a string literal, verbatim. A
/// triple-quoted `description` carries its own blank lines, indentation
/// and `#` characters into the built module, so normalizing them away
/// would call two different module descriptions equivalent.
pub fn normalize_for_scoring(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut open: Option<OpenString> = None;

    for raw in text.lines() {
        let inside_string_at_start = open.is_some();
        let scanned = scan_line(raw, open);
        open = scanned.open;

        let line = if open.is_some() {
            // Ends inside a literal: trailing whitespace is string content.
            scanned.code
        } else {
            scanned.code.trim_end().to_string()
        };

        if line.is_empty() && !inside_string_at_start {
            if scanned.had_comment {
                continue;
            }
            if out.last().map(|l| l.is_empty()).unwrap_or(true) {
                continue;
            }
        }
        out.push(line);
    }

    while out.last().map(|l| l.is_empty()).unwrap_or(false) {
        out.pop();
    }
    out.join("\n")
}

/// Where a reproduction attempt landed on the scoreboard ladder.
///
/// The two ladder rungs missing here, `ERROR` and `excluded`, are not
/// outcomes of comparing two files: `ERROR` means no file was produced to
/// compare, and `excluded` means the file was never a scoreable bump
/// (a toolchain meta-recipe, a backfill, a package with no prior recipe).
/// Both are decided before this comparison runs, and both are recorded
/// with their reason rather than dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ReproScore {
    /// Byte-identical to the merged file.
    Exact,
    /// Differs only in ways that cannot change the build: comments,
    /// trailing whitespace, blank-line runs.
    Semantic,
    /// A real difference in what would be built.
    Material,
}

impl ReproScore {
    /// The scoreboard spelling, for a `| File | Score | Note |` row.
    pub fn as_str(self) -> &'static str {
        match self {
            ReproScore::Exact => "EXACT",
            ReproScore::Semantic => "SEMANTIC",
            ReproScore::Material => "MATERIAL",
        }
    }
}

impl fmt::Display for ReproScore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Score `emitted` against the real merged `target`.
///
/// `EXACT` is raw equality, so it still means what a reader assumes it
/// means; normalization only decides `SEMANTIC` against `MATERIAL`.
pub fn score_reproduction(emitted: &str, target: &str) -> ReproScore {
    if emitted == target {
        return ReproScore::Exact;
    }
    if normalize_for_scoring(emitted) == normalize_for_scoring(target) {
        return ReproScore::Semantic;
    }
    ReproScore::Material
}

/// Which side of the comparison a diff line came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffTag {
    /// Present in the real merged target, absent from the emitted file.
    Removed,
    /// Present in the emitted file, absent from the target.
    Added,
}

/// One differing line of the raw (un-normalized) diff.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffLine {
    /// Which file the line belongs to.
    pub tag: DiffTag,
    /// The line, exactly as it appears in that file.
    pub text: String,
}

/// A scored reproduction: the ladder rung, plus the raw diff behind it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReproComparison {
    /// The ladder rung, decided on normalized text.
    pub score: ReproScore,
    /// Every line that differs between the raw files, in file order.
    ///
    /// Deliberately raw: a `SEMANTIC` verdict is only auditable if the
    /// reader can see the comment and whitespace differences the score
    /// forgave and judge that call for themselves.
    pub raw_diff: Vec<DiffLine>,
}

impl ReproComparison {
    /// The raw diff as text, one `-`/`+` line per difference.
    ///
    /// `-` lines are the real merged target, `+` lines are what the bump
    /// emitted.
    pub fn render_raw_diff(&self) -> String {
        self.raw_diff
            .iter()
            .map(|line| {
                let marker = match line.tag {
                    DiffTag::Removed => '-',
                    DiffTag::Added => '+',
                };
                format!("{marker}{}", line.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Cell budget for the longest-common-subsequence table.
///
/// Easyconfigs run to a few hundred lines, so the quadratic table is
/// small in practice. Past this budget the diff degrades to a whole-file
/// replacement rather than allocating without bound; the score itself is
/// unaffected, since it never consults the diff.
const DIFF_CELL_BUDGET: usize = 1_000_000;

/// Score `emitted` against `target` and keep the raw diff for display.
pub fn compare_reproduction(emitted: &str, target: &str) -> ReproComparison {
    ReproComparison {
        score: score_reproduction(emitted, target),
        raw_diff: raw_line_diff(emitted, target),
    }
}

/// Every line that differs between the raw texts, longest common
/// subsequence over lines, target side first.
fn raw_line_diff(emitted: &str, target: &str) -> Vec<DiffLine> {
    let target_lines: Vec<&str> = target.lines().collect();
    let emitted_lines: Vec<&str> = emitted.lines().collect();

    let whole_file = || {
        target_lines
            .iter()
            .map(|line| DiffLine {
                tag: DiffTag::Removed,
                text: (*line).to_string(),
            })
            .chain(emitted_lines.iter().map(|line| DiffLine {
                tag: DiffTag::Added,
                text: (*line).to_string(),
            }))
            .collect::<Vec<_>>()
    };

    let (n, m) = (target_lines.len(), emitted_lines.len());
    if n.saturating_mul(m) > DIFF_CELL_BUDGET {
        return whole_file();
    }

    // lcs[i][j] = length of the longest common subsequence of the tails
    // target_lines[i..] and emitted_lines[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if target_lines[i] == emitted_lines[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut diff = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if target_lines[i] == emitted_lines[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            diff.push(DiffLine {
                tag: DiffTag::Removed,
                text: target_lines[i].to_string(),
            });
            i += 1;
        } else {
            diff.push(DiffLine {
                tag: DiffTag::Added,
                text: emitted_lines[j].to_string(),
            });
            j += 1;
        }
    }
    for line in &target_lines[i..] {
        diff.push(DiffLine {
            tag: DiffTag::Removed,
            text: (*line).to_string(),
        });
    }
    for line in &emitted_lines[j..] {
        diff.push(DiffLine {
            tag: DiffTag::Added,
            text: (*line).to_string(),
        });
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Candidate, Toolchain};
    use crate::eb_parse::{existing_versions, resolve_easyconfig_str, ExistingVersionsQuery};

    fn backfill_candidate(name: &str, version: &str, toolchain: &Toolchain) -> Candidate {
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: toolchain.clone(),
            versionsuffix: None,
            easyconfig_path: String::new(),
            dependencies: vec![],
            builddependencies: vec![],
            exts_list: vec![],
        }
    }

    /// gompi-2026.1.eb, easybuilders/easybuild-easyconfigs PR #25613,
    /// merge commit f929f1f2ef4c15feb19692f33da59e9e1e072572. Real
    /// toolchain-definition recipe from the same PR whose sibling file
    /// (OpenMPI-5.0.10-GCC-15.2.0.eb) is a genuine bump target.
    const GOMPI_2026_1: &str = r#"
easyblock = 'Toolchain'

name = 'gompi'
version = '2026.1'

homepage = '(none)'
description = """GNU Compiler Collection (GCC) based compiler toolchain,
 including OpenMPI for MPI support."""

toolchain = SYSTEM

local_gccver = '15.2.0'

dependencies = [
    ('GCC', local_gccver),
    ('OpenMPI', '5.0.10', '', ('GCC', local_gccver)),
]

moduleclass = 'toolchain'
"#;

    /// OpenMPI-5.0.10-GCC-15.2.0.eb, same PR #25613: a genuine application
    /// recipe, not a toolchain definition. Only the header fields matter
    /// for this classifier, so the body is trimmed to those.
    const OPENMPI_5_0_10: &str = r#"
easyblock = 'ConfigureMake'

name = 'OpenMPI'
version = '5.0.10'

homepage = 'https://www.open-mpi.org/'
description = "OpenMPI"

toolchain = {'name': 'GCC', 'version': '15.2.0'}

source_urls = ['https://download.open-mpi.org/release/open-mpi/v5.0']
sources = ['%(namelower)s-%(version)s.tar.bz2']
checksums = ['0000000000000000000000000000000000000000000000000000000000000000']

moduleclass = 'mpi'
"#;

    #[test]
    fn toolchain_definition_recipe_is_filtered() {
        let recipe = resolve_easyconfig_str(GOMPI_2026_1).expect("parse gompi-2026.1.eb");
        assert!(
            is_toolchain_meta_recipe(&recipe),
            "gompi-2026.1.eb (easyblock=Toolchain, moduleclass=toolchain) must be classified as a toolchain meta-recipe"
        );
    }

    #[test]
    fn application_recipe_in_the_same_pr_is_not_filtered() {
        let recipe =
            resolve_easyconfig_str(OPENMPI_5_0_10).expect("parse OpenMPI-5.0.10-GCC-15.2.0.eb");
        assert!(
            !is_toolchain_meta_recipe(&recipe),
            "OpenMPI-5.0.10-GCC-15.2.0.eb is a real application recipe and must not be filtered as a toolchain meta-recipe"
        );
    }

    #[test]
    fn backfill_below_an_existing_newer_version_is_detected() {
        // easybuilders/easybuild-easyconfigs PR #25231: "CMake v3.31.11" at
        // GCCcore-15.2.0 merges into a tree that already has
        // CMake-4.2.1-GCCcore-15.2.0.eb from an earlier PR. A plain string
        // compare of "3.31.11" vs "4.2.1" would (wrongly) rank 4.2.1 lower;
        // cmp_version must not make that mistake.
        let generation = Toolchain {
            name: "GCCcore".into(),
            version: "15.2.0".into(),
        };
        let universe = vec![backfill_candidate("CMake", "4.2.1", &generation)];
        let existing = existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "CMake",
            generation: &generation,
        });
        assert!(
            is_backfill("3.31.11", &existing),
            "3.31.11 merging after 4.2.1 already exists at the same generation must be flagged as a backfill"
        );
    }

    #[test]
    fn clean_next_version_bump_is_not_a_backfill() {
        // easybuilders/easybuild-easyconfigs PR #26010: Boost 1.90.0 at
        // GCC-15.2.0, with only 1.88.0 present at the prior generation
        // (GCC-14.3.0) in the merge-base tree, nothing newer already at
        // 15.2.0.
        let generation = Toolchain {
            name: "GCC".into(),
            version: "15.2.0".into(),
        };
        let prior_generation = Toolchain {
            name: "GCC".into(),
            version: "14.3.0".into(),
        };
        let universe = vec![backfill_candidate("Boost", "1.88.0", &prior_generation)];
        let existing = existing_versions(&ExistingVersionsQuery {
            universe: &universe,
            name: "Boost",
            generation: &generation,
        });
        assert!(
            !is_backfill("1.90.0", &existing),
            "a version with nothing newer already in the tree at that generation is a clean bump, not a backfill"
        );
    }

    /// LLVM-21.1.8-GCCcore-15.2.0.eb as easybuilders/easybuild-easyconfigs
    /// PR #25009 merged it (merge commit 3e86bdac), through the patch list.
    /// The rest of the file is dependencies and sanity checks, which the
    /// bump reproduced exactly and which nothing here exercises.
    const LLVM_21_MERGED: &str = r#"
name = 'LLVM'
version = '21.1.8'


homepage = "https://llvm.org/"
description = """
The LLVM Core libraries provide a modern source- and target-independent
optimizer, along with code generation support for many popular CPUs
(as well as some less common ones!) These libraries are built around a well
specified code representation known as the LLVM intermediate representation
("LLVM IR"). The LLVM Core libraries are well documented, and it is
particularly easy to invent your own language (or port an existing compiler)
to use LLVM as an optimizer and code generator.
"""

toolchain = {'name': 'GCCcore', 'version': '15.2.0'}
toolchainopts = {
    'pic': True
}

source_urls = ['https://github.com/llvm/llvm-project/releases/download/llvmorg-%(version)s/']
sources = [
    'llvm-project-%(version)s.src.tar.xz',
]
patches = [
    'LLVM-18.1.8_envintest.patch',
    'LLVM-19.1.7_libomptarget_tests.patch',
    'LLVM-19.1.7_clang_rpathwrap_test.patch',
    'LLVM-21.1.x_fix-ompt-end-critical-race.patch',
    'LLVM-21.1.x_fix-sema-overload-deduction.patch',
]
"#;

    /// The same file as `eb-stack bump` emitted it, with the two cosmetic
    /// divergences the PR #25009 scoring run found and nothing else: the
    /// stale commented-out `toolchainopts` entry that the bump preserves
    /// verbatim from the 20.1.8 source and the real merge deleted as dead
    /// weight, and one blank-line difference.
    const LLVM_21_EMITTED_COSMETIC: &str = r#"
name = 'LLVM'
version = '21.1.8'

homepage = "https://llvm.org/"
description = """
The LLVM Core libraries provide a modern source- and target-independent
optimizer, along with code generation support for many popular CPUs
(as well as some less common ones!) These libraries are built around a well
specified code representation known as the LLVM intermediate representation
("LLVM IR"). The LLVM Core libraries are well documented, and it is
particularly easy to invent your own language (or port an existing compiler)
to use LLVM as an optimizer and code generator.
"""

toolchain = {'name': 'GCCcore', 'version': '15.2.0'}
toolchainopts = {
    # 'cstd': 'gnu++11',
    'pic': True
}

source_urls = ['https://github.com/llvm/llvm-project/releases/download/llvmorg-%(version)s/']
sources = [
    'llvm-project-%(version)s.src.tar.xz',
]
patches = [
    'LLVM-18.1.8_envintest.patch',
    'LLVM-19.1.7_libomptarget_tests.patch',
    'LLVM-19.1.7_clang_rpathwrap_test.patch',
    'LLVM-21.1.x_fix-ompt-end-critical-race.patch',
    'LLVM-21.1.x_fix-sema-overload-deduction.patch',
]
"#;

    /// The third divergence from the same run: the patch set the bump
    /// carried over from LLVM 20.1.8, which the real merge narrowed. Two
    /// of the retained patches are the ones upstream folded in at 21.1.x,
    /// so this file builds different sources than the merged one.
    const LLVM_21_EMITTED_PATCH_SET: &str = r#"
name = 'LLVM'
version = '21.1.8'

homepage = "https://llvm.org/"
description = """
The LLVM Core libraries provide a modern source- and target-independent
optimizer, along with code generation support for many popular CPUs
(as well as some less common ones!) These libraries are built around a well
specified code representation known as the LLVM intermediate representation
("LLVM IR"). The LLVM Core libraries are well documented, and it is
particularly easy to invent your own language (or port an existing compiler)
to use LLVM as an optimizer and code generator.
"""

toolchain = {'name': 'GCCcore', 'version': '15.2.0'}
toolchainopts = {
    # 'cstd': 'gnu++11',
    'pic': True
}

source_urls = ['https://github.com/llvm/llvm-project/releases/download/llvmorg-%(version)s/']
sources = [
    'llvm-project-%(version)s.src.tar.xz',
]
patches = [
    'LLVM-18.1.8_envintest.patch',
    'LLVM-19.1.7_libomptarget_tests.patch',
    'LLVM-19.1.7_clang_rpathwrap_test.patch',
    'LLVM-20.1.x_improved-CUDA-13-support.patch',
    'LLVM-20.1.5-fix_bindc_commonblocks_fortran.patch',  # This patch is included upstream from >=21.1.x
    'LLVM-20.1.x_always-link-compiler-rt-to-flang.patch',  # This patch is included upstream from >= 21.1.x
]
"#;

    #[test]
    fn dead_comments_and_whitespace_score_semantic_not_material() {
        assert_eq!(
            score_reproduction(LLVM_21_EMITTED_COSMETIC, LLVM_21_MERGED),
            ReproScore::Semantic,
            "a file differing from the merged one only by a commented-out line and a blank line \
             builds exactly what the merged one builds, and a raw text diff calling that MATERIAL \
             is what this normalization exists to fix"
        );
    }

    #[test]
    fn a_narrowed_patch_set_still_scores_material() {
        assert_eq!(
            score_reproduction(LLVM_21_EMITTED_PATCH_SET, LLVM_21_MERGED),
            ReproScore::Material,
            "carrying three LLVM-20.1.x patches the merge dropped changes the sources that get \
             built; normalization must not forgive it just because two of them carry comments"
        );
    }

    #[test]
    fn the_merged_file_scores_exact_against_itself() {
        assert_eq!(
            score_reproduction(LLVM_21_MERGED, LLVM_21_MERGED),
            ReproScore::Exact,
            "EXACT is raw equality, so it keeps meaning byte-identical"
        );
    }

    #[test]
    fn a_semantic_score_still_shows_the_raw_difference() {
        let comparison = compare_reproduction(LLVM_21_EMITTED_COSMETIC, LLVM_21_MERGED);
        assert_eq!(comparison.score, ReproScore::Semantic);
        let rendered = comparison.render_raw_diff();
        assert!(
            rendered.contains("+    # 'cstd': 'gnu++11',"),
            "the raw diff must still carry the forgiven comment line so the SEMANTIC call is \
             auditable, got:\n{rendered}"
        );
        assert!(
            comparison
                .raw_diff
                .iter()
                .any(|line| line.tag == DiffTag::Removed && line.text.is_empty()),
            "the raw diff must still carry the forgiven blank line, got:\n{rendered}"
        );
    }

    #[test]
    fn a_comment_only_line_disappears_and_the_code_around_it_does_not() {
        let normalized = normalize_for_scoring(
            "toolchainopts = {\n    # 'cstd': 'gnu++11',\n    'pic': True\n}\n",
        );
        assert_eq!(normalized, "toolchainopts = {\n    'pic': True\n}");
    }

    #[test]
    fn a_trailing_comment_goes_and_the_statement_stays() {
        // Real trailing comments from PR #25009's dropped patch lines.
        let normalized = normalize_for_scoring(
            "    'LLVM-20.1.5-fix_bindc_commonblocks_fortran.patch',  # included upstream\n",
        );
        assert_eq!(
            normalized,
            "    'LLVM-20.1.5-fix_bindc_commonblocks_fortran.patch',"
        );
    }

    #[test]
    fn a_hash_inside_a_string_is_not_a_comment() {
        let with_fragment = "source_urls = ['https://example.org/dl#anchor']\n";
        assert_eq!(
            normalize_for_scoring(with_fragment),
            "source_urls = ['https://example.org/dl#anchor']",
            "cutting at the first # would silently truncate a URL fragment and make two different \
             download locations normalize to the same text"
        );
    }

    #[test]
    fn blank_line_runs_collapse_and_the_ends_are_trimmed() {
        assert_eq!(
            normalize_for_scoring("\n\nname = 'LLVM'\n\n\n\nversion = '21.1.8'\n\n"),
            "name = 'LLVM'\n\nversion = '21.1.8'"
        );
    }

    #[test]
    fn trailing_whitespace_does_not_survive_normalization() {
        assert_eq!(
            normalize_for_scoring("version = '21.1.8'   \n"),
            "version = '21.1.8'"
        );
    }

    #[test]
    fn a_description_body_is_string_content_and_survives_intact() {
        // A module description reaches the user verbatim, so its blank
        // lines, indentation and any # it contains are build output, not
        // dead text.
        let described = "description = \"\"\"first\n\n\n#  not a comment\n   spaced   \n\"\"\"\n";
        assert_eq!(
            normalize_for_scoring(described),
            "description = \"\"\"first\n\n\n#  not a comment\n   spaced   \n\"\"\"",
            "normalizing inside a triple-quoted description would call two different module \
             descriptions equivalent"
        );
    }

    #[test]
    fn two_descriptions_differing_only_inside_the_string_score_material() {
        let one = "description = \"\"\"a\n\nb\"\"\"\n";
        let two = "description = \"\"\"a\nb\"\"\"\n";
        assert_eq!(
            score_reproduction(one, two),
            ReproScore::Material,
            "a blank line inside a description is content the built module shows"
        );
    }
}
