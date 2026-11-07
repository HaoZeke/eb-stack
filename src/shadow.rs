//! Report site-layer recipes that shadow a module the EESSI base stack provides.
//!
//! A site layer is the software EESSI does not install. When the site module
//! path is searched first, a recipe whose name is already a base-stack module
//! hides that module. `stack shadow` reads the site recipes and the base
//! inventory from [`crate::gap::inventory_from_module_source`], the loader
//! `stack gap` uses for `--installed`, and reports each shadow.
//!
//! EasyBuild's installed module version is the software version, then
//! [`crate::build_order::MODULE_VERSION_TOOLCHAIN_SEPARATOR`], then the
//! toolchain, then the versionsuffix. See
//! [`crate::build_order::installed_module_version`]. A base version is the
//! same software version when it equals the recipe version (plus its
//! versionsuffix) or continues with that separator. A recipe versionsuffix
//! also has to end the base version, so a CUDA variant does not count as the
//! same version as the plain build. A name the base lists with an empty
//! version set is `unversioned`: the loader records a bare name that way.

use crate::build_order::{installed_module_version, MODULE_VERSION_TOOLCHAIN_SEPARATOR};
use crate::cli_parse::usage;
use crate::domain::Candidate;
use crate::gap::{inventory_from_module_source, ModuleInventory};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The recipe's software version is one the base stack already installs.
pub const SHADOW_SAME: &str = "same";

/// The base stack installs this name at a different software version.
pub const SHADOW_DIFFERENT: &str = "different";

/// The base stack lists this name and records no version.
pub const SHADOW_UNVERSIONED: &str = "unversioned";

/// Report order: an exact version, then another version, then a bare name.
pub const SHADOW_KINDS: &[&str] = &[SHADOW_SAME, SHADOW_DIFFERENT, SHADOW_UNVERSIONED];

/// Marker for a field with nothing to print. `gap::render_tsv` uses it too.
const ABSENT: &str = "-";

/// Arguments for `stack shadow`.
pub struct ShadowRequest {
    /// Site-layer easyconfig trees, or individual `.eb` files. Repeatable.
    pub easyconfigs: Vec<PathBuf>,
    /// EESSI base module trees or listings. Repeatable. Merged the way
    /// `stack gap` merges repeated `--installed` paths.
    pub base: Vec<PathBuf>,
    /// Print one JSON document instead of the text report.
    pub json: bool,
}

/// One site recipe whose name the base stack already provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShadowHit {
    /// [`SHADOW_SAME`], [`SHADOW_DIFFERENT`], or [`SHADOW_UNVERSIONED`].
    pub kind: String,
    /// Easyconfig path the recipe was parsed from.
    pub path: String,
    /// Package name, as the recipe spells it.
    pub name: String,
    /// Software version, without the versionsuffix.
    pub version: String,
    /// Recipe versionsuffix, including its leading separator when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub versionsuffix: Option<String>,
    /// Base module versions that decided the kind. Matching versions for
    /// [`SHADOW_SAME`]; every recorded version for [`SHADOW_DIFFERENT`];
    /// empty for [`SHADOW_UNVERSIONED`].
    pub base: Vec<String>,
}

/// What `stack shadow` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShadowReport {
    /// Shadows, in [`SHADOW_KINDS`] order, then by name and path.
    pub hits: Vec<ShadowHit>,
    /// Site recipes that parsed.
    pub recipes: usize,
    /// Distinct base-stack names, matched without regard to case.
    pub base_names: usize,
    /// Text for stdout. JSON when the request asked for it.
    #[serde(skip)]
    pub stdout: String,
}

impl ShadowReport {
    /// How many hits have this kind.
    pub fn count(&self, kind: &str) -> usize {
        self.hits.iter().filter(|hit| hit.kind == kind).count()
    }

