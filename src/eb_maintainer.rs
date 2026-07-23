//! Maintainer-acceptability checks distilled from real upstream reviews.
//!
//! Primary sources:
//! - easybuild-easyconfigs PR #26435 (CHANGES_REQUESTED): cross-generation
//!   dependency pins ("mixing two different toolchain generations") and
//!   staged/incomprehensible shell in preconfigopts/postinstallcmds.
//! - easybuild-easyconfigs PR #26480 review: hard-coded dependency toolchain
//!   tuples the robot would resolve itself, test suites that exist but are
//!   disabled or never run, and thin builds where the tree convention is to
//!   install packages as fat as possible.
//!
//! These are mechanical gates for `recipe check` / `recipe lint`. They do **not**
//! replace `eb --check-contrib` or a SUCCESS test report.

use crate::domain::{Candidate, Toolchain};
use crate::eb_parse::ResolvedEasyconfig;
use crate::hierarchy::{is_system_toolchain, known_hierarchy};
use serde::{Deserialize, Serialize};

/// Severity of a maintainer-acceptability finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaintainerSeverity {
    /// Hard reject: same class as the #26435 cross-generation pin.
    Error,
    /// Strong reject: same class as the #26435 "incomprehensible" shell pipeline.
    Warning,
}

/// One maintainer-acceptability finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintainerFinding {
    pub code: String,
    pub severity: MaintainerSeverity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

impl MaintainerFinding {
    fn error(code: &str, message: impl Into<String>, evidence: Option<String>) -> Self {
        Self {
            code: code.into(),
            severity: MaintainerSeverity::Error,
            message: message.into(),
            evidence,
        }
    }

    fn warning(code: &str, message: impl Into<String>, evidence: Option<String>) -> Self {
        Self {
            code: code.into(),
            severity: MaintainerSeverity::Warning,
            message: message.into(),
            evidence,
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == MaintainerSeverity::Error
    }
}

/// Composite result for CLI/MCP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintainerReport {
    pub findings: Vec<MaintainerFinding>,
}

impl MaintainerReport {
    pub fn ok_for_upstream(&self) -> bool {
        self.findings.iter().all(|f| !f.is_error())
    }

    pub fn has_warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == MaintainerSeverity::Warning)
    }
}

/// High-level toolchains where a version pin is a generation pin.
fn is_high_level_toolchain(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "foss"
            | "fosscuda"
            | "gfbf"
            | "gompi"
            | "gompic"
            | "golfc"
            | "fossxl"
            | "intel"
            | "intel-compilers"
            | "iimpi"
            | "iimkl"
            | "iomkl"
            | "nvompi"
            | "nvhpc"
    )
}

/// True when dep toolchain is in the same generation hierarchy as the recipe.
fn dep_in_recipe_hierarchy(recipe_tc: &Toolchain, dep_tc: &Toolchain) -> bool {
    if is_system_toolchain(dep_tc) {
        return true;
    }
    if recipe_tc.name.eq_ignore_ascii_case(&dep_tc.name) && recipe_tc.version == dep_tc.version {
        return true;
    }
    // Subtoolchain of the same generation (e.g. GCCcore-15.2.0 under foss-2026.1).
    if let Some(h) = known_hierarchy(recipe_tc) {
        return h
            .members
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case(&dep_tc.name) && m.version == dep_tc.version);
    }
    // Without a hierarchy fixture: same high-level version string only.
    if is_high_level_toolchain(&recipe_tc.name) && is_high_level_toolchain(&dep_tc.name) {
        return recipe_tc.version == dep_tc.version;
    }
    // GCCcore pin without hierarchy: cannot prove same generation → treat as
    // unknown (no hard error). High-level mismatch still fails below.
    true
}

