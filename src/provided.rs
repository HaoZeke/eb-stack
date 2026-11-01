//! Fail when a build list names software a baseline already provides.
//!
//! A site layer is the software EESSI does not install. `stack solve
//! --build-list-excludes-baseline` writes that list. A later edit can put
//! an EESSI module back on it. `stack provided` reads the list and the
//! baseline (an installed-module listing, a stack lock, or both) and fails
//! when a line's package name is one the baseline has, ignoring version
//! and toolchain. `--allow` keeps a second version the site was asked for.

use crate::cli_parse::usage;
use crate::domain::StackLock;
use crate::gap::{names_from_listing, software_name};
use crate::jenkins::parse_build_list;
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Arguments for `stack provided`.
pub struct ProvidedRequest {
    /// Jenkins or path-style build lists. Repeatable.
    pub build_lists: Vec<PathBuf>,
    /// Installed-module listings, `Name/Version` or a bare name. Repeatable.
    pub installed: Vec<PathBuf>,
    /// Stack lock whose package names count as provided.
    pub baseline_lock: Option<PathBuf>,
    /// Package names that may stay on the list. Matched without regard to case.
    pub allow: Vec<String>,
    /// Print the report as JSON.
    pub json: bool,
}

/// One build-list line whose package the baseline already provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvidedHit {
    /// Build list the line came from.
    pub list: String,
    /// The line's easyconfig, as written.
    pub file: String,
    /// Package name parsed from that filename.
    pub name: String,
}

/// One build-list line that names no package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnparsedLine {
    /// Build list the line came from.
    pub list: String,
    /// The line's easyconfig, as written.
    pub file: String,
}

/// What `stack provided` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvidedReport {
    /// Lines whose package the baseline provides and `--allow` does not name.
    pub hits: Vec<ProvidedHit>,
    /// `.eb` lines whose filename names no package.
    pub unparsed: Vec<UnparsedLine>,
    /// Lines `--allow` kept off `hits`.
    pub allowed: usize,
    /// Distinct package names the baseline provides.
    pub provided: usize,
    /// Text for stdout. JSON when the request asked for it.
    #[serde(skip)]
    pub stdout: String,
}

