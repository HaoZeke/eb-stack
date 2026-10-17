//! Diff scope: which parts of a review fix touch lines the review is not
//! about.
//!
//! A review fix changes what the comments ask for and what that change
//! needs, nothing else (easybuild-dos-donts R1). The hunks between the file as
//! first seen and the file now are compared with the review anchors:
//!
//! - `EB_DIFF_OUTSIDE_REVIEW` (warning): a hunk more than [`NEAR`] lines from
//!   every review line whose change is not the same kind of change as a hunk
//!   near the review (same tokens removed and added, as when one fix is
//!   applied at every occurrence).
//! - `EB_DIFF_SOURCES_CHANGED` (warning that blocks clean): such a hunk
//!   inside `sources`, `checksums`, `crates`, `patches`, `source_urls` or
//!   `data_sources`. EasyBuild verifies every download against `checksums`
//!   (`eb --check-contrib`, the easyconfigs test suite), so editing them
//!   without being asked breaks the recipe or hides a real problem.
//!
//! Both are skipped under `review_scope: "file"` and when no review lines
//! are known.

use std::collections::BTreeSet;

use super::{severity_for, StyleFinding};

/// A review line this close to a hunk (in either the old or the new file's
/// numbering) makes the hunk part of the review.
pub const NEAR: usize = 3;

/// Parameters whose values are downloads and their checksums.
const SOURCE_PARAMS: &[&str] = &[
    "sources",
    "checksums",
    "crates",
    "patches",
    "source_urls",
    "data_sources",
];

/// One changed region between two texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// 1-based first line in the old text (the line after which lines were
    /// inserted when `removed` is empty).
    pub old_start: usize,
    /// Lines removed from the old text.
    pub removed: Vec<String>,
    /// 1-based first line in the new text (the line after which lines were
    /// removed when `added` is empty).
    pub new_start: usize,
    /// Lines added in the new text.
    pub added: Vec<String>,
    /// Top-level easyconfig parameters the hunk touches, old and new side.
    pub params: Vec<String>,
}

impl Hunk {
    fn old_range(&self) -> (usize, usize) {
        (
            self.old_start,
            self.old_start + self.removed.len().saturating_sub(1),
        )
    }

    fn new_range(&self) -> (usize, usize) {
        (
            self.new_start,
            self.new_start + self.added.len().saturating_sub(1),
        )
    }

    /// True when every changed line is a comment or blank.
    pub fn comment_only(&self) -> bool {
        self.removed
            .iter()
            .chain(self.added.iter())
            .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
    }

    /// Tokens removed and tokens added, ignoring tokens on both sides.
    fn signature(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let removed = tokens(&self.removed);
        let added = tokens(&self.added);
        (
            removed.difference(&added).cloned().collect(),
            added.difference(&removed).cloned().collect(),
        )
    }
}

fn tokens(lines: &[String]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // Comment lines do not change what the recipe does.
    for line in lines
        .iter()
        .filter(|line| !line.trim_start().starts_with('#'))
    {
        let mut word = String::new();
        for c in line.chars() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
                continue;
            }
            if !word.is_empty() {
                out.insert(std::mem::take(&mut word));
            }
            if !c.is_whitespace() {
                out.insert(c.to_string());
            }
        }
        if !word.is_empty() {
            out.insert(word);
        }
    }
    out
}

/// Top-level parameter each line belongs to (`name = ...` at column 0 starts
/// one; indented and continuation lines belong to the last one).
fn line_params(text: &str) -> Vec<String> {
    let mut current = String::new();
    text.lines()
        .map(|line| {
            let starts = line
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
            if starts {
                let name: String = line
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let rest = line[name.len()..].trim_start();
                if rest.starts_with('=') || rest.starts_with("+=") {
                    current = name;
                }
            }
            current.clone()
        })
        .collect()
}

/// Cell budget for the line LCS table; past it the whole file is one hunk.
const CELL_BUDGET: usize = 2_000_000;