/// Cross-generation dependency pins: hard error (#26435).
pub fn check_cross_generation_pins(recipe: &ResolvedEasyconfig) -> Vec<MaintainerFinding> {
    let mut out = Vec::new();
    let parent = &recipe.toolchain;
    for (role, deps) in [
        ("dependencies", &recipe.dependencies),
        ("builddependencies", &recipe.builddependencies),
    ] {
        for dep in deps.iter() {
            let Some(dep_tc) = dep.toolchain.as_ref() else {
                continue;
            };
            if is_system_toolchain(dep_tc) {
                continue;
            }
            // Plain GCCcore under foss is fine when hierarchy admits it.
            if dep_in_recipe_hierarchy(parent, dep_tc) {
                continue;
            }
            // High-level toolchain with a different generation is always wrong.
            if is_high_level_toolchain(&dep_tc.name)
                && (is_high_level_toolchain(&parent.name) && parent.version != dep_tc.version
                    || !dep_in_recipe_hierarchy(parent, dep_tc))
            {
                out.push(MaintainerFinding::error(
                    "EB_MAINT_CROSS_GEN",
                    format!(
                        "{role} pin ({}, {}, {}, ({}-{})) mixes toolchain generations with recipe {}-{} (easybuild-easyconfigs #26435)",
                        dep.name,
                        dep.version,
                        dep.versionsuffix.as_deref().unwrap_or(""),
                        dep_tc.name,
                        dep_tc.version,
                        parent.name,
                        parent.version
                    ),
                    Some(
                        "This is mixing two different toolchain generations, it shouldn't be done"
                            .to_string(),
                    ),
                ));
                continue;
            }
            // Explicit toolchain outside hierarchy (e.g. wrong GCCcore for generation).
            if !dep_in_recipe_hierarchy(parent, dep_tc) {
                out.push(MaintainerFinding::error(
                    "EB_MAINT_CROSS_GEN",
                    format!(
                        "{role} pin {} @ {}-{} is outside the hierarchy of recipe {}-{}",
                        dep.name, dep_tc.name, dep_tc.version, parent.name, parent.version
                    ),
                    Some(
                        "dependency toolchain must be the recipe generation or a subtoolchain of it"
                            .into(),
                    ),
                ));
            }
        }
    }
    out
}

/// Thresholds for "incomprehensible" shell staging in easyconfig parameters.
const PRECONFIG_PLUS_EQ_HARD: usize = 4;
const PRECONFIG_CHARS_HARD: usize = 400;
const POSTINSTALL_PATCHELF_FORCE: &str = "patchelf --force-rpath";

