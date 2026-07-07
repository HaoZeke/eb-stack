//! Simple version comparison for EasyBuild-style versions (X.Y.Z or year.Z).

use std::cmp::Ordering;

/// Parse a version string into numeric components (non-numeric suffixes ignored after split).
pub fn parse_version_parts(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u64>().ok())
        .collect()
}

pub fn cmp_version(a: &str, b: &str) -> Ordering {
    let pa = parse_version_parts(a);
    let pb = parse_version_parts(b);
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

/// version_req grammar (v1): `==X`, `>=X`, `>X`, `<=X`, `<X`, or bare `X` (exact).
pub fn matches_req(version: &str, req: &str) -> bool {
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
    }
}
