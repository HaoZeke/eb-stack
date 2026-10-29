//! What a site provides that a stack does not, and the recipe that would close it.
//!
//! A site's module tree names the software its users load. A stack such as an
//! EESSI version names what it installs. For every package the site has and
//! the stack lacks, the gap says whether the easyconfig trees carry a recipe
//! on the stack's toolchain generation, and which: the newest version on the
//! generation's toolchains or on the system toolchain. Those recipes are the
//! roots `stack batch` plans pull requests from.

use crate::domain::Candidate;
use crate::hierarchy::is_system_toolchain;
use crate::version::cmp_version;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// One site package and what closing it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GapRow {
    /// EasyBuild name, as the site's module tree spells it.
    pub name: String,
    /// `in-stack`, `recipe`, `no-recipe`, or `excluded`.
    pub route: String,
    /// The easyconfig on the generation, for `recipe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// The newest easyconfig of the name on any toolchain, for `no-recipe`:
    /// the source a bump starts from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_elsewhere: Option<String>,
    /// Why a name is `excluded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Package names from a module listing: `Name/Version` lines, bare names, or
/// `Name` directories. Blank lines and `#` comments are skipped.
pub fn names_from_listing(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split('/').next())
        .map(|n| n.trim_end_matches(".lua").to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

fn file_of(candidate: &Candidate) -> String {
    candidate
        .easyconfig_path
        .rsplit('/')
        .next()
        .unwrap_or(&candidate.easyconfig_path)
        .to_string()
}

fn toolchain_label(candidate: &Candidate) -> String {
    format!(
        "{}-{}",
        candidate.toolchain.name, candidate.toolchain.version
    )
}

/// Order candidates newest first: version, then a recipe without a
/// versionsuffix before one with, then by file name.
fn newest_first(a: &&Candidate, b: &&Candidate) -> Ordering {
    cmp_version(&b.version, &a.version)
        .then_with(|| {
            a.versionsuffix
                .as_deref()
                .unwrap_or("")
                .len()
                .cmp(&b.versionsuffix.as_deref().unwrap_or("").len())
        })
        .then_with(|| file_of(a).cmp(&file_of(b)))
}

/// The gap between `site` and `stack`.
///
/// `stack` names are matched without regard to case, as Lmod does. `members`
/// are the generation's toolchain labels (`GCCcore-15.2.0`, `foss-2026.1`).
/// `excluded` maps a name to the reason it is not the stack's to carry
/// (licensed, vendor, site portal) or is filtered to the compatibility layer.
pub fn gap(
    site: &BTreeSet<String>,
    stack: &BTreeSet<String>,
    candidates: &[Candidate],
    members: &[String],
    excluded: &BTreeMap<String, String>,
) -> Vec<GapRow> {
    let stack_lower: BTreeSet<String> = stack.iter().map(|n| n.to_lowercase()).collect();
    let mut by_name: BTreeMap<&str, Vec<&Candidate>> = BTreeMap::new();
    for c in candidates {
        by_name.entry(c.name.as_str()).or_default().push(c);
    }
    let mut rows = Vec::new();
    for name in site {
        if stack_lower.contains(&name.to_lowercase()) {
            rows.push(GapRow {
                name: name.clone(),
                route: "in-stack".into(),
                recipe: None,
                newest_elsewhere: None,
                reason: None,
            });
            continue;
        }
        if let Some(reason) = excluded.get(name) {
            rows.push(GapRow {
                name: name.clone(),
                route: "excluded".into(),
                recipe: None,
                newest_elsewhere: None,
                reason: Some(reason.clone()),
            });
            continue;
        }
        let mut all: Vec<&Candidate> = by_name.get(name.as_str()).cloned().unwrap_or_default();
        all.sort_by(newest_first);
        let on_generation = all.iter().find(|c| {
            is_system_toolchain(&c.toolchain) || members.iter().any(|m| *m == toolchain_label(c))
        });
        match on_generation {
            Some(c) => rows.push(GapRow {
                name: name.clone(),
                route: "recipe".into(),
                recipe: Some(file_of(c)),
                newest_elsewhere: None,
                reason: None,
            }),
            None => rows.push(GapRow {
                name: name.clone(),
                route: "no-recipe".into(),
                recipe: None,
                newest_elsewhere: all.first().map(|c| file_of(c)),
                reason: None,
            }),
        }
    }
    rows
}

/// The gap as a tab-separated table with a header line.
pub fn render_tsv(rows: &[GapRow]) -> String {
    let mut out = String::from("name\troute\trecipe\tnewest_elsewhere\treason\n");
    for r in rows {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            r.name,
            r.route,
            r.recipe.as_deref().unwrap_or("-"),
            r.newest_elsewhere.as_deref().unwrap_or("-"),
            r.reason.as_deref().unwrap_or("-")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Toolchain;

    fn cand(name: &str, version: &str, tc: (&str, &str), suffix: Option<&str>) -> Candidate {
        let tcs = if tc.0 == "system" {
            String::new()
        } else {
            format!("-{}-{}", tc.0, tc.1)
        };
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: Toolchain {
                name: tc.0.into(),
                version: tc.1.into(),
            },
            versionsuffix: suffix.map(String::from),
            easyconfig_path: format!("/ec/{name}-{version}{tcs}{}.eb", suffix.unwrap_or("")),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            easyblock: None,
            moduleclass: None,
        }
    }

    fn members() -> Vec<String> {
        [
            "system",
            "GCCcore-15.2.0",
            "GCC-15.2.0",
            "gompi-2026.1",
            "foss-2026.1",
        ]
        .map(String::from)
        .to_vec()
    }

    #[test]
    fn routes_each_site_package() {
        let site = names_from_listing("BLAST+/2.16.0-gompi-2024a\nLAMMPS\nJulia/1.10.4\nCP2K/2025.2-foss-2025a\nMATLAB/2025b\nzlib/1.3.1\n");
        let stack = names_from_listing("zlib/1.3.1-GCCcore-15.2.0\n");
        let candidates = vec![
            cand("BLAST+", "2.17.0", ("gompi", "2026.1"), None),
            cand("BLAST+", "2.16.0", ("gompi", "2024a"), None),
            cand("Julia", "1.13.2", ("system", ""), None),
            cand("CP2K", "2025.2", ("foss", "2025b"), None),
            cand("CP2K", "2023.1", ("foss", "2023a"), None),
        ];
        let excluded = BTreeMap::from([("MATLAB".to_string(), "licensed".to_string())]);
        let rows = gap(&site, &stack, &candidates, &members(), &excluded);
        let by: BTreeMap<_, _> = rows.iter().map(|r| (r.name.as_str(), r)).collect();
        assert_eq!(by["BLAST+"].route, "recipe");
        assert_eq!(
            by["BLAST+"].recipe.as_deref(),
            Some("BLAST+-2.17.0-gompi-2026.1.eb")
        );
        assert_eq!(by["Julia"].recipe.as_deref(), Some("Julia-1.13.2.eb"));
        assert_eq!(by["CP2K"].route, "no-recipe");
        assert_eq!(
            by["CP2K"].newest_elsewhere.as_deref(),
            Some("CP2K-2025.2-foss-2025b.eb")
        );
        assert_eq!(by["LAMMPS"].route, "no-recipe");
        assert_eq!(by["LAMMPS"].newest_elsewhere, None);
        assert_eq!(by["MATLAB"].route, "excluded");
        assert_eq!(by["zlib"].route, "in-stack");
    }

    #[test]
    fn the_plain_build_beats_a_suffixed_one_of_the_same_version() {
        let site = names_from_listing("NCCL\n");
        let candidates = vec![
            cand(
                "NCCL",
                "2.30.4",
                ("GCCcore", "15.2.0"),
                Some("-CUDA-13.3.0"),
            ),
            cand("NCCL", "2.30.4", ("GCCcore", "15.2.0"), None),
        ];
        let rows = gap(
            &site,
            &BTreeSet::new(),
            &candidates,
            &members(),
            &BTreeMap::new(),
        );
        assert_eq!(
            rows[0].recipe.as_deref(),
            Some("NCCL-2.30.4-GCCcore-15.2.0.eb")
        );
    }
}
