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

/// One review comment the fix has to answer, anchored in the file as first
/// seen (1-based, inclusive).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewItem {
    /// Caller's id for the comment (reported back as given).
    pub id: String,
    pub line_start: usize,
    pub line_end: usize,
    /// The comment's suggestion block: `None` for a prose comment, `Some("")`
    /// for an empty suggestion (delete the anchored lines).
    pub suggestion: Option<String>,
    /// The comment text, used only for the `mentions_absent` hint.
    pub comment: String,
}

/// Whether the change from the first-seen file answers one review item.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReviewItemStatus {
    pub id: String,
    pub line_start: usize,
    pub line_end: usize,
    /// `changed` (a hunk touches the anchored lines), `suggestion_applied`,
    /// `deleted`, `no_change_needed` (acknowledged by the caller), or, while
    /// open: `no_hunk`, `suggestion_missing`, `not_deleted`.
    pub status: &'static str,
    pub open: bool,
    /// Suggestion lines the file does not contain yet.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Backticked names from the comment that the file does not contain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions_absent: Vec<String>,
}

/// Whitespace-free text with trailing commas before a closing bracket (and
/// at the end) dropped, so a suggestion re-wrapped over several lines still matches.
fn squash(text: &str) -> String {
    let mut out: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    for (from, to) in [(",]", "]"), (",)", ")"), (",}", "}")] {
        while out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out.trim_end_matches(',').to_string()
}

/// A line that carries content (not blank, not only brackets and commas,
/// not a comment).
fn content_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && !trimmed.starts_with('#')
        && trimmed
            .chars()
            .any(|c| !matches!(c, '[' | ']' | '(' | ')' | '{' | '}' | ','))
}

/// Backticked spans of a comment (3 to 80 characters, no whitespace).
fn backticked(comment: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (index, span) in comment.split('`').enumerate() {
        if index % 2 == 1
            && (3..=80).contains(&span.chars().count())
            && !span.chars().any(char::is_whitespace)
            && !out.iter().any(|seen: &String| seen == span)
        {
            out.push(span.to_string());
        }
    }
    out
}

/// Status of each review item for the change from `base` (the file as first
/// seen, the numbering the items use) to `text`. Ids in `acknowledged` were
/// answered without a change (the caller says none was needed).
pub fn review_item_status(
    base: &str,
    text: &str,
    items: &[ReviewItem],
    acknowledged: &[String],
) -> Vec<ReviewItemStatus> {
    let all = hunks(base, text);
    let base_lines: Vec<&str> = base.lines().collect();
    let squashed = squash(text);
    let count = |haystack: &str, needle: &str| {
        haystack
            .lines()
            .filter(|line| content_line(line) && squash(line) == needle)
            .count()
    };
    items
        .iter()
        .map(|item| {
            let start = item.line_start.max(1);
            let end = item.line_end.max(start);
            let touched = all.iter().any(|hunk| {
                let (from, to) = hunk.old_range();
                // An insertion sits between old_start - 1 and old_start.
                let to = if hunk.removed.is_empty() { from } else { to };
                from <= end + 1 && to + 1 >= start
            });
            let mut missing = Vec::new();
            let mut status = match item.suggestion.as_deref() {
                None => {
                    if touched {
                        "changed"
                    } else {
                        "no_hunk"
                    }
                }
                Some(suggestion) if suggestion.trim().is_empty() => {
                    let anchored: Vec<String> = base_lines
                        .get(start - 1..end.min(base_lines.len()))
                        .unwrap_or_default()
                        .iter()
                        .filter(|line| content_line(line))
                        .map(|line| squash(line))
                        .collect();
                    let gone = if anchored.is_empty() {
                        touched
                    } else {
                        anchored
                            .iter()
                            .any(|line| count(text, line.as_str()) < count(base, line.as_str()))
                    };
                    if gone {
                        "deleted"
                    } else {
                        "not_deleted"
                    }
                }
                Some(suggestion) => {
                    missing = suggestion
                        .lines()
                        .filter(|line| content_line(line))
                        .filter(|line| !squashed.contains(&squash(line)))
                        .map(|line| line.trim().to_string())
                        .collect();
                    if missing.is_empty() {
                        "suggestion_applied"
                    } else {
                        "suggestion_missing"
                    }
                }
            };
            let mut open = matches!(status, "no_hunk" | "not_deleted" | "suggestion_missing");
            if open && acknowledged.iter().any(|id| *id == item.id) {
                status = "no_change_needed";
                open = false;
            }
            let mentions_absent = backticked(&item.comment)
                .into_iter()
                .filter(|name| !text.contains(name.as_str()))
                .collect();
            ReviewItemStatus {
                id: item.id.clone(),
                line_start: start,
                line_end: end,
                status,
                open,
                missing,
                mentions_absent,
            }
        })
        .collect()
}