/// Shell-monster / staged-build patterns: warning by default, escalated to error
/// when the PR shape matches #26435 (many `preconfigopts +=` or cargo cinstall stage).
pub fn check_shell_monsters(text: &str) -> Vec<MaintainerFinding> {
    let mut out = Vec::new();
    let plus_eq = text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("preconfigopts +=")
                || t.starts_with("preconfigopts+=")
                || t.starts_with("preinstallopts +=")
                || t.starts_with("postinstallcmds +=")
        })
        .count();

    let preconfig_blob: String = text
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("preconfigopts")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let has_cargo_stage = preconfig_blob.contains("cargo cinstall")
        || preconfig_blob.contains("cargo install")
            && preconfig_blob.contains("builddir")
            && (preconfig_blob.contains("stage") || preconfig_blob.contains("prefix"));

    let has_subshell_cd = preconfig_blob.contains("(cd ") || preconfig_blob.contains("( cd ");

    if plus_eq >= PRECONFIG_PLUS_EQ_HARD || preconfig_blob.len() >= PRECONFIG_CHARS_HARD {
        let severity_error = plus_eq >= PRECONFIG_PLUS_EQ_HARD || has_cargo_stage;
        let msg = format!(
            "preconfigopts/preinstallopts shell pipeline looks staged/incomprehensible ({} `+=` lines, {} chars); move staged builds to companion easyconfigs or a patch/easyblock (easybuild-easyconfigs #26435)",
            plus_eq,
            preconfig_blob.len()
        );
        let evidence = Some(
            "Sorry, but we can never accept this. It's incomprehensible and uncommented (so we don't even know _why_ you are trying to do this)."
                .into(),
        );
        out.push(if severity_error {
            MaintainerFinding::error("EB_MAINT_SHELL_MONSTER", msg, evidence)
        } else {
            MaintainerFinding::warning("EB_MAINT_SHELL_MONSTER", msg, evidence)
        });
    } else if has_cargo_stage || has_subshell_cd {
        out.push(MaintainerFinding::warning(
            "EB_MAINT_SHELL_STAGE",
            "preconfigopts stages another package build (cargo/subshell); prefer a standalone easyconfig companion",
            Some(
                "readcon-core became its own GCCcore recipe instead of inline cargo cinstall"
                    .into(),
            ),
        ));
    }

    if text.contains(POSTINSTALL_PATCHELF_FORCE) {
        out.push(MaintainerFinding::error(
            "EB_MAINT_PATCHELF_RPATH",
            "postinstallcmds uses `patchelf --force-rpath`; that overrides EasyBuild RPATH policy (see #26480 / upstream-pr skill)",
            Some("use check_readelf_rpath = False when cargo-c installs lack RPATH, do not invent $ORIGIN".into()),
        ));
    }

    // Uncommented multi-line += without a nearby comment is part of #26435.
    if plus_eq >= 2 {
        let mut consecutive = 0usize;
        let mut max_uncommented = 0usize;
        for line in text.lines() {
            let t = line.trim_start();
            if t.starts_with("preconfigopts +=") || t.starts_with("preconfigopts+=") {
                consecutive += 1;
                max_uncommented = max_uncommented.max(consecutive);
            } else if !t.is_empty() {
                // Comments and any other content both break the uncommented run.
                consecutive = 0;
            }
        }
        if max_uncommented >= PRECONFIG_PLUS_EQ_HARD {
            // Already reported as SHELL_MONSTER error; skip duplicate.
        }
    }

    out
}

/// Dependency toolchain tuples the robot would resolve itself: warning (#26480).
///
/// Cross-generation pins are the hard error above; this catches the softer
/// review class where the pin is *in* the recipe hierarchy but still
/// hard-coded. EasyBuild only hard-codes dependency toolchains in very
/// exceptional cases (defining a higher-level toolchain); everywhere else the
/// robot walks the subtoolchains of the recipe generation.
pub fn check_dep_toolchain_pins(recipe: &ResolvedEasyconfig) -> Vec<MaintainerFinding> {
    let parent = &recipe.toolchain;
    if !is_high_level_toolchain(&parent.name) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (role, deps) in [
        ("dependencies", &recipe.dependencies),
        ("builddependencies", &recipe.builddependencies),
    ] {
        for dep in deps.iter() {
            let Some(dep_tc) = dep.toolchain.as_ref() else {
                continue;
            };
            if is_system_toolchain(dep_tc) {
                continue;
            }
            if !dep_in_recipe_hierarchy(parent, dep_tc) {
                // Cross-generation: already a hard error elsewhere.
                continue;
            }
            out.push(MaintainerFinding::warning(
                "EB_MAINT_DEP_TOOLCHAIN_PIN",
                format!(
                    "{role} entry {} hard-codes toolchain {}-{}; the robot resolves {}-{} subtoolchains itself, so use a plain (name, version) tuple (easybuild-easyconfigs #26480 review)",
                    dep.name, dep_tc.name, dep_tc.version, parent.name, parent.version
                ),
                Some(
                    "No need to specify the toolchain here - in fact we only hard-code the toolchain for the dependency in very exceptional cases".into(),
                ),
            ));
        }
    }
    out
}

/// Thin-build flags that keep optional features off without a versionsuffix
/// variant. Tree convention is to install as fat as possible (#26480 review).
const THIN_FLAGS: &[&str] = &[
    "pure_lib=true",
    "client_only=true",
    "minimal=true",
    "headers_only=true",
];

