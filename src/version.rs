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
    /// A run of digits, compared numerically.
    Num(u64),
    /// A run of letters, lowercased and compared lexically.
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

/// Order two version strings by their tokenized parts, digits before letters.
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

/// One comparison in a requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequirementOp {
    /// `==X` or a bare `X`.
    Exact,
    /// `!=X`.
    NotEqual,
    /// `>=X`.
    AtLeast,
    /// `>X`.
    Above,
    /// `<=X`.
    AtMost,
    /// `<X`.
    Below,
    /// `~=X`, PEP 440 compatible release.
    Compatible,
    /// `^X`, Cargo caret.
    Caret,
    /// `~X`, Cargo tilde.
    Tilde,
}

impl RequirementOp {
    /// Whether this operator puts a floor under the version.
    ///
    /// The emitted-extension path needs a concrete version, and for every
    /// operator that bounds from below the floor is the version named.
    fn is_lower_bound(self) -> bool {
        matches!(
            self,
            Self::Exact
                | Self::AtLeast
                | Self::Above
                | Self::Compatible
                | Self::Caret
                | Self::Tilde
        )
    }
}

/// One parsed clause: an operator and the version it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementClause {
    /// The comparison.
    pub op: RequirementOp,
    /// The version on the right-hand side.
    pub version: String,
}

/// A parsed requirement: a conjunction of clauses.
///
/// This is the single interpretation of a constraint string. The solver asks
/// it what a candidate satisfies and the emitter asks it for a version, so the
/// two cannot drift apart the way two hand-rolled readers of the same syntax
/// did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Requirement {
    /// Every clause, all of which must hold.
    pub clauses: Vec<RequirementClause>,
}

/// A clause no operator in the language covers.
///
/// Returned rather than silently answering "matches nothing", which is what an
/// unknown operator used to do: it fell through to a bare-exact comparison
/// against a string starting with punctuation, so the version set came back
/// empty and the dependency looked unsatisfiable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedRequirement {
    /// The clause that could not be parsed.
    pub clause: String,
    /// Why, in one line.
    pub reason: String,
}

impl std::fmt::Display for UnsupportedRequirement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.clause, self.reason)
    }
}

impl std::error::Error for UnsupportedRequirement {}

/// Parse a requirement, or say which clause is not expressible.
pub fn parse_requirement(req: &str) -> Result<Requirement, UnsupportedRequirement> {
    let mut clauses = Vec::new();
    for clause in req.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        clauses.push(parse_clause(clause)?);
    }
    Ok(Requirement { clauses })
}

fn parse_clause(clause: &str) -> Result<RequirementClause, UnsupportedRequirement> {
    // Two-character operators first, so `~=` is never read as `~`.
    for (prefix, op) in [
        ("~=", RequirementOp::Compatible),
        ("!=", RequirementOp::NotEqual),
        ("==", RequirementOp::Exact),
        (">=", RequirementOp::AtLeast),
        ("<=", RequirementOp::AtMost),
        ("^", RequirementOp::Caret),
        ("~", RequirementOp::Tilde),
        (">", RequirementOp::Above),
        ("<", RequirementOp::Below),
    ] {
        if let Some(rest) = clause.strip_prefix(prefix) {
            let version = rest.trim().to_string();
            if version.is_empty() {
                return Err(UnsupportedRequirement {
                    clause: clause.to_string(),
                    reason: format!("{prefix} without a version"),
                });
            }
            return Ok(RequirementClause { op, version });
        }
    }
    if clause.starts_with(|character: char| character.is_ascii_digit()) {
        return Ok(RequirementClause {
            op: RequirementOp::Exact,
            version: clause.to_string(),
        });
    }
    Err(UnsupportedRequirement {
        clause: clause.to_string(),
        reason: "no operator this language covers, and not a bare version".into(),
    })
}

impl Requirement {
    /// Whether a version satisfies every clause.
    pub fn matches(&self, version: &str) -> bool {
        self.clauses
            .iter()
            .all(|clause| clause_matches(version, clause))
    }

    /// The exact version this requirement names, when it names one.
    pub fn exact(&self) -> Option<&str> {
        self.clauses
            .iter()
            .find(|clause| clause.op == RequirementOp::Exact)
            .map(|clause| clause.version.as_str())
    }

    /// The lowest version this requirement admits, when it has a floor.
    ///
    /// An extension entry needs one concrete version, and for a lower-bounded
    /// requirement the floor is the honest choice: it is the version the
    /// foreign metadata actually named.
    pub fn lower_bound(&self) -> Option<&str> {
        self.exact().or_else(|| {
            self.clauses
                .iter()
                .find(|clause| clause.op.is_lower_bound())
                .map(|clause| clause.version.as_str())
        })
    }
}

fn clause_matches(version: &str, clause: &RequirementClause) -> bool {
    let right = clause.version.as_str();
    match clause.op {
        RequirementOp::Exact => cmp_version(version, right) == Ordering::Equal,
        RequirementOp::NotEqual => cmp_version(version, right) != Ordering::Equal,
        RequirementOp::AtLeast => matches!(
            cmp_version(version, right),
            Ordering::Equal | Ordering::Greater
        ),
        RequirementOp::Above => cmp_version(version, right) == Ordering::Greater,
        RequirementOp::AtMost => matches!(
            cmp_version(version, right),
            Ordering::Equal | Ordering::Less
        ),
        RequirementOp::Below => cmp_version(version, right) == Ordering::Less,
        RequirementOp::Compatible => matches_compatible_release(version, right),
        RequirementOp::Caret => matches_caret(version, right),
        RequirementOp::Tilde => matches_tilde(version, right),
    }
}

