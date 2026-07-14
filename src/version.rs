//! Simple version comparison for EasyBuild-style versions (X.Y.Z or year.Z).
//!
//! This targets EasyBuild-style version strings, not full PEP 440 / semver.
//! A version decomposes into an ordered run of tokens: maximal digit runs
//! parse as Num, maximal alphabetic runs parse as Alpha (case-folded to
//! lowercase), and any other character (dot, hyphen, underscore, ...) is a
//! separator that is dropped. Example: 1.0rc1 tokenizes to Num(1), Num(0),
//! Alpha(rc), Num(1); 2025a tokenizes to Num(2025), Alpha(a).
//!
//! Comparison walks both token sequences position by position. Two Num
//! tokens compare numerically; two Alpha tokens compare lexicographically
//! (so 2025a is before 2025b). A Num against a missing token pads the
//! missing side with zero. An Alpha against a missing token is treated as
//! a pre-release suffix (rc, alpha, beta, or a bare trailing letter) that
//! sorts before the side with nothing more. Mixed Num versus Alpha at the
//! same position is rare for EasyBuild; Num sorts before Alpha there for a
//! total deterministic order.

use std::cmp::Ordering;

/// One tokenized piece of a version string: a numeric run or an
/// alphabetic run (lowercased). Separator characters are dropped during
/// tokenization and never produce a `Part`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Num(u64),
    Alpha(String),
}

/// Tokenize a version string into an ordered run of numeric and
/// alphabetic parts, dropping separator characters. See the module docs
/// for the exact tokenization and comparison rules.
pub fn parse_version_parts(v: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut chars = v.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    s.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if let Ok(n) = s.parse::<u64>() {
                parts.push(Part::Num(n));
            }
        } else if c.is_alphabetic() {
            let mut s = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_alphabetic() {
                    s.push(c.to_ascii_lowercase());
                    chars.next();
                } else {
                    break;
                }
            }
            parts.push(Part::Alpha(s));
        } else {
            chars.next();
        }
    }
    parts
}