impl ProvidedReport {
    /// The stderr line when the check failed, or `None` when it passed.
    pub fn failure(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.hits.is_empty() {
            let n = self.hits.len();
            parts.push(format!(
                "{n} build-list {} {} software the baseline already provides",
                plural(n, "line", "lines"),
                plural(n, "names", "name"),
            ));
        }
        if !self.unparsed.is_empty() {
            let n = self.unparsed.len();
            parts.push(format!(
                "{n} build-list {} {} no package",
                plural(n, "line", "lines"),
                plural(n, "names", "name"),
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    }
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// Read the lists and compare each line's package name with the baseline.
pub fn run_provided(args: &ProvidedRequest) -> Result<ProvidedReport> {
    if args.build_lists.is_empty() {
        return Err(usage("pass at least one --build-list"));
    }
    if args.installed.is_empty() && args.baseline_lock.is_none() {
        return Err(usage(
            "pass --installed or --baseline-lock so the check knows what the baseline provides",
        ));
    }
    let mut provided = BTreeSet::new();
    for path in &args.installed {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--installed {}: not readable", path.display()))?;
        provided.extend(
            names_from_listing(&text)
                .into_iter()
                .map(|name| name.to_lowercase()),
        );
    }
    if let Some(path) = &args.baseline_lock {
        let lock: StackLock = crate::load_json_file(path)
            .with_context(|| format!("--baseline-lock {}", path.display()))?;
        provided.extend(
            lock.packages
                .iter()
                .map(|package| package.name.to_lowercase()),
        );
    }
    let allow: BTreeSet<String> = args.allow.iter().map(|name| name.to_lowercase()).collect();
    let mut hits = Vec::new();
    let mut unparsed = Vec::new();
    let mut allowed = 0usize;
    for path in &args.build_lists {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("--build-list {}: not readable", path.display()))?;
        classify_list(
            &path.display().to_string(),
            &text,
            &provided,
            &allow,
            &mut hits,
            &mut unparsed,
            &mut allowed,
        );
    }
    let report = ProvidedReport {
        hits,
        unparsed,
        allowed,
        provided: provided.len(),
        stdout: String::new(),
    };
    let stdout = if args.json {
        serde_json::to_string_pretty(&report).context("provided report as JSON")? + "\n"
    } else {
        render_text(&report)
    };
    Ok(ProvidedReport { stdout, ..report })
}

fn classify_list(
    list: &str,
    text: &str,
    provided: &BTreeSet<String>,
    allow: &BTreeSet<String>,
    hits: &mut Vec<ProvidedHit>,
    unparsed: &mut Vec<UnparsedLine>,
    allowed: &mut usize,
) {
    for line in parse_build_list(text) {
        let Some(name) = software_name(&line.file) else {
            unparsed.push(UnparsedLine {
                list: list.to_string(),
                file: line.file,
            });
            continue;
        };
        if !provided.contains(&name.to_lowercase()) {
            continue;
        }
        if allow.contains(&name.to_lowercase()) {
            *allowed += 1;
            continue;
        }
        hits.push(ProvidedHit {
            list: list.to_string(),
            file: line.file,
            name,
        });
    }
}

fn render_text(report: &ProvidedReport) -> String {
    let mut out = String::new();
    for hit in &report.hits {
        out.push_str(&format!(
            "provided {} name={} list={}\n",
            hit.file, hit.name, hit.list
        ));
    }
    for line in &report.unparsed {
        out.push_str(&format!("unparsed {} list={}\n", line.file, line.list));
    }
    out.push_str(&format!(
        "hits={} unparsed={} allowed={} provided={}\n",
        report.hits.len(),
        report.unparsed.len(),
        report.allowed,
        report.provided
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(text: &str, provided: &[&str], allow: &[&str]) -> ProvidedReport {
        let provided: BTreeSet<String> = provided.iter().map(|name| name.to_lowercase()).collect();
        let allow: BTreeSet<String> = allow.iter().map(|name| name.to_lowercase()).collect();
        let mut hits = Vec::new();
        let mut unparsed = Vec::new();
        let mut allowed_count = 0;
        classify_list(
            "site.list",
            text,
            &provided,
            &allow,
            &mut hits,
            &mut unparsed,
            &mut allowed_count,
        );
        ProvidedReport {
            hits,
            unparsed,
            allowed: allowed_count,
            provided: provided.len(),
            stdout: String::new(),
        }
    }

    #[test]
    fn a_name_the_baseline_has_fails_and_a_second_version_can_be_allowed() {
        let text = "\
# kept off the list
# FFTW-3.3.10-gompi-2026.1.eb
GROMACS-2026.0-foss-2026.1.eb
FFTW-3.3.10-gompi-2026.1.eb --hooks=/sw/hooks/x.py
fixtures/foss-2025b/Python-3.12.3-foss-2025b.eb
Vendor-1.2.eb
not-a-version.eb
";
        let report = classify(text, &["GROMACS", "fftw", "Python"], &["gromacs"]);
        assert_eq!(report.hits.len(), 2, "{:?}", report.hits);
        assert_eq!(report.hits[0].name, "FFTW");
        assert_eq!(report.hits[1].name, "Python");
        assert_eq!(
            report.hits[1].file,
            "fixtures/foss-2025b/Python-3.12.3-foss-2025b.eb"
        );
        assert_eq!(report.allowed, 1);
        assert_eq!(report.unparsed.len(), 1);
        assert_eq!(report.unparsed[0].file, "not-a-version.eb");
        assert!(report
            .failure()
            .unwrap()
            .contains("2 build-list lines name"));
        assert!(report
            .failure()
            .unwrap()
            .contains("1 build-list line names no package"));
    }

    #[test]
    fn a_list_of_site_only_names_passes() {
        let report = classify("VASP-6.5.1.eb\n", &["GROMACS"], &[]);
        assert!(report.hits.is_empty());
        assert!(report.unparsed.is_empty());
        assert!(report.failure().is_none());
    }
}