    /// The stderr line when a recipe shadows the base, or `None` when none do.
    pub fn failure(&self) -> Option<String> {
        let n = self.hits.len();
        if n == 0 {
            return None;
        }
        Some(format!(
            "{n} site-layer {} {} a module the EESSI base stack provides",
            plural(n, "recipe", "recipes"),
            plural(n, "shadows", "shadow"),
        ))
    }
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

fn kind_rank(kind: &str) -> usize {
    SHADOW_KINDS
        .iter()
        .position(|name| *name == kind)
        .unwrap_or(SHADOW_KINDS.len())
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Whether `module_version` is the recipe's software version.
///
/// The base string comes from [`inventory_from_module_source`], which keeps
/// the Lmod spelling, toolchain included. Equality covers a listing that
/// stored only the software version (and versionsuffix). A longer module
/// version matches when it continues with
/// [`MODULE_VERSION_TOOLCHAIN_SEPARATOR`], which is how
/// [`installed_module_version`] attaches the toolchain. A recipe
/// versionsuffix has to end that module version: the suffix is part of the
/// version token, as `Candidate` stores it (for example `-CUDA-12.6.0`).
pub fn same_software_version(
    version: &str,
    versionsuffix: Option<&str>,
    module_version: &str,
) -> bool {
    let suffix = versionsuffix.unwrap_or("");
    let token = format!("{version}{suffix}");
    if module_version == token {
        return true;
    }
    let prefixed = format!("{version}{MODULE_VERSION_TOOLCHAIN_SEPARATOR}");
    module_version.starts_with(&prefixed) && (suffix.is_empty() || module_version.ends_with(suffix))
}

fn versions_for(inventory: &ModuleInventory, name: &str) -> Option<BTreeSet<String>> {
    let mut found = false;
    let mut versions = BTreeSet::new();
    for (key, set) in inventory {
        if key.eq_ignore_ascii_case(name) {
            found = true;
            versions.extend(set.iter().cloned());
        }
    }
    found.then_some(versions)
}

fn merge_inventory(into: &mut ModuleInventory, from: ModuleInventory) {
    for (name, versions) in from {
        let key = into
            .keys()
            .find(|key| key.eq_ignore_ascii_case(&name))
            .cloned();
        match key {
            Some(key) => {
                into.get_mut(&key)
                    .expect("key was just found in the inventory")
                    .extend(versions);
            }
            None => {
                into.insert(name, versions);
            }
        }
    }
}

/// Load every `--base` path and merge them into one inventory.
///
/// Each path goes through [`inventory_from_module_source`]. Names that
/// differ only by case share one key, the spelling of the first path that
/// named it.
pub fn load_base(paths: &[PathBuf]) -> Result<ModuleInventory> {
    if paths.is_empty() {
        return Err(usage("pass at least one --base module tree"));
    }
    let mut inventory = ModuleInventory::new();
    for path in paths {
        let loaded = inventory_from_module_source(path)
            .with_context(|| format!("--base {}", path.display()))?;
        merge_inventory(&mut inventory, loaded);
    }
    Ok(inventory)
}

fn load_recipes(paths: &[PathBuf]) -> Result<Vec<Candidate>> {
    if paths.is_empty() {
        return Err(usage("pass at least one --easyconfigs tree"));
    }
    let mut directories: Vec<&Path> = Vec::new();
    let mut files = Vec::new();
    for path in paths {
        if path.is_file() {
            files.push(path);
        } else {
            directories.push(path.as_path());
        }
    }
    let mut candidates = Vec::new();
    if !directories.is_empty() {
        let parsed = crate::parse_easyconfig_trees(&directories).map_err(|error| {
            anyhow::Error::msg(error).context(format!(
                "--easyconfigs {}",
                directories
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            ))
        })?;
        if !parsed.skipped.is_empty() {
            let n = parsed.skipped.len();
            let lines = parsed
                .skipped
                .iter()
                .map(|skipped| format!("{}: {}", skipped.path, skipped.error))
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "{n} site-layer {} did not parse:\n{lines}",
                plural(n, "recipe", "recipes")
            );
        }
        candidates.extend(parsed.candidates);
    }
    for path in files {
        let candidate = crate::parse_easyconfig_file(path).map_err(|error| {
            anyhow::Error::msg(error).context(format!("--easyconfigs {}", path.display()))
        })?;
        candidates.push(candidate);
    }
    Ok(candidates)
}

/// Classify site recipes against a base inventory. Names match without
/// regard to case. A name the inventory does not have is the site layer
/// and is left out.
pub fn classify(recipes: &[Candidate], base: &ModuleInventory) -> Vec<ShadowHit> {
    let mut hits = Vec::new();
    for recipe in recipes {
        let Some(recorded) = versions_for(base, &recipe.name) else {
            continue;
        };
        let installed = installed_module_version(recipe);
        let matched: Vec<String> = recorded
            .iter()
            .filter(|module_version| {
                *module_version == &installed
                    || same_software_version(
                        &recipe.version,
                        recipe.versionsuffix.as_deref(),
                        module_version,
                    )
            })
            .cloned()
            .collect();
        let (kind, base_versions) = if recorded.is_empty() {
            (SHADOW_UNVERSIONED, Vec::new())
        } else if !matched.is_empty() {
            (SHADOW_SAME, matched)
        } else {
            (SHADOW_DIFFERENT, recorded.into_iter().collect())
        };
        hits.push(ShadowHit {
            kind: kind.to_string(),
            path: recipe.easyconfig_path.clone(),
            name: recipe.name.clone(),
            version: recipe.version.clone(),
            versionsuffix: recipe.versionsuffix.clone(),
            base: base_versions,
        });
    }
    hits.sort_by(|left, right| {
        kind_rank(&left.kind)
            .cmp(&kind_rank(&right.kind))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.version.cmp(&right.version))
            .then_with(|| left.path.cmp(&right.path))
    });
    hits
}