/// `EB_REVIEW_ITEM_OPEN` (warning that blocks clean) for each open item.
pub fn review_item_findings(statuses: &[ReviewItemStatus]) -> Vec<StyleFinding> {
    statuses
        .iter()
        .filter(|status| status.open)
        .map(|status| {
            let what = match status.status {
                "suggestion_missing" => format!(
                    "its suggestion is not in the file yet (missing: {}); apply it with eb_recipe_apply_suggestion",
                    status.missing.join(" | ")
                ),
                "not_deleted" => "it asks to delete the anchored lines and they are still there".to_string(),
                _ => "no change touches its anchored lines".to_string(),
            };
            let mut message = format!(
                "review item {} (lines {}-{} as first seen) is not answered: {what}. Make the change it asks for; if it needs no change (a question already answered, the file already does it, or you met its intent in another form the rules require), pass its id in no_change_needed and say why in the final answer",
                status.id, status.line_start, status.line_end
            );
            if !status.mentions_absent.is_empty() {
                message.push_str(&format!(
                    ". The comment names {}, which the file does not contain",
                    status
                        .mentions_absent
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            StyleFinding {
                line: status.line_start.max(1),
                column: 1,
                code: "EB_REVIEW_ITEM_OPEN".to_string(),
                message,
                mechanical: false,
                origin: String::new(),
                severity: severity_for("EB_REVIEW_ITEM_OPEN"),
                info: false,
                in_review: false,
            }
        })
        .collect()
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

    fn item(id: &str, start: usize, end: usize, suggestion: Option<&str>) -> ReviewItem {
        ReviewItem {
            id: id.to_string(),
            line_start: start,
            line_end: end,
            suggestion: suggestion.map(str::to_string),
            comment: String::new(),
        }
    }

    #[test]
    fn review_items_report_prose_comments_without_a_hunk() {
        let new = BASE.replace("x = 1", "x = 10");
        let items = [item("a", 8, 8, None), item("b", 11, 11, None)];
        let found = review_item_status(BASE, &new, &items, &[]);
        assert_eq!(found[0].status, "changed");
        assert!(!found[0].open);
        assert_eq!(found[1].status, "no_hunk");
        assert!(found[1].open);
        let findings = review_item_findings(&found);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, "EB_REVIEW_ITEM_OPEN");
        assert!(super::super::blocks_clean(&findings[0]));
        // Acknowledged: answered without a change.
        let acked = review_item_status(BASE, &new, &items, &["b".to_string()]);
        assert_eq!(acked[1].status, "no_change_needed");
        assert!(review_item_findings(&acked).is_empty());
    }

    #[test]
    fn review_items_check_suggestions_even_when_rewrapped() {
        let base =
            "name = 'foo'\nsanity_check_paths = {\n    'files': ['bin/foo'],\n    'dirs': [],\n}\n";
        let suggestion = "    'files': ['bin/foo', 'lib/libfoo.a'],\n    'dirs': ['include'],";
        let items = [item("s", 3, 4, Some(suggestion))];
        let open = review_item_status(base, base, &items, &[]);
        assert_eq!(open[0].status, "suggestion_missing");
        assert_eq!(
            open[0].missing,
            [
                "'files': ['bin/foo', 'lib/libfoo.a'],",
                "'dirs': ['include'],"
            ]
        );
        // Re-wrapped, and the last entry before '}' keeps its trailing comma.
        let wrapped = base
            .replace(
                "    'files': ['bin/foo'],\n",
                "    'files': [\n        'bin/foo',\n        'lib/libfoo.a',\n    ],\n",
            )
            .replace("'dirs': [],", "'dirs': ['include'],");
        let done = review_item_status(base, &wrapped, &items, &[]);
        assert_eq!(done[0].status, "suggestion_applied", "{done:?}");
    }

    #[test]
    fn review_items_check_empty_suggestions_delete_their_lines() {
        let base =
            "name = 'foo'\ndependencies = [\n    ('zlib', '1.3'),\n    ('bzip2', '1.0'),\n]\n";
        let items = [item("d", 4, 4, Some(""))];
        assert_eq!(
            review_item_status(base, base, &items, &[])[0].status,
            "not_deleted"
        );
        let moved = base.replace("    ('zlib', '1.3'),\n", "");
        assert_eq!(
            review_item_status(base, &moved, &items, &[])[0].status,
            "not_deleted"
        );
        let deleted = base.replace("    ('bzip2', '1.0'),\n", "");
        assert_eq!(
            review_item_status(base, &deleted, &items, &[])[0].status,
            "deleted"
        );
    }

    #[test]
    fn review_items_name_backticked_tokens_the_file_lacks() {
        let mut asked = item("q", 2, 2, None);
        asked.comment = "should this set `WITH_PIC=ON`? see `name`".to_string();
        let found = review_item_status(BASE, BASE, &[asked], &[]);
        assert_eq!(found[0].mentions_absent, ["WITH_PIC=ON"]);
        let findings = review_item_findings(&found);
        assert!(
            findings[0].message.contains("`WITH_PIC=ON`"),
            "{findings:?}"
        );
    }
}