pub fn cmp_version(a: &str, b: &str) -> Ordering {
    let pa = parse_version_parts(a);
    let pb = parse_version_parts(b);
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i);
        let y = pb.get(i);
        let o = match (x, y) {
            (Some(Part::Num(x)), Some(Part::Num(y))) => x.cmp(y),
            (Some(Part::Alpha(x)), Some(Part::Alpha(y))) => x.cmp(y),
            // Mixed types at an aligned position: numeric sorts before
            // alphabetic for a deterministic total order.
            (Some(Part::Num(_)), Some(Part::Alpha(_))) => Ordering::Less,
            (Some(Part::Alpha(_)), Some(Part::Num(_))) => Ordering::Greater,
            // One side ran out of tokens: a numeric remainder pads the
            // missing side with 0 (so "1.2.3" > "1.2"); an alphabetic
            // remainder is a pre-release suffix that sorts before the
            // side with nothing more (so "1.0rc1" < "1.0").
            (Some(Part::Num(x)), None) => x.cmp(&0),
            (None, Some(Part::Num(y))) => 0u64.cmp(y),
            (Some(Part::Alpha(_)), None) => Ordering::Less,
            (None, Some(Part::Alpha(_))) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        match o {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

/// Version requirements accept exact equality, ordered comparisons, bare exact
/// versions, and comma-separated conjunctions of those clauses.
///
/// A compound requirement matches only if **every** non-empty clause matches.
pub fn matches_req(version: &str, req: &str) -> bool {
    let req = req.trim();
    if req.is_empty() {
        return true;
    }
    // Compound AND ranges: split on commas, require every clause.
    if req.contains(',') {
        return req
            .split(',')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .all(|clause| matches_req_single(version, clause));
    }
    matches_req_single(version, req)
}

/// Single clause: one leading operator or bare exact version.
fn matches_req_single(version: &str, req: &str) -> bool {
    let req = req.trim();
    if req.is_empty() {
        return true;
    }
    if let Some(rest) = req.strip_prefix("==") {
        return cmp_version(version, rest.trim()) == Ordering::Equal;
    }
    if let Some(rest) = req.strip_prefix(">=") {
        return matches!(
            cmp_version(version, rest.trim()),
            Ordering::Equal | Ordering::Greater
        );
    }
    if let Some(rest) = req.strip_prefix(">") {
        return cmp_version(version, rest.trim()) == Ordering::Greater;
    }
    if let Some(rest) = req.strip_prefix("<=") {
        return matches!(
            cmp_version(version, rest.trim()),
            Ordering::Equal | Ordering::Less
        );
    }
    if let Some(rest) = req.strip_prefix('<') {
        return cmp_version(version, rest.trim()) == Ordering::Less;
    }
    // bare exact
    cmp_version(version, req) == Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gromacs_versions_order() {
        assert_eq!(cmp_version("2024.1", "2024.4"), Ordering::Less);
        assert_eq!(cmp_version("2025.0", "2024.4"), Ordering::Greater);
        assert_eq!(cmp_version("4.1.6", "4.1.5"), Ordering::Greater);
    }

    #[test]
    fn req_ops() {
        assert!(matches_req("4.1.6", ">=4.1.6"));
        assert!(!matches_req("4.1.5", ">=4.1.6"));
        assert!(matches_req("0.3.24", "==0.3.24"));
        assert!(!matches_req("0.3.27", "==0.3.24"));
        // bare exact still exact
        assert!(matches_req("1.2.3", "1.2.3"));
        assert!(!matches_req("1.2.4", "1.2.3"));
    }

    #[test]
    fn compound_and_ranges() {
        // Classic half-open minor range.
        let req = ">=4.1.0,<4.2.0";
        assert!(matches_req("4.1.0", req));
        assert!(matches_req("4.1.5", req));
        assert!(matches_req("4.1.99", req));
        assert!(!matches_req("4.0.9", req), "below lower bound");
        assert!(!matches_req("4.2.0", req), "at exclusive upper bound");
        assert!(!matches_req("5.0.0", req), "above upper bound");
        // Whitespace around commas is fine.
        assert!(matches_req("4.1.6", ">=4.1.0, <4.2.0"));
        // Three clauses AND: every clause must hold.
        assert!(matches_req("2.0", ">=1.0,<=3.0,==2.0"));
        assert!(!matches_req("2.1", ">=1.0,<=3.0,==2.0"));
        // Single-op path unchanged when no comma.
        assert!(matches_req("4.1.6", ">=4.1.6"));
        assert!(!matches_req("4.1.5", ">=4.1.6"));
    }

    #[test]
    fn alpha_suffix_breaks_numeric_tie() {
        // A trailing alpha letter after an equal numeric prefix breaks
        // the tie alphabetically instead of comparing equal.
        assert_eq!(cmp_version("2025a", "2025b"), Ordering::Less);
        assert_eq!(cmp_version("2025b", "2025a"), Ordering::Greater);
        assert_ne!(cmp_version("2025a", "2025b"), Ordering::Equal);
    }

    #[test]
    fn pre_release_sorts_before_final_release() {
        // `rc`, `alpha`, `beta` markers sort before the final release.
        assert_eq!(cmp_version("1.0rc1", "1.0"), Ordering::Less);
        assert_eq!(cmp_version("1.0", "1.0rc1"), Ordering::Greater);
        assert_eq!(cmp_version("2.3.0alpha1", "2.3.0"), Ordering::Less);
        assert_eq!(cmp_version("2.3.0beta2", "2.3.0"), Ordering::Less);
    }

    #[test]
    fn bare_trailing_letter_is_pre_release_of_full_release() {
        // A bare trailing letter after a complete numeric release is
        // treated the same as an explicit pre-release marker: it sorts
        // before the corresponding final release.
        assert_eq!(cmp_version("1.2.3a", "1.2.3"), Ordering::Less);
        assert_eq!(cmp_version("1.2.3", "1.2.3a"), Ordering::Greater);
    }

    #[test]
    fn numeric_padding_still_applies_without_alpha() {
        // Pure numeric extensions keep the original zero-padding
        // behaviour: a longer numeric tail is only greater if it is
        // nonzero.
        assert_eq!(cmp_version("1.2.3", "1.2"), Ordering::Greater);
        assert_eq!(cmp_version("1.2.0", "1.2"), Ordering::Equal);
    }
}