fn version_token(hit: &ShadowHit) -> String {
    format!(
        "{}{}",
        hit.version,
        hit.versionsuffix.as_deref().unwrap_or("")
    )
}

fn render_text(report: &ShadowReport) -> String {
    let mut out = String::new();
    for hit in &report.hits {
        let base = if hit.base.is_empty() {
            ABSENT.to_string()
        } else {
            hit.base.join(",")
        };
        out.push_str(&format!(
            "{} {} name={} version={} base={base}\n",
            hit.kind,
            basename(&hit.path),
            hit.name,
            version_token(hit),
        ));
    }
    out.push_str(&format!(
        "same={} different={} unversioned={} recipes={} base={}\n",
        report.count(SHADOW_SAME),
        report.count(SHADOW_DIFFERENT),
        report.count(SHADOW_UNVERSIONED),
        report.recipes,
        report.base_names,
    ));
    out
}

fn render_json(report: &ShadowReport) -> String {
    let mut value = serde_json::to_value(report).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.insert("same".into(), serde_json::json!(report.count(SHADOW_SAME)));
        object.insert(
            "different".into(),
            serde_json::json!(report.count(SHADOW_DIFFERENT)),
        );
        object.insert(
            "unversioned".into(),
            serde_json::json!(report.count(SHADOW_UNVERSIONED)),
        );
    }
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