/// Version requirements accept exact equality, ordered comparisons, bare exact
/// versions, and comma-separated conjunctions of those clauses.
///
/// A compound requirement matches only if **every** non-empty clause matches.
pub fn matches_req(version: &str, req: &str) -> bool {
    // A clause the language cannot express excludes nothing rather than
    // everything. Ingestion turns the same parse failure into a visible
    // residual, so the constraint is reported rather than quietly enforced as
    // an empty version set.
    match parse_requirement(req) {
        Ok(requirement) => requirement.matches(version),
        Err(_) => true,
    }
}

/// Numeric components of a version, stopping at the first non-numeric run.
///
/// `1.2.3rc1` yields `[1, 2, 3]`, which is what the range operators below need:
/// they bound a release series, and a pre-release suffix does not move the
/// bound.
fn numeric_components(version: &str) -> Vec<u64> {
    let mut components = Vec::new();
    for field in version.split('.') {
        let digits: String = field.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        match digits.parse::<u64>() {
            Ok(value) => components.push(value),
            Err(_) => break,
        }
    }
    components
}

/// Upper bound of a release series, as `[components…]` with `index` bumped and
/// everything after it dropped.
fn series_ceiling(components: &[u64], index: usize) -> String {
    let mut bound: Vec<u64> = components[..=index].to_vec();
    bound[index] += 1;
    bound
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// `~=X.Y.Z` is `>=X.Y.Z, <X.(Y+1)`; `~=X.Y` is `>=X.Y, <(X+1)`.
fn matches_compatible_release(version: &str, floor: &str) -> bool {
    if cmp_version(version, floor) == Ordering::Less {
        return false;
    }
    let components = numeric_components(floor);
    if components.len() < 2 {
        // `~=X` is not a valid compatible release, so only the floor applies.
        return true;
    }
    let ceiling = series_ceiling(&components, components.len() - 2);
    cmp_version(version, &ceiling) == Ordering::Less
}

/// `^X.Y.Z` allows anything up to the next change of the left-most non-zero
/// component: `^1.2.3` is `<2`, `^0.2.3` is `<0.3`, `^0.0.3` is `<0.0.4`.
fn matches_caret(version: &str, floor: &str) -> bool {
    if cmp_version(version, floor) == Ordering::Less {
        return false;
    }
    let components = numeric_components(floor);
    if components.is_empty() {
        return true;
    }
    let significant = components
        .iter()
        .position(|component| *component != 0)
        .unwrap_or(components.len() - 1);
    let ceiling = series_ceiling(&components, significant);
    cmp_version(version, &ceiling) == Ordering::Less
}

/// `~X.Y.Z` and `~X.Y` allow patch-level changes, `~X` minor-level ones.
fn matches_tilde(version: &str, floor: &str) -> bool {
    if cmp_version(version, floor) == Ordering::Less {
        return false;
    }
    let components = numeric_components(floor);
    if components.is_empty() {
        return true;
    }
    let index = if components.len() >= 2 { 1 } else { 0 };
    let ceiling = series_ceiling(&components, index);
    cmp_version(version, &ceiling) == Ordering::Less
}

#[cfg(test)]
mod ecosystem_operator_tests {
    use super::*;

    #[test]
    fn exclusion_excludes_only_the_named_version() {
        assert!(matches_req("1.3.0", "!=1.2.0"));
        assert!(!matches_req("1.2.0", "!=1.2.0"));
        // The form that made every PyPI dependency carrying one unsatisfiable.
        assert!(matches_req("2.1.3", ">=1.0,!=2.0.0"));
        assert!(!matches_req("2.0.0", ">=1.0,!=2.0.0"));
    }

    #[test]
    fn compatible_release_bounds_the_series() {
        assert!(matches_req("1.2.5", "~=1.2.0"));
        assert!(!matches_req("1.3.0", "~=1.2.0"));
        assert!(!matches_req("1.1.9", "~=1.2.0"));
        assert!(matches_req("1.9.0", "~=1.2"));
        assert!(!matches_req("2.0.0", "~=1.2"));
    }

    #[test]
    fn caret_stops_at_the_leftmost_nonzero_component() {
        assert!(matches_req("1.5.0", "^1.2.0"));
        assert!(!matches_req("2.0.0", "^1.2.0"));
        assert!(matches_req("0.2.9", "^0.2.3"));
        assert!(!matches_req("0.3.0", "^0.2.3"));
        assert!(matches_req("0.0.3", "^0.0.3"));
        assert!(!matches_req("0.0.4", "^0.0.3"));
    }

    #[test]
    fn tilde_allows_the_last_named_component_to_move() {
        assert!(matches_req("1.2.9", "~1.2.3"));
        assert!(!matches_req("1.3.0", "~1.2.3"));
        assert!(matches_req("1.2.9", "~1.2"));
        assert!(!matches_req("1.3.0", "~1.2"));
        assert!(matches_req("1.9.0", "~1"));
        assert!(!matches_req("2.0.0", "~1"));
    }

    #[test]
    fn a_prerelease_suffix_does_not_move_the_bound() {
        assert!(matches_req("1.2.3rc1", "^1.2.0"));
        assert!(matches_req("1.2.3", "~=1.2.0"));
    }
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