/// Config flags that switch an existing test suite off.
const TESTS_OFF_FLAGS: &[&str] = &[
    "with_tests=false",
    "with_tests=off",
    "build_tests=off",
    "build_testing=off",
    "enable_tests=off",
    "enable_testing=off",
];

/// Config flags that build a test suite (pair them with `runtest`).
const TESTS_ON_FLAGS: &[&str] = &[
    "with_tests=true",
    "build_tests=on",
    "build_testing=on",
    "enable_tests=on",
    "enable_testing=on",
];

/// Fat-build and run-the-tests review classes from #26480: warnings.
pub fn check_fat_build(text: &str) -> Vec<MaintainerFinding> {
    let mut out = Vec::new();
    let configopts_blob: String = text
        .lines()
        .filter(|l| l.trim_start().starts_with("configopts"))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let has_versionsuffix = text
        .lines()
        .any(|l| l.trim_start().starts_with("versionsuffix"));

    if !has_versionsuffix {
        if let Some(flag) = THIN_FLAGS.iter().find(|f| configopts_blob.contains(**f)) {
            out.push(MaintainerFinding::warning(
                "EB_MAINT_THIN_BUILD",
                format!(
                    "configopts keeps the build thin ({flag}); EasyBuild installs packages as fat as possible, and mutually exclusive choices get a versionsuffix variant (easybuild-easyconfigs #26480 review)"
                ),
                Some(
                    "we typically install packages as 'fat' as possible, i.e. with as many optional features enabled as we can".into(),
                ),
            ));
        }
    }

    let has_runtest = text.lines().any(|l| {
        let t = l.trim_start();
        (t.starts_with("runtest") && !t.starts_with("runtest = False"))
            || t.starts_with("runtests = True")
    });
    if let Some(flag) = TESTS_OFF_FLAGS
        .iter()
        .find(|f| configopts_blob.contains(**f))
    {
        out.push(MaintainerFinding::warning(
            "EB_MAINT_TESTS_OFF",
            format!(
                "configopts disables the package test suite ({flag}); maintainers prefer compiling and running unit tests to validate the installation (easybuild-easyconfigs #26480 review)"
            ),
            Some(
                "We typically do prefer to run unit tests (if they exist) to validate the sanity of the installation".into(),
            ),
        ));
    } else if TESTS_ON_FLAGS.iter().any(|f| configopts_blob.contains(*f)) && !has_runtest {
        out.push(MaintainerFinding::warning(
            "EB_MAINT_TESTS_OFF",
            "test suite is compiled but never run: pair the tests-on config flag with runtest so the test step executes it".to_string(),
            Some(
                "We typically do prefer to run unit tests (if they exist) to validate the sanity of the installation".into(),
            ),
        ));
    }

    out
}

/// Re-adding an easyconfig the robot tree already ships: hard error (#26480).
///
/// Do/don't rule 8. A PR that rewrites a file `develop` already has at the same
/// name-version-toolchain is pure churn: reviewers see an unexplained diff
/// against a working recipe, and the contributor's own version is usually worse
/// (different source URL, missing dependencies) because it was written blind.
///
/// `candidates` is the robot tree the recipe will be built against. The recipe
/// under review is expected to come from a *draft* tree, so a self-match on the
/// same path is ignored.
pub fn check_duplicate_upstream(
    recipe: &ResolvedEasyconfig,
    candidates: &[Candidate],
) -> Vec<MaintainerFinding> {
    let mut out = Vec::new();
    for candidate in candidates {
        if candidate.easyconfig_path == recipe.easyconfig_path {
            continue;
        }
        if !candidate.name.eq_ignore_ascii_case(&recipe.name)
            || candidate.version != recipe.version
            || !candidate
                .toolchain
                .name
                .eq_ignore_ascii_case(&recipe.toolchain.name)
            || candidate.toolchain.version != recipe.toolchain.version
        {
            continue;
        }
        let lhs = candidate.versionsuffix.as_deref().unwrap_or("");
        let rhs = recipe.versionsuffix.as_deref().unwrap_or("");
        if lhs != rhs {
            continue;
        }
        out.push(MaintainerFinding::error(
            "EB_MAINT_DUPLICATE_UPSTREAM",
            format!(
                "{}-{} on {}-{} already exists in the robot tree at {}; drop it from the PR and depend on the existing recipe (easybuild-easyconfigs do/don't 8)",
                recipe.name,
                recipe.version,
                recipe.toolchain.name,
                recipe.toolchain.version,
                candidate.easyconfig_path
            ),
            Some(
                "Do not re-add packages that develop already has at the target generation".into(),
            ),
        ));
        break;
    }
    out
}