/// Read the site recipes and the base inventory, and report every shadow.
pub fn run(request: &ShadowRequest) -> Result<ShadowReport> {
    let base = load_base(&request.base)?;
    let recipes = load_recipes(&request.easyconfigs)?;
    let hits = classify(&recipes, &base);
    let mut report = ShadowReport {
        hits,
        recipes: recipes.len(),
        base_names: base.len(),
        stdout: String::new(),
    };
    report.stdout = if request.json {
        render_json(&report)
    } else {
        render_text(&report)
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Candidate, Toolchain};
    use std::collections::BTreeSet;

    fn recipe(
        name: &str,
        version: &str,
        toolchain: (&str, &str),
        versionsuffix: Option<&str>,
        path: &str,
    ) -> Candidate {
        Candidate {
            easyblock: None,
            name: name.into(),
            version: version.into(),
            toolchain: Toolchain {
                name: toolchain.0.into(),
                version: toolchain.1.into(),
            },
            versionsuffix: versionsuffix.map(str::to_string),
            versionprefix: None,
            easyconfig_path: path.into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
            moduleclass: None,
        }
    }

    fn inventory(rows: &[(&str, &[&str])]) -> ModuleInventory {
        let mut inventory = ModuleInventory::new();
        for (name, versions) in rows {
            inventory.insert(
                (*name).to_string(),
                versions
                    .iter()
                    .map(|version| (*version).to_string())
                    .collect(),
            );
        }
        inventory
    }

    #[test]
    fn the_installed_module_version_is_the_same_software_version() {
        let zlib = recipe(
            "zlib",
            "1.3.1",
            ("GCCcore", "15.2.0"),
            None,
            "zlib-1.3.1-GCCcore-15.2.0.eb",
        );
        let full = installed_module_version(&zlib);
        assert_eq!(full, "1.3.1-GCCcore-15.2.0");
        assert!(same_software_version("1.3.1", None, &full));
        let hits = classify(&[zlib], &inventory(&[("zlib", &[&full])]));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, SHADOW_SAME);
        assert_eq!(hits[0].base, vec![full]);
    }

    #[test]
    fn a_longer_version_does_not_match_a_shorter_one() {
        assert!(!same_software_version("1.3", None, "1.3.1-GCCcore-15.2.0"));
        assert!(same_software_version("1.3.1", None, "1.3.1"));
        assert!(same_software_version("1.3.1", None, "1.3.1-GCCcore-15.2.0"));
    }

    #[test]
    fn a_versionsuffix_has_to_end_the_base_module_version() {
        let suffix = "-CUDA-12.6.0";
        assert!(same_software_version(
            "2025.1",
            Some(suffix),
            "2025.1-foss-2026.1-CUDA-12.6.0"
        ));
        assert!(!same_software_version(
            "2025.1",
            Some(suffix),
            "2025.1-foss-2026.1"
        ));
        let lammps = recipe(
            "LAMMPS",
            "2025.1",
            ("foss", "2026.1"),
            Some(suffix),
            "LAMMPS-2025.1-foss-2026.1-CUDA-12.6.0.eb",
        );
        assert_eq!(
            installed_module_version(&lammps),
            "2025.1-foss-2026.1-CUDA-12.6.0"
        );
        let hits = classify(
            &[lammps],
            &inventory(&[("LAMMPS", &["2025.1-foss-2026.1"])]),
        );
        assert_eq!(hits[0].kind, SHADOW_DIFFERENT);
    }

    #[test]
    fn classify_reports_same_different_and_unversioned_and_skips_site_only() {
        let recipes = vec![
            recipe(
                "VASP",
                "6.5.1",
                ("foss", "2026.1"),
                None,
                "VASP-6.5.1-foss-2026.1.eb",
            ),
            recipe(
                "FFTW",
                "3.3.11",
                ("gompi", "2023a"),
                None,
                "FFTW-3.3.11-gompi-2023a.eb",
            ),
            recipe(
                "zlib",
                "1.3.1",
                ("GCCcore", "15.2.0"),
                None,
                "zlib-1.3.1-GCCcore-15.2.0.eb",
            ),
            recipe(
                "OnlyName",
                "1.2",
                ("GCCcore", "15.2.0"),
                None,
                "OnlyName-1.2-GCCcore-15.2.0.eb",
            ),
        ];
        let base = inventory(&[
            ("Zlib", &["1.3.1-GCCcore-13.2.0"]),
            ("FFTW", &["3.3.10-gompi-2023a"]),
            ("OnlyName", &[]),
        ]);
        let hits = classify(&recipes, &base);
        assert_eq!(
            hits.iter().map(|hit| hit.kind.as_str()).collect::<Vec<_>>(),
            vec![SHADOW_SAME, SHADOW_DIFFERENT, SHADOW_UNVERSIONED]
        );
        assert_eq!(hits[0].name, "zlib");
        assert_eq!(hits[1].name, "FFTW");
        assert_eq!(hits[1].base, vec!["3.3.10-gompi-2023a".to_string()]);
        assert!(hits[2].base.is_empty());
    }

    #[test]
    fn merged_base_paths_fold_case_and_versions() {
        let mut into = inventory(&[("zlib", &["1.2.13-GCCcore-12.3.0"])]);
        merge_inventory(&mut into, inventory(&[("Zlib", &["1.3.1-GCCcore-15.2.0"])]));
        assert_eq!(into.len(), 1);
        let versions = into.get("zlib").expect("first spelling");
        assert_eq!(
            versions,
            &BTreeSet::from([
                "1.2.13-GCCcore-12.3.0".to_string(),
                "1.3.1-GCCcore-15.2.0".to_string(),
            ])
        );
    }

    #[test]
    fn the_text_report_names_each_shadow_and_the_failure() {
        let recipes = vec![recipe(
            "zlib",
            "1.3.1",
            ("GCCcore", "15.2.0"),
            None,
            "easyconfigs/z/zlib/zlib-1.3.1-GCCcore-15.2.0.eb",
        )];
        let base = inventory(&[("zlib", &["1.3.1-GCCcore-15.2.0"])]);
        let report = ShadowReport {
            hits: classify(&recipes, &base),
            recipes: recipes.len(),
            base_names: base.len(),
            stdout: String::new(),
        };
        let text = render_text(&report);
        assert_eq!(
            text,
            "\
same zlib-1.3.1-GCCcore-15.2.0.eb name=zlib version=1.3.1 base=1.3.1-GCCcore-15.2.0
same=1 different=0 unversioned=0 recipes=1 base=1
"
        );
        assert_eq!(
            report.failure().as_deref(),
            Some("1 site-layer recipe shadows a module the EESSI base stack provides")
        );
    }

    #[test]
    fn an_empty_site_layer_has_no_failure() {
        let report = ShadowReport {
            hits: Vec::new(),
            recipes: 1,
            base_names: 2,
            stdout: String::new(),
        };
        assert!(report.failure().is_none());
        assert!(
            render_text(&report).ends_with("same=0 different=0 unversioned=0 recipes=1 base=2\n")
        );
    }
}
