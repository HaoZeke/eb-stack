//! EasyBuild's `--filter-deps` / `EASYBUILD_FILTER_DEPS` matching.
//!
//! A filtered dependency is dropped before the robot walks it, so the
//! dependency and every module reachable only through it are never considered.
//! The rules are `EasyConfig.parse_filter_deps`, `parse_version_range`, and
//! `dep_is_filtered` in `easybuild/framework/easyconfig/easyconfig.py`,
//! including the `name=version` and `name=[low:high]` forms. Version order is
//! `easybuild.tools.LooseVersion`.

use crate::domain::{Candidate, DepReq};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// Why a `--filter-deps` entry cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterDepsError {
    /// The entry that failed.
    pub spec: String,
    /// What is wrong with it, in EasyBuild's wording.
    pub reason: String,
}

impl std::fmt::Display for FilterDepsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.reason, self.spec)
    }
}

impl std::error::Error for FilterDepsError {}

/// One parsed `--filter-deps` list.
///
/// The last entry for a name wins, as it does when EasyBuild stores the
/// specs in a dict keyed by dependency name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilterDeps {
    by_name: BTreeMap<String, VersionFilter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VersionFilter {
    /// A bare name: every version is filtered.
    Always,
    /// `name=version` or `name=<range>`.
    Range(VersionRange),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionRange {
    lower: String,
    upper: String,
    excl_lower: bool,
    excl_upper: bool,
}

impl FilterDeps {
    /// No dependencies are filtered.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether the list names nothing.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Parse EasyBuild's filter list.
    ///
    /// Each entry is a software name, `name=version`, or `name=` followed by
    /// a version range. A range uses `:` between the limits. `[` and `]`
    /// include a limit; `]` before the lower limit and `[` after the upper
    /// limit exclude it. Either limit may be empty.
    pub fn parse(specs: &[String]) -> Result<Self, FilterDepsError> {
        let mut by_name = BTreeMap::new();
        for raw in specs {
            let spec = raw.trim();
            if spec.is_empty() {
                continue;
            }
            match split_name_version(spec)? {
                None => {
                    by_name.insert(spec.to_string(), VersionFilter::Always);
                }
                Some((name, version_spec)) => {
                    // An empty version spec makes both limits falsy in
                    // `dep_is_filtered`, which leaves the dependency filtered.
                    let filter = if version_spec.is_empty() {
                        VersionFilter::Always
                    } else {
                        VersionFilter::Range(parse_version_range(version_spec)?)
                    };
                    by_name.insert(name.to_string(), filter);
                }
            }
        }
        Ok(Self { by_name })
    }

    /// Whether a dependency of this name and version is filtered.
    ///
    /// `version` is the version the dependency tuple states, not a solved
    /// module version. An empty version is filtered only by a bare name.
    pub fn dep_is_filtered(&self, name: &str, version: &str) -> bool {
        let Some(filter) = self.by_name.get(name) else {
            return false;
        };
        match filter {
            VersionFilter::Always => true,
            VersionFilter::Range(range) => version_in_range(version, range),
        }
    }

    /// Drop filtered runtime and build dependencies. What remains is what the
    /// robot is allowed to walk.
    pub fn apply(&self, candidates: &mut [Candidate]) {
        if self.is_empty() {
            return;
        }
        for candidate in candidates {
            candidate
                .dependencies
                .retain(|dep| !self.dep_is_filtered(&dep.name, dep_version(dep)));
            candidate
                .builddependencies
                .retain(|dep| !self.dep_is_filtered(&dep.name, dep_version(dep)));
        }
    }
}

/// Split `name=version` on the one `=`. A bare name is `None`.
///
/// More than one `=` is the error `parse_filter_deps` raises. An empty
/// version (`name=`) is kept: EasyBuild stores it, and both limits are then
/// falsy, so every version of that name is filtered.
fn split_name_version(spec: &str) -> Result<Option<(&str, &str)>, FilterDepsError> {
    let equals = spec.bytes().filter(|byte| *byte == b'=').count();
    if equals == 0 {
        return Ok(None);
    }
    if equals != 1 {
        return Err(FilterDepsError {
            spec: spec.to_string(),
            reason: "Incorrect specification for dependency to filter".into(),
        });
    }
    Ok(Some(spec.split_once('=').expect("one separator")))
}

fn parse_version_range(version_spec: &str) -> Result<VersionRange, FilterDepsError> {
    let range_sep = ':';
    if version_spec.contains(range_sep) {
        let limits = version_spec.replace(['[', ']'], "");
        let mut parts = limits.split(range_sep);
        let lower = parts.next().unwrap_or("").to_string();
        let upper = parts.next().unwrap_or("").to_string();
        if parts.next().is_some() {
            return Err(FilterDepsError {
                spec: version_spec.to_string(),
                reason: "Incorrect version range, expected lower/upper limit".into(),
            });
        }
        if !lower.is_empty() && !upper.is_empty() && loose_cmp(&lower, &upper) == Ordering::Greater
        {
            return Err(FilterDepsError {
                spec: version_spec.to_string(),
                reason: "Incorrect version range, found lower limit > higher limit".into(),
            });
        }
        Ok(VersionRange {
            lower,
            upper,
            excl_lower: version_spec.starts_with(']'),
            excl_upper: version_spec.ends_with('['),
        })
    } else {
        Ok(VersionRange {
            lower: version_spec.to_string(),
            upper: version_spec.to_string(),
            excl_lower: false,
            excl_upper: false,
        })
    }
}

fn version_in_range(version: &str, range: &VersionRange) -> bool {
    if version.is_empty() {
        return false;
    }
    // Assume the dependency is filtered, then reject it when it falls outside
    // either limit. That is the order `dep_is_filtered` uses.
    let mut filtered = true;
    let version = LooseVersion::parse(version);
    if !range.lower.is_empty() {
        let lower = LooseVersion::parse(&range.lower);
        if version < lower || (range.excl_lower && version == lower) {
            filtered = false;
        }
    }
    if !range.upper.is_empty() {
        let upper = LooseVersion::parse(&range.upper);
        if version > upper || (range.excl_upper && version == upper) {
            filtered = false;
        }
    }
    filtered
}

/// The version a dependency tuple states.
///
/// Parsed dependencies store an exact pin as `==version`. EasyBuild compares
/// the version field before that rewrite.
pub fn dep_version(dep: &DepReq) -> &str {
    dep.version_req
        .strip_prefix("==")
        .unwrap_or(&dep.version_req)
}

/// `easybuild.tools.LooseVersion`, the comparison `--filter-deps` uses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LooseVersion {
    components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Component {
    Num(u64),
    Text(String),
}

impl LooseVersion {
    fn parse(text: &str) -> Self {
        let mut components = Vec::new();
        let mut unmatched = String::new();
        let mut chars = text.chars().peekable();
        let flush = |unmatched: &mut String, components: &mut Vec<Component>| {
            if !unmatched.is_empty() {
                components.push(Component::Text(std::mem::take(unmatched)));
            }
        };
        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_digit() {
                flush(&mut unmatched, &mut components);
                let mut digits = String::new();
                while let Some(ch) = chars.peek().copied() {
                    if ch.is_ascii_digit() {
                        digits.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match digits.parse::<u64>() {
                    Ok(number) => components.push(Component::Num(number)),
                    Err(_) => components.push(Component::Text(digits)),
                }
            } else if ch.is_ascii_lowercase() {
                flush(&mut unmatched, &mut components);
                let mut letters = String::new();
                while let Some(ch) = chars.peek().copied() {
                    if ch.is_ascii_lowercase() {
                        letters.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                components.push(Component::Text(letters));
            } else if ch == '.' {
                flush(&mut unmatched, &mut components);
                chars.next();
            } else {
                unmatched.push(ch);
                chars.next();
            }
        }
        flush(&mut unmatched, &mut components);
        Self { components }
    }
}

impl PartialOrd for LooseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LooseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        let width = self.components.len().max(other.components.len());
        for index in 0..width {
            let left = padded(self.components.get(index), other.components.get(index));
            let right = padded(other.components.get(index), self.components.get(index));
            let order = match (left, right) {
                (Component::Num(left), Component::Num(right)) => left.cmp(&right),
                (Component::Text(left), Component::Text(right)) => left.cmp(&right),
                _ => unreachable!("mixed components are stringified before compare"),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    }
}

/// One side of a `LooseVersion._cmp` step.
///
/// A missing numeric component is `0`. A missing text component is `""`.
/// Two components of different types are both converted with `str()`.
fn padded(mine: Option<&Component>, other: Option<&Component>) -> Component {
    match (mine, other) {
        (None, Some(Component::Num(_)) | None) => Component::Num(0),
        (None, Some(Component::Text(_))) => Component::Text(String::new()),
        (Some(Component::Num(number)), None | Some(Component::Num(_))) => Component::Num(*number),
        (Some(Component::Text(text)), None | Some(Component::Text(_))) => {
            Component::Text(text.clone())
        }
        // Different component types compare as `str(component)`, which is what
        // `LooseVersion._cmp` does after `type(i) is type(j)` fails.
        (Some(left), Some(_)) => Component::Text(component_string(left)),
    }
}

fn component_string(component: &Component) -> String {
    match component {
        Component::Num(number) => number.to_string(),
        Component::Text(text) => text.clone(),
    }
}

fn loose_cmp(left: &str, right: &str) -> Ordering {
    LooseVersion::parse(left).cmp(&LooseVersion::parse(right))
}

/// Specs from the command line, or from `EASYBUILD_FILTER_DEPS` when the
/// command line did not pass any.
///
/// An explicit list wins, including a list that filters nothing only when it
/// is non-empty. The environment variable is comma-separated, matching the
/// value EESSI-extend exports.
pub fn specs_from_cli_or_env(cli: &[String]) -> Vec<String> {
    let cli: Vec<String> = cli
        .iter()
        .map(|spec| spec.trim().to_string())
        .filter(|spec| !spec.is_empty())
        .collect();
    if !cli.is_empty() {
        return cli;
    }
    match std::env::var("EASYBUILD_FILTER_DEPS") {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|spec| !spec.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specs(list: &str) -> FilterDeps {
        let parts: Vec<String> = list.split(',').map(str::to_string).collect();
        FilterDeps::parse(&parts).expect(list)
    }

    #[test]
    fn a_bare_name_filters_every_version_and_the_last_entry_wins() {
        let filter = specs("zlib,ncurses,zlib=1.2.8");
        assert!(!filter.dep_is_filtered("zlib", "1.2.11"));
        assert!(filter.dep_is_filtered("zlib", "1.2.8"));
        assert!(filter.dep_is_filtered("ncurses", "6.6"));
        assert!(!filter.dep_is_filtered("Bison", "3.8.2"));
    }

    #[test]
    fn version_ranges_follow_the_framework_examples() {
        let zlib = specs("zlib=[1.2:1.2.99]");
        assert!(zlib.dep_is_filtered("zlib", "1.2"));
        assert!(zlib.dep_is_filtered("zlib", "1.2.8"));
        assert!(zlib.dep_is_filtered("zlib", "1.2.99"));
        assert!(!zlib.dep_is_filtered("zlib", "1.1.9"));
        assert!(!zlib.dep_is_filtered("zlib", "1.3"));

        let ncurses = specs("ncurses=[5.0:");
        assert!(ncurses.dep_is_filtered("ncurses", "5.0"));
        assert!(ncurses.dep_is_filtered("ncurses", "6.6"));
        assert!(!ncurses.dep_is_filtered("ncurses", "4.9"));

        let cmake = specs("CMake=:4.0[");
        assert!(cmake.dep_is_filtered("CMake", "3.21.0"));
        assert!(!cmake.dep_is_filtered("CMake", "4.0"));
        assert!(!cmake.dep_is_filtered("CMake", "4.1"));
    }

    #[test]
    fn an_exact_version_uses_looseversion_equality() {
        let filter = specs("zlib=1.2.8");
        assert!(filter.dep_is_filtered("zlib", "1.2.8"));
        assert!(filter.dep_is_filtered("zlib", "1.2.08"));
        assert!(!filter.dep_is_filtered("zlib", "1.2.9"));
        // Trailing zeros compare equal: 1 == 1.0.
        assert!(specs("pkg=1").dep_is_filtered("pkg", "1.0"));
        assert!(specs("pkg=1.0").dep_is_filtered("pkg", "1"));
    }

    #[test]
    fn exclusive_limits_and_rejected_ranges() {
        let filter = specs("flex=]2.5.39:]");
        assert!(!filter.dep_is_filtered("flex", "2.5.39"));
        assert!(filter.dep_is_filtered("flex", "2.6.4"));

        let bad = FilterDeps::parse(&["zlib=2.0:1.0".into()]).unwrap_err();
        assert!(bad.reason.contains("lower limit > higher limit"), "{bad}");
        let bad = FilterDeps::parse(&["zlib=1:2:3".into()]).unwrap_err();
        assert!(bad.reason.contains("lower/upper limit"), "{bad}");
        let bad = FilterDeps::parse(&["zlib=1.2=3".into()]).unwrap_err();
        assert!(bad.reason.contains("Incorrect specification"), "{bad}");
        // `name=` has an empty version spec. EasyBuild treats both limits as
        // absent and filters every version.
        assert!(specs("zlib=").dep_is_filtered("zlib", "1.3"));
    }

    #[test]
    fn looseversion_orders_numbers_numerically_and_letters_after() {
        assert_eq!(loose_cmp("1.10", "1.9"), Ordering::Greater);
        assert_eq!(loose_cmp("1.0a", "1.0"), Ordering::Greater);
        assert_eq!(loose_cmp("1.0", "1"), Ordering::Equal);
    }

    #[test]
    fn applying_a_filter_drops_the_dependency_itself() {
        let mut candidates = vec![Candidate {
            name: "App".into(),
            version: "1".into(),
            toolchain: crate::domain::Toolchain {
                name: "system".into(),
                version: "system".into(),
            },
            versionsuffix: None,
            easyconfig_path: "App.eb".into(),
            dependencies: vec![DepReq {
                name: "zlib".into(),
                version_req: "==1.2.8".into(),
                versionsuffix: None,
                toolchain: None,
            }],
            builddependencies: vec![DepReq {
                name: "CMake".into(),
                version_req: "==3.21.0".into(),
                versionsuffix: None,
                toolchain: None,
            }],
            exts_list: Vec::new(),
            easyblock: None,
            moduleclass: None,
        }];
        specs("zlib=[1.2:1.2.99],CMake=:4.0[").apply(&mut candidates);
        assert!(candidates[0].dependencies.is_empty());
        assert!(candidates[0].builddependencies.is_empty());
    }
}