/// Full maintainer-acceptability report from resolved recipe + source text.
pub fn check_maintainer_acceptability(
    recipe: &ResolvedEasyconfig,
    source_text: &str,
) -> MaintainerReport {
    let mut findings = Vec::new();
    findings.extend(check_cross_generation_pins(recipe));
    findings.extend(check_dep_toolchain_pins(recipe));
    findings.extend(check_shell_monsters(source_text));
    findings.extend(check_fat_build(source_text));
    MaintainerReport { findings }
}

/// Text-only path (lint without full resolve): shell monsters + rough cross-gen
/// regex for four-element foss/gfbf pins that disagree with the recipe toolchain line.
pub fn check_maintainer_acceptability_text(source_text: &str) -> MaintainerReport {
    let mut findings = check_shell_monsters(source_text);
    findings.extend(check_fat_build(source_text));
    // Lightweight cross-gen when resolve is unavailable: look for foss/gfbf/gompi
    // version tokens that differ from the recipe toolchain version.
    if let Some(recipe_ver) = recipe_toolchain_version_from_text(source_text) {
        for (pkg, gen) in high_level_dep_pins_from_text(source_text) {
            if gen != recipe_ver {
                findings.push(MaintainerFinding::error(
                    "EB_MAINT_CROSS_GEN",
                    format!(
                        "dependency pin for {pkg} uses high-level generation {gen} while recipe is {recipe_ver}"
                    ),
                    Some(
                        "This is mixing two different toolchain generations, it shouldn't be done"
                            .into(),
                    ),
                ));
            }
        }
    }
    MaintainerReport { findings }
}