/// Line hunks from `old` to `new` (longest common subsequence over lines).
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (pa, pb) = (line_params(old), line_params(new));
    let (n, m) = (a.len(), b.len());
    let mut out = Vec::new();
    if n.saturating_mul(m) > CELL_BUDGET {
        if old != new {
            out.push(make_hunk(1, &a, 1, &b, &pa, &pb, 0, 0));
        }
        return out;
    }
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        while (i < n || j < m) && !(i < n && j < m && a[i] == b[j]) {
            if j >= m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
                i += 1;
            } else {
                j += 1;
            }
        }
        out.push(make_hunk(
            si + 1,
            &a[si..i],
            sj + 1,
            &b[sj..j],
            &pa,
            &pb,
            si,
            sj,
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn make_hunk(
    old_start: usize,
    removed: &[&str],
    new_start: usize,
    added: &[&str],
    pa: &[String],
    pb: &[String],
    si: usize,
    sj: usize,
) -> Hunk {
    let mut params = BTreeSet::new();
    for k in 0..removed.len() {
        if let Some(p) = pa.get(si + k).filter(|p| !p.is_empty()) {
            params.insert(p.clone());
        }
    }
    for k in 0..added.len() {
        if let Some(p) = pb.get(sj + k).filter(|p| !p.is_empty()) {
            params.insert(p.clone());
        }
    }
    Hunk {
        old_start,
        removed: removed.iter().map(|line| (*line).to_string()).collect(),
        new_start,
        added: added.iter().map(|line| (*line).to_string()).collect(),
        params: params.into_iter().collect(),
    }
}

fn near(range: (usize, usize), lines: &[usize]) -> bool {
    lines
        .iter()
        .any(|line| *line + NEAR >= range.0 && *line <= range.1 + NEAR)
}

/// `EB_DIFF_OUTSIDE_REVIEW` / `EB_DIFF_SOURCES_CHANGED` findings for the
/// change from `base` to `text`, with `review_lines` the anchored lines (in
/// the old or the new numbering; both are tried).
pub fn diff_scope_findings(base: &str, text: &str, review_lines: &[usize]) -> Vec<StyleFinding> {
    if review_lines.is_empty() {
        return Vec::new();
    }
    let all = hunks(base, text);
    let in_review =
        |hunk: &Hunk| near(hunk.old_range(), review_lines) || near(hunk.new_range(), review_lines);
    let reviewed: Vec<(BTreeSet<String>, BTreeSet<String>)> = all
        .iter()
        .filter(|hunk| in_review(hunk))
        .map(Hunk::signature)
        .collect();
    let mut out = Vec::new();
    for hunk in all.iter().filter(|hunk| !in_review(hunk)) {
        let (gone, new) = hunk.signature();
        let same_change = reviewed.iter().any(|(r_gone, r_new)| {
            !(gone.is_empty() && new.is_empty()) && gone.is_subset(r_gone) && new.is_subset(r_new)
        });
        if same_change || hunk.comment_only() {
            continue;
        }
        let sources: Vec<&String> = hunk
            .params
            .iter()
            .filter(|p| SOURCE_PARAMS.contains(&p.as_str()))
            .collect();
        let (code, message) = if sources.is_empty() {
            (
                "EB_DIFF_OUTSIDE_REVIEW",
                format!(
                    "this change (old lines {}-{}, {}) is not near any review line and is not the change the review asked for elsewhere; revert it unless the requested change needs it",
                    hunk.old_range().0,
                    hunk.old_range().1,
                    if hunk.params.is_empty() { "no parameter".to_string() } else { hunk.params.join(", ") }
                ),
            )
        } else {
            (
                "EB_DIFF_SOURCES_CHANGED",
                format!(
                    "{} changed but no review comment is about {}: never delete, add or re-spell sources, crates or checksums to clear a lint finding; restore them unless the review asks for this",
                    sources.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                    if sources.len() == 1 { "it" } else { "them" }
                ),
            )
        };
        out.push(StyleFinding {
            line: hunk.new_start.max(1),
            column: 1,
            code: code.to_string(),
            message,
            mechanical: false,
            origin: String::new(),
            severity: severity_for(code),
            info: false,
            in_review: false,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "name = 'foo'\n\
        sources = ['v%(version)s.tar.gz']\n\
        checksums = [\n    {'v1.tar.gz': 'aa'},\n    {'dep-1.tar.gz': 'bb'},\n]\n\
        \n\
        x = 1\n\
        y = 2\n\
        z = 3\n\
        runtest = 'foo -h'\n";

    #[test]
    fn hunks_cover_inserts_deletes_and_replacements() {
        let new = BASE
            .replace("    {'dep-1.tar.gz': 'bb'},\n", "")
            .replace("x = 1", "x = 10");
        let found = hunks(BASE, &new);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].params, ["checksums"]);
        assert_eq!(found[1].removed, ["x = 1"]);
    }

    #[test]
    fn unasked_checksum_edit_is_flagged_and_blocks() {
        let new = BASE
            .replace("    {'dep-1.tar.gz': 'bb'},\n", "")
            .replace("runtest = 'foo -h'", "sanity_check_commands = ['foo -h']");
        // review anchored on the runtest line (old line 11)
        let found = diff_scope_findings(BASE, &new, &[11]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, "EB_DIFF_SOURCES_CHANGED");
        assert!(super::super::blocks_clean(&found[0]));
    }

    #[test]
    fn same_change_elsewhere_and_comments_are_fine() {
        let base = "a = '%s' % X\nb = 1\nc = 2\nd = 3\ne = 4\nf = 5\ng = '%s' % X\n";
        let new = "a = f'{X}'\nb = 1\nc = 2\nd = 3\ne = 4\nf = 5\n# note\ng = f'{X}'\n";
        assert!(diff_scope_findings(base, new, &[1]).is_empty());
        let other = "a = f'{X}'\nb = 1\nc = 2\nd = 3\ne = 4\nf = 5\ng = '%s' % Y\n";
        let found = diff_scope_findings(base, other, &[1]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, "EB_DIFF_OUTSIDE_REVIEW");
        assert!(!super::super::blocks_clean(&found[0]));
        assert!(diff_scope_findings(base, other, &[]).is_empty());
    }
}