fn recipe_toolchain_version_from_text(text: &str) -> Option<String> {
    // toolchain = {'name': 'foss', 'version': '2026.1'}
    for line in text.lines() {
        let t = line.trim();
        if !t.starts_with("toolchain") {
            continue;
        }
        if let Some(idx) = t.find("'version'") {
            let rest = &t[idx..];
            if let Some(start) = rest.find('\'') {
                let after = &rest[start + 1..];
                if let Some(_end) = after.find('\'') {
                    // first quote pair may be 'version'; find value after :
                    if let Some(colon) = rest.find(':') {
                        let vpart = rest[colon + 1..].trim();
                        let vpart = vpart.trim_start_matches(['\'', '"', ' ']);
                        let end = vpart.find(['\'', '"', ',', '}']).unwrap_or(vpart.len());
                        let v = &vpart[..end];
                        if !v.is_empty() && v != "version" {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
        // also version = '2026.1' form inside dict with double quotes
        if let Some(idx) = t.find("\"version\"") {
            let rest = &t[idx..];
            if let Some(colon) = rest.find(':') {
                let vpart = rest[colon + 1..]
                    .trim()
                    .trim_start_matches(['\'', '"', ' ']);
                let end = vpart.find(['\'', '"', ',', '}']).unwrap_or(vpart.len());
                let v = &vpart[..end];
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

fn high_level_dep_pins_from_text(text: &str) -> Vec<(String, String)> {
    // ('PyTorch', '2.9.1', '', ('foss', '2024a')),
    let mut out = Vec::new();
    let re = regex_lite_high_level_pin();
    for cap in re.captures_iter(text) {
        let pkg = cap
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        let gen = cap
            .get(3)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if !pkg.is_empty() && !gen.is_empty() {
            out.push((pkg, gen));
        }
    }
    out
}

fn regex_lite_high_level_pin() -> regex::Regex {
    // Avoid pulling a second regex crate; project already depends on `regex`.
    regex::Regex::new(
        r#"\(\s*'([^']+)'\s*,\s*'[^']*'\s*,\s*'[^']*'\s*,\s*\(\s*'(foss|gfbf|gompi|gompic|intel|iomkl|iimpi)'\s*,\s*'([^']+)'\s*\)"#,
    )
    .expect("high-level pin regex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eb_parse::resolve_easyconfig_str;

    fn load(path: &str) -> (ResolvedEasyconfig, String) {
        let text = std::fs::read_to_string(path).unwrap();
        let recipe = resolve_easyconfig_str(&text).unwrap();
        (recipe, text)
    }

    #[test]
    fn rejects_cross_generation_fixture() {
        let (recipe, text) = load("fixtures/maintainer_reject_26435/bad_cross_gen.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_CROSS_GEN"),
            "{:?}",
            report
        );
        assert!(!report.ok_for_upstream());
    }

    #[test]
    fn rejects_shell_monster_fixture() {
        let (recipe, text) = load("fixtures/maintainer_reject_26435/bad_shell_monster.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_SHELL_MONSTER" || f.code == "EB_MAINT_PATCHELF_RPATH"),
            "{:?}",
            report
        );
    }

    #[test]
    fn accepts_clean_single_generation() {
        let (recipe, text) = load("fixtures/maintainer_reject_26435/good_single_gen.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report.ok_for_upstream(),
            "unexpected findings: {:?}",
            report
        );
    }

    #[test]
    fn thin_pr_head_fires_thin_build_warning() {
        let (recipe, text) = load("fixtures/maintainer_fat_26480/rgpot-2.5.3-thin-pr-head.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_THIN_BUILD"),
            "{report:?}"
        );
        // Warning class: still upstreamable, but flagged for justification.
        assert!(report.ok_for_upstream());
        assert!(report.has_warnings());
    }

    #[test]
    fn dep_toolchain_pin_fires_warning_not_error() {
        let (recipe, text) = load("fixtures/maintainer_fat_26480/bad_dep_pin.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_DEP_TOOLCHAIN_PIN"),
            "{report:?}"
        );
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_CROSS_GEN"),
            "in-hierarchy pin is not the cross-generation error: {report:?}"
        );
        assert!(report.ok_for_upstream());
    }

    #[test]
    fn tests_off_fires_warning() {
        let (recipe, text) = load("fixtures/maintainer_fat_26480/bad_tests_off.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_TESTS_OFF"),
            "{report:?}"
        );
    }

    #[test]
    fn tests_built_but_never_run_fires_warning() {
        let text = "configopts = '-Dwith_tests=true'\nmoduleclass = 'tools'\n";
        let findings = check_fat_build(text);
        assert!(
            findings.iter().any(|f| f.code == "EB_MAINT_TESTS_OFF"),
            "{findings:?}"
        );
    }

    #[test]
    fn versionsuffix_variant_may_stay_thin() {
        let text = "versionsuffix = '-client'\nconfigopts = '-Dwith_rpc_client_only=true'\n";
        let findings = check_fat_build(text);
        assert!(
            !findings.iter().any(|f| f.code == "EB_MAINT_THIN_BUILD"),
            "deliberate versionsuffix variants are the sanctioned thin shape: {findings:?}"
        );
    }

    #[test]
    fn good_fat_control_is_clean() {
        let (recipe, text) = load("fixtures/maintainer_fat_26480/good_fat.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report.findings.is_empty(),
            "fat control must be finding-free: {report:?}"
        );
    }

    #[test]
    fn fat_rgpot_fixture_is_finding_free() {
        let (recipe, text) =
            load("fixtures/eon_core_rgpot/easyconfigs/r/rgpot/rgpot-2.5.3-GCCcore-15.2.0.eb");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report.findings.is_empty(),
            "the shipped fat rgpot recipe must pass every maintainer gate: {report:?}"
        );
    }

    fn candidate(name: &str, version: &str, tc: &str, tc_ver: &str, path: &str) -> Candidate {
        Candidate {
            name: name.into(),
            version: version.into(),
            toolchain: Toolchain {
                name: tc.into(),
                version: tc_ver.into(),
            },
            versionsuffix: None,
            easyconfig_path: path.into(),
            dependencies: Vec::new(),
            builddependencies: Vec::new(),
            exts_list: Vec::new(),
        }
    }

    #[test]
    fn duplicate_upstream_is_an_error() {
        // The real #26480 slip: nanobind-2.13.0-GCCcore-15.2.0 was written from
        // scratch and pushed over the copy develop already shipped.
        let (recipe, _) = load("fixtures/maintainer_fat_26480/good_fat.eb");
        let robot = vec![candidate(
            &recipe.name,
            &recipe.version,
            &recipe.toolchain.name,
            &recipe.toolchain.version,
            "/robot/g/GoodFat/GoodFat-1.0.0-GCCcore-15.2.0.eb",
        )];
        let findings = check_duplicate_upstream(&recipe, &robot);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].code, "EB_MAINT_DUPLICATE_UPSTREAM");
        assert!(findings[0].is_error());
    }

    #[test]
    fn duplicate_upstream_ignores_self_and_other_versions() {
        let (recipe, _) = load("fixtures/maintainer_fat_26480/good_fat.eb");
        // Same file resolved from the draft tree must not flag itself.
        let mut me = candidate(
            &recipe.name,
            &recipe.version,
            &recipe.toolchain.name,
            &recipe.toolchain.version,
            &recipe.easyconfig_path,
        );
        me.easyconfig_path = recipe.easyconfig_path.clone();
        assert!(check_duplicate_upstream(&recipe, &[me]).is_empty());

        // A different version or generation is a legitimate new contribution.
        let others = vec![
            candidate(
                &recipe.name,
                "0.9.0",
                &recipe.toolchain.name,
                &recipe.toolchain.version,
                "/robot/old.eb",
            ),
            candidate(
                &recipe.name,
                &recipe.version,
                "GCCcore",
                "14.3.0",
                "/robot/prev-gen.eb",
            ),
        ];
        assert!(check_duplicate_upstream(&recipe, &others).is_empty());
    }

    #[test]
    fn duplicate_upstream_respects_versionsuffix_variants() {
        let (recipe, _) = load("fixtures/maintainer_fat_26480/good_fat.eb");
        // A -client variant upstream is a different product, not a duplicate.
        let mut variant = candidate(
            &recipe.name,
            &recipe.version,
            &recipe.toolchain.name,
            &recipe.toolchain.version,
            "/robot/variant.eb",
        );
        variant.versionsuffix = Some("-client".into());
        assert!(check_duplicate_upstream(&recipe, &[variant]).is_empty());
    }

    #[test]
    fn real_26435_eon_triggers_both_classes() {
        let path = "fixtures/maintainer_reject_26435/eOn-2.16.0-foss-2026.1.eb";
        let text = std::fs::read_to_string(path).unwrap();
        let recipe = resolve_easyconfig_str(&text).expect("parse 26435 eOn");
        let report = check_maintainer_acceptability(&recipe, &text);
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_CROSS_GEN"),
            "expected cross-gen on real #26435: {:?}",
            report
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.code == "EB_MAINT_SHELL_MONSTER"),
            "expected shell monster on real #26435: {:?}",
            report
        );
        assert!(!report.ok_for_upstream());
    }
}
