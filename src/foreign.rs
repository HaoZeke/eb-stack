//! Syntax-aware conda-forge and Spack package recipe adapters.
//!
//! # Conda-forge
//!
//! Supports classic `meta.yaml` (conda-build) and v1 `recipe.yaml` (rattler-build):
//! - expands a **restricted** Jinja subset: `{% set x = "..." %}`, `context:` scalars,
//!   `${{ var }}` / `{{ var }}` / `{{ var|lower }}` (no filters beyond lower, no
//!   `compiler()` evaluation — those lines are dropped as build-tool noise);
//! - parses multi-entry `source:` lists and single mapping sources;
//! - walks `requirements.{build,host,run}` list items, including selector-wrapped
//!   maps (`- if: ... then: ...`) by taking the first string leaf.
//!
//! # Spack
//!
//! Restricted static parse of `package.py` (no Python exec), following Spack's
//! package DSL as written in real packages:
//! - `class Name(Base)` and multi-base `class Name(Base1, Base2)`;
//! - `homepage` / `url` / `git` string attributes;
//! - `version("X", sha256=..., tag=..., commit=..., url=...)` kwargs;
//! - preferred version = explicit `preferred=True`, then the first
//!   non-`develop`/`main`/`master`/`head` entry;
//! - `depends_on("spec", type=..., when=...)` including `type=("build", "run")`
//!   tuples and multi-type lists; language virtuals `c`/`cxx`/`fortran` skipped.
//!
//! The adapters preserve source provenance and conditional dependency intent.
//! Canonical package planning, Resolvo selection, and EasyBuild emission live
//! in their dedicated stages.

use crate::package::{ConditionExpr, ConditionPredicate, Confidence, Provenance, SourceSpan};
use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use thiserror::Error;

/// Origin of a foreign recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForeignFormat {
    CondaForge,
    Spack,
}

impl ForeignFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CondaForge => "conda-forge",
            Self::Spack => "spack",
        }
    }
}

/// One dependency name (and optional pin) drawn from a foreign recipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignDep {
    pub name: String,
    /// Pin / constraint as written in the foreign recipe when present.
    pub pin: Option<String>,
    /// Role: `build`, `host`, `run`, or Spack type string when known.
    pub role: String,
    /// Complete upstream dependency spec before EasyBuild name mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_spec: Option<String>,
    /// Selector / `when=` expression represented structurally.
    #[serde(default)]
    pub condition: ConditionExpr,
    #[serde(default)]
    pub provenance: Vec<Provenance>,
}

/// One source artifact (URL/git + optional checksum).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ForeignSource {
    pub url: Option<String>,
    pub filename: Option<String>,
    pub sha256: Option<String>,
    pub git: Option<String>,
    pub tag: Option<String>,
    pub commit: Option<String>,
    /// Conda `target_directory` or Spack resource destination folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_directory: Option<String>,
}

/// Variant / feature flag from Spack `variant()` or residual conda feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ForeignVariant {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub condition: ConditionExpr,
    #[serde(default)]
    pub provenance: Vec<Provenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ForeignRuleKind {
    Conflict,
    Requirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignRule {
    pub kind: ForeignRuleKind,
    pub spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default)]
    pub condition: ConditionExpr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub provenance: Provenance,
}

/// Syntax-normalized fields shared by all foreign recipe formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignRecipe {
    pub format: ForeignFormat,
    pub name: String,
    pub version: String,
    pub homepage: Option<String>,
    /// Preferred / primary source (first expanded URL or preferred Spack version).
    pub source_url: Option<String>,
    pub source_filename: Option<String>,
    pub sha256: Option<String>,
    /// All sources when the foreign recipe lists multiple (conda multi-source, etc.).
    #[serde(default)]
    pub sources: Vec<ForeignSource>,
    pub summary: Option<String>,
    /// Longer description when available (conda about.description, Spack docstring).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub dependencies: Vec<ForeignDep>,
    /// Build-system / base-class hints (e.g. Spack `MesonPackage`, `CMakePackage`).
    #[serde(default)]
    pub build_system_hints: Vec<String>,
    /// Mechanically extracted configure flags (e.g. Spack meson_args / cmake_args literals).
    #[serde(default)]
    pub configopts: Option<String>,
    /// Patch filenames / URLs recorded from foreign recipe (not applied).
    #[serde(default)]
    pub patches: Vec<String>,
    /// Spack variants (and residual conda features when recorded).
    #[serde(default)]
    pub variants: Vec<ForeignVariant>,
    /// Spack `conflicts()` and `requires()` directives.
    #[serde(default)]
    pub rules: Vec<ForeignRule>,
    /// Human notes from the parser.
    pub notes: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ForeignError {
    #[error("foreign recipe parse: {0}")]
    Parse(String),
    #[error("unsupported or undetected foreign recipe format for {0}")]
    Unsupported(String),
    #[error("IO: {0}")]
    Io(String),
}

/// Detect format from path basename / extension.
pub fn detect_foreign_format(path: &Path) -> Option<ForeignFormat> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "meta.yaml" || name == "meta.yml" || name == "recipe.yaml" || name == "recipe.yml" {
        return Some(ForeignFormat::CondaForge);
    }
    if name == "package.py" {
        return Some(ForeignFormat::Spack);
    }
    None
}

/// Parse foreign recipe text for the given format.
pub fn parse_foreign_str(format: ForeignFormat, text: &str) -> Result<ForeignRecipe, ForeignError> {
    match format {
        ForeignFormat::CondaForge => parse_conda_forge(text),
        ForeignFormat::Spack => parse_spack_package(text),
    }
}

/// Parse from path; format auto-detected unless `format` is Some.
pub fn parse_foreign_path(
    path: &Path,
    format: Option<ForeignFormat>,
) -> Result<ForeignRecipe, ForeignError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ForeignError::Io(format!("read {}: {e}", path.display())))?;
    let fmt = format
        .or_else(|| detect_foreign_format(path))
        .ok_or_else(|| ForeignError::Unsupported(path.display().to_string()))?;
    let mut recipe = parse_foreign_str(fmt, &text)?;
    set_recipe_source_path(&mut recipe, &path.display().to_string());
    Ok(recipe)
}

fn set_recipe_source_path(recipe: &mut ForeignRecipe, path: &str) {
    for dependency in &mut recipe.dependencies {
        for provenance in &mut dependency.provenance {
            provenance.span.path = path.to_string();
        }
    }
    for variant in &mut recipe.variants {
        for provenance in &mut variant.provenance {
            provenance.span.path = path.to_string();
        }
    }
    for rule in &mut recipe.rules {
        rule.provenance.span.path = path.to_string();
    }
}

fn source_span(text: &str, start: usize, end: usize) -> SourceSpan {
    fn line_column(text: &str, offset: usize) -> (u32, u32) {
        let prefix = &text[..offset.min(text.len())];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let column = prefix
            .rsplit_once('\n')
            .map(|(_, tail)| tail.chars().count() as u32 + 1)
            .unwrap_or_else(|| prefix.chars().count() as u32 + 1);
        (line, column)
    }

    let (start_line, start_column) = line_column(text, start);
    let (end_line, end_column) = line_column(text, end);
    SourceSpan {
        path: "<memory>".into(),
        start_line,
        start_column,
        end_line,
        end_column,
    }
}

fn provenance_for_range(
    text: &str,
    start: usize,
    end: usize,
    extractor: &str,
    confidence: Confidence,
) -> Provenance {
    Provenance {
        span: source_span(text, start, end),
        extractor: extractor.into(),
        original: text[start..end].trim().to_string(),
        confidence,
    }
}

fn provenance_for_text(text: &str, needle: &str, extractor: &str) -> Provenance {
    let start = text.find(needle).unwrap_or(0);
    let end = start.saturating_add(needle.len()).min(text.len());
    provenance_for_range(text, start, end, extractor, Confidence::Derived)
}

fn extract_spack_config_flags(text: &str) -> Option<String> {
    // Line must start with whitespace then a quote (not f") so f-strings are skipped.
    let lit = Regex::new(r#"(?m)^[ \t]+[\"'](-D[A-Za-z0-9_./+=-]+)[\"']"#).ok()?;
    let mut flags: Vec<String> = lit
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    if flags.is_empty() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    flags.retain(|f| seen.insert(f.clone()));
    Some(flags.join(" "))
}

pub(crate) fn guess_easyblock(recipe: &ForeignRecipe, warnings: &mut Vec<String>) -> String {
    let hint = |needles: &[&str]| {
        recipe.build_system_hints.iter().find(|hint| {
            let lower = hint.to_ascii_lowercase();
            needles.iter().any(|needle| lower.contains(needle))
        })
    };
    if let Some(hint) = hint(&["meson"]) {
        warnings.push(format!("build-system hint {hint} → easyblock MesonNinja"));
        return "MesonNinja".into();
    }
    if let Some(hint) = hint(&["cmake"]) {
        warnings.push(format!("build-system hint {hint} → easyblock CMakeNinja"));
        return "CMakeNinja".into();
    }
    if let Some(hint) = hint(&["python", "pip"]) {
        warnings.push(format!(
            "build-system hint {hint} → easyblock PythonPackage"
        ));
        return "PythonPackage".into();
    }
    if hint(&["autotools", "autoreconf"]).is_some() {
        return "ConfigureMake".into();
    }
    // Dep names as weak signal
    let dep_names: Vec<&str> = recipe
        .dependencies
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    if dep_names
        .iter()
        .any(|n| *n == "meson" || n.ends_with("-meson"))
    {
        warnings.push("meson in foreign deps → easyblock MesonNinja".into());
        return "MesonNinja".into();
    }
    if dep_names.iter().any(|n| *n == "cmake" || *n == "ninja") {
        warnings.push("cmake/ninja in foreign deps → easyblock CMakeNinja".into());
        return "CMakeNinja".into();
    }
    "ConfigureMake".into()
}

// ===========================================================================
// Conda-forge / rattler-build
// ===========================================================================

fn parse_conda_forge(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut notes = Vec::new();
    let (expanded, ctx_notes) = expand_conda_templates(text);
    notes.extend(ctx_notes);
    let expanded = structure_conda_requirement_selectors(&expanded);

    let yaml: YamlValue = serde_yaml::from_str(&expanded).map_err(|e| {
        ForeignError::Parse(format!(
            "conda YAML parse after template expand: {e}; first 200 chars: {:?}",
            expanded.chars().take(200).collect::<String>()
        ))
    })?;

    let map = yaml
        .as_mapping()
        .ok_or_else(|| ForeignError::Parse("conda: top-level must be a mapping".into()))?;

    let package = map
        .get(YamlValue::from("package"))
        .and_then(|v| v.as_mapping());
    let name = package
        .and_then(|p| p.get(YamlValue::from("name")))
        .and_then(yaml_as_string)
        .ok_or_else(|| ForeignError::Parse("conda: missing package.name".into()))?;
    let version = package
        .and_then(|p| p.get(YamlValue::from("version")))
        .and_then(yaml_as_string)
        .ok_or_else(|| ForeignError::Parse("conda: missing package.version".into()))?;

    if version.contains("{{") || version.contains("${{") {
        return Err(ForeignError::Parse(format!(
            "conda: package.version still has unexpanded template: {version}"
        )));
    }

    let sources = parse_conda_sources(map.get(YamlValue::from("source")));
    if sources.is_empty() {
        notes.push("conda: no source entries extracted".into());
    } else if sources.len() > 1 {
        notes.push(format!(
            "conda: {} source entries (multi-source recipe)",
            sources.len()
        ));
    }

    let about = map
        .get(YamlValue::from("about"))
        .and_then(|v| v.as_mapping());
    let homepage = about
        .and_then(|a| {
            a.get(YamlValue::from("homepage"))
                .or_else(|| a.get(YamlValue::from("home")))
        })
        .and_then(yaml_as_string);
    let summary = about
        .and_then(|a| a.get(YamlValue::from("summary")))
        .and_then(yaml_as_string);
    let description = about
        .and_then(|a| a.get(YamlValue::from("description")))
        .and_then(yaml_as_string);
    let license = about
        .and_then(|a| {
            a.get(YamlValue::from("license"))
                .or_else(|| a.get(YamlValue::from("license_file")))
        })
        .and_then(yaml_as_string);

    let mut dependencies = Vec::new();
    if let Some(req) = map
        .get(YamlValue::from("requirements"))
        .and_then(|v| v.as_mapping())
    {
        for (section, role) in [("build", "build"), ("host", "host"), ("run", "run")] {
            if let Some(list) = req
                .get(YamlValue::from(section))
                .and_then(|v| v.as_sequence())
            {
                for item in list {
                    for selected in flatten_conda_req_item(item, &ConditionExpr::Always) {
                        if let Some(dep) = parse_conda_dep_line(
                            &selected.raw,
                            role,
                            selected.condition,
                            provenance_for_text(text, &selected.raw, "conda-selector"),
                        ) {
                            if is_conda_compiler_macro(&dep.name) {
                                notes.push(format!(
                                    "skipped conda compiler/stdlib macro: {}",
                                    dep.name
                                ));
                                continue;
                            }
                            if dep.name.starts_with("if:")
                                || dep.name == "then"
                                || dep.name == "else"
                            {
                                continue;
                            }
                            dependencies.push(dep);
                        }
                    }
                }
            }
        }
    }

    // patches: list of filenames / urls (classic + rattler)
    let mut patches = Vec::new();
    if let Some(p) = map.get(YamlValue::from("patches")) {
        match p {
            YamlValue::Sequence(seq) => {
                for item in seq {
                    if let Some(s) = yaml_as_string(item) {
                        patches.push(s);
                    } else if let Some(m) = item.as_mapping() {
                        if let Some(s) = m
                            .get(YamlValue::from("path"))
                            .or_else(|| m.get(YamlValue::from("file")))
                            .and_then(yaml_as_string)
                        {
                            patches.push(s);
                        }
                    }
                }
            }
            YamlValue::String(s) => patches.push(s.clone()),
            _ => {}
        }
    }
    if let Some(source) = map.get(YamlValue::from("source")) {
        collect_conda_source_patches(source, &mut patches);
    }
    let mut seen_patches = HashSet::new();
    patches.retain(|patch| seen_patches.insert(patch.clone()));
    if !patches.is_empty() {
        notes.push(format!("conda: {} patch path(s) recorded", patches.len()));
    }

    // build.number residual
    if let Some(b) = map
        .get(YamlValue::from("build"))
        .and_then(|v| v.as_mapping())
    {
        if b.get(YamlValue::from("number")).is_some() {
            notes.push("conda: build.number present (not an EB field; residual)".into());
        }
        if b.get(YamlValue::from("script")).is_some() {
            notes.push("conda: build.script present — product build residual".into());
        }
    }
    if map.get(YamlValue::from("test")).is_some() {
        notes.push("conda: test: section present — residual (not mapped to EB sanity)".into());
    }

    // Build system hints from build deps
    let mut build_system_hints = Vec::new();
    for d in &dependencies {
        let n = d.name.to_ascii_lowercase();
        if n == "meson" {
            build_system_hints.push("Meson".into());
        }
        if n == "cmake" {
            build_system_hints.push("CMake".into());
        }
        if n == "ninja" {
            build_system_hints.push("Ninja".into());
        }
    }

    let primary = sources.first().cloned().unwrap_or_default();
    Ok(ForeignRecipe {
        format: ForeignFormat::CondaForge,
        name: sanitize_pkg_name(&name),
        version: version.trim().to_string(),
        homepage,
        source_url: primary.url.clone(),
        source_filename: primary.filename.clone(),
        sha256: primary.sha256.clone(),
        sources,
        summary,
        description,
        license,
        dependencies,
        build_system_hints,
        configopts: None,
        patches,
        variants: Vec::new(),
        rules: Vec::new(),
        notes,
    })
}

fn structure_conda_requirement_selectors(text: &str) -> String {
    let selector = Regex::new(r#"^(\s*)-\s+(.+?)\s+#\s*\[([^]]+)\]\s*$"#)
        .expect("static conda selector regex");
    let mut output = Vec::new();
    let mut requirements_indent = None;

    for line in text.lines() {
        let trimmed = line.trim();
        let indent = line.len().saturating_sub(line.trim_start().len());
        if trimmed == "requirements:" {
            requirements_indent = Some(indent);
            output.push(line.to_string());
            continue;
        }
        if requirements_indent
            .is_some_and(|base| !trimmed.is_empty() && !trimmed.starts_with('#') && indent <= base)
        {
            requirements_indent = None;
        }
        if requirements_indent.is_some() {
            if let Some(captures) = selector.captures(line) {
                let indentation = captures.get(1).map_or("", |value| value.as_str());
                let value = captures.get(2).map_or("", |value| value.as_str());
                let condition = captures.get(3).map_or("", |value| value.as_str());
                let quoted =
                    serde_json::to_string(condition).unwrap_or_else(|_| format!("\"{condition}\""));
                output.push(format!("{indentation}- if: {quoted}"));
                output.push(format!("{indentation}  then: {value}"));
                continue;
            }
        }
        output.push(line.to_string());
    }

    let mut structured = output.join("\n");
    if text.ends_with('\n') {
        structured.push('\n');
    }
    structured
}

fn collect_conda_source_patches(source: &YamlValue, patches: &mut Vec<String>) {
    match source {
        YamlValue::Sequence(sources) => {
            for source in sources {
                collect_conda_source_patches(source, patches);
            }
        }
        YamlValue::Mapping(source) => {
            let Some(value) = source.get(YamlValue::from("patches")) else {
                return;
            };
            match value {
                YamlValue::Sequence(items) => {
                    for item in items {
                        if let Some(patch) = yaml_as_string(item) {
                            patches.push(patch);
                        } else if let Some(mapping) = item.as_mapping() {
                            if let Some(patch) = mapping
                                .get(YamlValue::from("path"))
                                .or_else(|| mapping.get(YamlValue::from("file")))
                                .and_then(yaml_as_string)
                            {
                                patches.push(patch);
                            }
                        }
                    }
                }
                YamlValue::String(patch) => patches.push(patch.clone()),
                _ => {}
            }
        }
        _ => {}
    }
}

/// Expand deterministic `{% set %}` expressions, `context:` scalars, and
/// simple `${{ x }}` / `{{ x }}` / `|lower` substitutions.
fn expand_conda_templates(text: &str) -> (String, Vec<String>) {
    let mut notes = Vec::new();
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut dates: HashMap<String, NaiveDate> = HashMap::new();

    let set_re = Regex::new(
        r#"(?m)^[ \t]*\{%\s*set\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([^\r\n]*?)\s*%\}[ \t]*(?:\r?\n|$)"#,
    )
    .expect("set re");
    let mut unresolved_sets = Vec::new();
    for c in set_re.captures_iter(text) {
        let name = c[1].to_string();
        let expression = c[2].trim();
        if let Some(value) = eval_conda_set_expression(expression, &vars, &dates) {
            match value {
                CondaTemplateValue::String(value) => {
                    vars.insert(name, value);
                }
                CondaTemplateValue::Date(value) => {
                    dates.insert(name, value);
                }
            }
        } else {
            unresolved_sets.push(name);
        }
    }

    // rattler context: block — simple scalar keys only
    let context_re = Regex::new(r"(?m)^context\s*:\s*$").expect("context re");
    let context_end_re = Regex::new(r"^[A-Za-z_]").expect("context end re");
    let context_scalar_re = Regex::new(
        r#"^[ \t]+([A-Za-z_][A-Za-z0-9_]*)\s*:\s*[\"']?([^\"'#\n]+?)[\"']?\s*(?:#.*)?$"#,
    )
    .expect("context scalar re");
    if let Some(ctx_start) = context_re.find(text) {
        let rest = &text[ctx_start.end()..];
        for line in rest.lines() {
            if context_end_re.is_match(line)
                && line.contains(':')
                && !line.starts_with(' ')
                && !line.starts_with('\t')
            {
                break;
            }
            if let Some(c) = context_scalar_re.captures(line) {
                let k = c[1].to_string();
                let v = c[2].trim().to_string();
                // skip nested structures
                if !v.is_empty() && v != "|" && v != ">" {
                    vars.insert(k, v);
                }
            }
        }
    }

    if !vars.is_empty() {
        notes.push(format!(
            "expanded {} template variable(s): {}",
            vars.len(),
            vars.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if !unresolved_sets.is_empty() {
        notes.push(format!(
            "unevaluated template assignment(s) retained as residuals: {}",
            unresolved_sets.join(", ")
        ));
    }

    let mut out = text.to_string();
    // Assignment and control statements are not YAML. Deterministic values
    // are substituted below; unresolved statements remain represented in notes.
    out = set_re.replace_all(&out, "").to_string();
    let control_re =
        Regex::new(r#"(?m)^[ \t]*\{%\s*(?:if|elif|else|endif)\b[^\r\n]*%\}[ \t]*(?:\r?\n|$)"#)
            .expect("control statement re");
    let control_count = control_re.find_iter(&out).count();
    out = control_re.replace_all(&out, "").to_string();
    if control_count > 0 {
        notes.push(format!(
            "removed {control_count} unevaluated Jinja control statement(s); branch contents preserved"
        ));
    }

    let pure_macro_requirement_re =
        Regex::new(r#"(?m)^[ \t]*-[ \t]*(?:\$\{\{|\{\{)[^\r\n]*\}\}[ \t]*(?:#.*)?(?:\r?\n|$)"#)
            .expect("pure macro requirement re");
    let macro_requirement_count = pure_macro_requirement_re.find_iter(&out).count();
    out = pure_macro_requirement_re.replace_all(&out, "").to_string();
    if macro_requirement_count > 0 {
        notes.push(format!(
            "skipped {macro_requirement_count} pure template requirement(s)"
        ));
    }
    out = remove_duplicate_selector_keys(&out, &mut notes);

    // Replace longer keys first
    let mut keys: Vec<_> = vars.keys().cloned().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in keys {
        let v = vars.get(&k).unwrap();
        let v_lower = v.to_ascii_lowercase();
        // ${{ var }}  ${{ var|lower }}
        for (pat, rep) in [
            (format!("${{{{ {k} }}}}"), v.as_str()),
            (format!("${{{{{k}}}}}"), v.as_str()),
            (format!("${{{{ {k}|lower }}}}"), v_lower.as_str()),
            (format!("${{{{{k}|lower}}}}"), v_lower.as_str()),
            (format!("{{{{ {k} }}}}"), v.as_str()),
            (format!("{{{{{k}}}}}"), v.as_str()),
            (format!("{{{{ {k}|lower }}}}"), v_lower.as_str()),
            (format!("{{{{{k}|lower}}}}"), v_lower.as_str()),
        ] {
            out = out.replace(&pat, rep);
        }
    }

    if out.contains("{{") || out.contains("${{") {
        notes.push(
            "residual Jinja/template constructs remain after expand (compiler macros, selectors)"
                .into(),
        );
    }

    (out, notes)
}

enum CondaTemplateValue {
    String(String),
    Date(NaiveDate),
}

fn eval_conda_set_expression(
    expression: &str,
    vars: &HashMap<String, String>,
    dates: &HashMap<String, NaiveDate>,
) -> Option<CondaTemplateValue> {
    let quoted = Regex::new(r#"^[\"']([^\"']*)[\"']$"#).expect("quoted expression re");
    if let Some(value) = quoted
        .captures(expression)
        .and_then(|capture| capture.get(1))
    {
        return Some(CondaTemplateValue::String(value.as_str().to_string()));
    }

    let scalar = Regex::new(r"^[A-Za-z0-9_.+-]+$").expect("scalar expression re");
    if scalar.is_match(expression) {
        return Some(CondaTemplateValue::String(
            vars.get(expression)
                .cloned()
                .unwrap_or_else(|| expression.to_string()),
        ));
    }

    let strptime = Regex::new(
        r#"^datetime\.datetime\.strptime\(([A-Za-z_][A-Za-z0-9_]*)\.split\([\"']([^\"']*)[\"']\)\[([0-9]+)\],\s*[\"']([^\"']+)[\"']\)$"#,
    )
    .expect("strptime expression re");
    if let Some(capture) = strptime.captures(expression) {
        let value = vars.get(&capture[1])?;
        let index = capture[3].parse::<usize>().ok()?;
        let part = value.split(&capture[2]).nth(index)?;
        let date = NaiveDate::parse_from_str(part, &capture[4]).ok()?;
        return Some(CondaTemplateValue::Date(date));
    }

    let date_format =
        Regex::new(r#"^[\"']\{:(%[^\"']+)\}[\"']\.format\(([A-Za-z_][A-Za-z0-9_]*)\)$"#)
            .expect("date format expression re");
    if let Some(capture) = date_format.captures(expression) {
        let date = dates.get(&capture[2])?;
        return Some(CondaTemplateValue::String(
            date.format(&capture[1]).to_string(),
        ));
    }

    None
}

fn remove_duplicate_selector_keys(text: &str, notes: &mut Vec<String>) -> String {
    let top_level_key = Regex::new(r"^([A-Za-z_][A-Za-z0-9_.-]*):").expect("top-level YAML key re");
    let selector_key = Regex::new(r"^([ \t]+)([A-Za-z_][A-Za-z0-9_.-]*):[^#]*#\s*\[[^]]+\]\s*$")
        .expect("selector-gated YAML key re");
    let mut section = String::new();
    let mut seen = HashSet::new();
    let mut duplicate_count = 0usize;
    let mut out = String::with_capacity(text.len());

    for line in text.lines() {
        if let Some(capture) = top_level_key.captures(line) {
            section = capture[1].to_string();
        }
        let duplicate = selector_key.captures(line).is_some_and(|capture| {
            let identity = (section.clone(), capture[1].len(), capture[2].to_string());
            !seen.insert(identity)
        });
        if duplicate {
            duplicate_count += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    if duplicate_count > 0 {
        notes.push(format!(
            "collapsed {duplicate_count} duplicate selector-gated mapping key(s)"
        ));
    }
    out
}

fn parse_conda_sources(source_val: Option<&YamlValue>) -> Vec<ForeignSource> {
    let Some(v) = source_val else {
        return Vec::new();
    };
    let mut out = Vec::new();
    match v {
        YamlValue::Sequence(seq) => {
            for item in seq {
                if let Some(s) = foreign_source_from_yaml_map(item) {
                    out.push(s);
                }
            }
        }
        YamlValue::Mapping(_) => {
            if let Some(s) = foreign_source_from_yaml_map(v) {
                out.push(s);
            }
        }
        _ => {}
    }
    out
}

fn foreign_source_from_yaml_map(v: &YamlValue) -> Option<ForeignSource> {
    let m = v.as_mapping()?;
    let url = m.get(YamlValue::from("url")).and_then(yaml_as_string);
    let git = m
        .get(YamlValue::from("git_url"))
        .or_else(|| m.get(YamlValue::from("git")))
        .and_then(yaml_as_string);
    let filename = m
        .get(YamlValue::from("fn"))
        .or_else(|| m.get(YamlValue::from("file")))
        .and_then(yaml_as_string);
    let sha256 = m.get(YamlValue::from("sha256")).and_then(yaml_as_string);
    let tag = m.get(YamlValue::from("tag")).and_then(yaml_as_string);
    let target_directory = m
        .get(YamlValue::from("target_directory"))
        .or_else(|| m.get(YamlValue::from("folder")))
        .and_then(yaml_as_string);
    if url.is_none() && git.is_none() && sha256.is_none() {
        return None;
    }
    Some(ForeignSource {
        url,
        filename,
        sha256,
        git,
        tag,
        commit: m
            .get(YamlValue::from("git_rev"))
            .or_else(|| m.get(YamlValue::from("git_commit")))
            .and_then(yaml_as_string),
        target_directory,
    })
}

#[derive(Debug, Clone)]
struct CondaRequirementItem {
    raw: String,
    condition: ConditionExpr,
}

/// Flatten a requirements list item while preserving selector branches.
fn flatten_conda_req_item(
    item: &YamlValue,
    inherited: &ConditionExpr,
) -> Vec<CondaRequirementItem> {
    match item {
        YamlValue::String(s) => vec![CondaRequirementItem {
            raw: s.clone(),
            condition: inherited.clone(),
        }],
        YamlValue::Mapping(m) => {
            // Selector form: { if: ..., then: "pkg" | [..], else: ... }
            if let Some(selector) = m.get(YamlValue::from("if")).and_then(yaml_as_string) {
                let selector_condition = parse_conda_selector(&selector);
                let mut out = Vec::new();
                if let Some(value) = m.get(YamlValue::from("then")) {
                    let condition = condition_all(inherited.clone(), selector_condition.clone());
                    out.extend(flatten_conda_req_item(value, &condition));
                }
                if let Some(value) = m.get(YamlValue::from("else")) {
                    let condition = condition_all(
                        inherited.clone(),
                        ConditionExpr::Not(Box::new(selector_condition)),
                    );
                    out.extend(flatten_conda_req_item(value, &condition));
                }
                return out;
            }

            let mut out = Vec::new();
            for value in m.values() {
                out.extend(flatten_conda_req_item(value, inherited));
            }
            out
        }
        YamlValue::Sequence(seq) => seq
            .iter()
            .flat_map(|value| flatten_conda_req_item(value, inherited))
            .collect(),
        _ => Vec::new(),
    }
}

fn condition_all(left: ConditionExpr, right: ConditionExpr) -> ConditionExpr {
    match (left, right) {
        (ConditionExpr::Never, _) | (_, ConditionExpr::Never) => ConditionExpr::Never,
        (ConditionExpr::Always, expression) | (expression, ConditionExpr::Always) => expression,
        (ConditionExpr::All(mut left), ConditionExpr::All(right)) => {
            left.extend(right);
            ConditionExpr::All(left)
        }
        (ConditionExpr::All(mut expressions), expression) => {
            expressions.push(expression);
            ConditionExpr::All(expressions)
        }
        (expression, ConditionExpr::All(mut expressions)) => {
            expressions.insert(0, expression);
            ConditionExpr::All(expressions)
        }
        (left, right) => ConditionExpr::All(vec![left, right]),
    }
}

fn parse_conda_selector(selector: &str) -> ConditionExpr {
    let selector = strip_selector_outer_parentheses(selector.trim());
    if let Some((left, right)) = split_selector_top_level(selector, " or ") {
        return ConditionExpr::Any(vec![
            parse_conda_selector(left),
            parse_conda_selector(right),
        ]);
    }
    if let Some((left, right)) = split_selector_top_level(selector, " and ") {
        return ConditionExpr::All(vec![
            parse_conda_selector(left),
            parse_conda_selector(right),
        ]);
    }
    if let Some(rest) = selector.strip_prefix("not ") {
        return ConditionExpr::Not(Box::new(parse_conda_selector(rest)));
    }
    if let Some((left, right)) = split_selector_top_level(selector, " != ") {
        return ConditionExpr::Predicate(ConditionPredicate::VariableComparison {
            left: left.trim().into(),
            operator: "!=".into(),
            right: right.trim().into(),
        });
    }
    if let Some((left, right)) = split_selector_top_level(selector, " == ") {
        return ConditionExpr::Predicate(ConditionPredicate::VariableComparison {
            left: left.trim().into(),
            operator: "==".into(),
            right: right.trim().into(),
        });
    }
    ConditionExpr::Predicate(ConditionPredicate::Platform {
        name: selector.into(),
    })
}

fn strip_selector_outer_parentheses(mut selector: &str) -> &str {
    loop {
        let Some(inner) = selector.strip_prefix('(') else {
            return selector.trim();
        };
        let mut depth = 1usize;
        let mut closing = None;
        let mut quote = None;
        for (index, character) in inner.char_indices() {
            if let Some(active) = quote {
                if character == active {
                    quote = None;
                }
                continue;
            }
            match character {
                '\'' | '"' => quote = Some(character),
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        closing = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        if closing == Some(inner.len().saturating_sub(1)) {
            selector = inner[..inner.len().saturating_sub(1)].trim();
        } else {
            return selector.trim();
        }
    }
}

fn split_selector_top_level<'a>(selector: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0usize;
    let mut quote = None;
    for (index, character) in selector.char_indices() {
        if let Some(active) = quote {
            if character == active {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 && selector[index..].starts_with(separator) => {
                return Some((&selector[..index], &selector[index + separator.len()..]));
            }
            _ => {}
        }
    }
    None
}

fn yaml_as_string(v: &YamlValue) -> Option<String> {
    match v {
        YamlValue::String(s) => Some(s.clone()),
        YamlValue::Number(n) => Some(n.to_string()),
        YamlValue::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn parse_conda_dep_line(
    raw: &str,
    role: &str,
    condition: ConditionExpr,
    provenance: Provenance,
) -> Option<ForeignDep> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // "name", "name version", "name >=1.2", "libtorch *cpu*"
    let mut parts = line.split_whitespace();
    let name = parts.next()?.to_string();
    if name.contains("{{") || name.contains("${{") {
        return None;
    }
    let pin = {
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty()
            || rest
                .iter()
                .any(|fragment| fragment.contains("{{") || fragment.contains("${{"))
        {
            None
        } else {
            Some(rest.join(" "))
        }
    };
    Some(ForeignDep {
        name: sanitize_pkg_name(&name),
        pin,
        role: role.into(),
        original_spec: Some(line.into()),
        condition,
        provenance: vec![provenance],
    })
}

fn is_conda_compiler_macro(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("compiler(")
        || n.contains("stdlib(")
        || n.starts_with("cross-python")
        || n == "sccache"
}

fn sanitize_pkg_name(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        return "unknown".into();
    }
    n.to_string()
}

// ===========================================================================
// Spack package.py (restricted static parse)
// ===========================================================================

fn parse_spack_package(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut notes = Vec::new();
    notes.push("Spack package.py: restricted static parse (no Python execution)".into());

    // class Name(Base) or class Name(Base1, Base2, ...)
    let class_re = Regex::new(r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)\s*:")
        .map_err(|e| ForeignError::Parse(e.to_string()))?;
    let class_cap = class_re
        .captures(text)
        .ok_or_else(|| ForeignError::Parse("spack: no class Name(...): found".into()))?;
    let class_name = class_cap.get(1).unwrap().as_str();
    let bases_raw = class_cap.get(2).unwrap().as_str();
    let bases: Vec<String> = bases_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let name = spack_class_to_pkg_name(class_name);
    let mut build_system_hints = bases.clone();
    notes.push(format!("class {class_name} bases: {}", bases.join(", ")));

    let homepage = spack_string_attr(text, "homepage");
    let url = spack_string_attr(text, "url");
    let git = spack_string_attr(text, "git");

    // version("X", sha256="...", tag="...", commit="...", url="...")
    let versions = parse_spack_versions(text)?;
    if versions.is_empty() {
        return Err(ForeignError::Parse(
            "spack: no version(\"...\") directives found".into(),
        ));
    }

    let preferred = pick_preferred_spack_version(&versions);
    notes.push(format!(
        "preferred version {} (from {} version() directives)",
        preferred.version,
        versions.len()
    ));
    if versions.len() > 1 {
        notes.push(format!(
            "additional versions not emitted: {}",
            versions
                .iter()
                .filter(|v| v.version != preferred.version)
                .take(8)
                .map(|v| v.version.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let dependencies = parse_spack_depends_on(text, &mut notes);

    let resource_calls = python_call_spans(text, "resource");
    if !resource_calls.is_empty() {
        notes.push(format!(
            "{} conditional resource() fetch(es) included in planned sources",
            resource_calls.len()
        ));
    }

    let primary_url = if preferred.url.is_some() {
        preferred.url.clone()
    } else if text.contains("def url_for_version") {
        let materialized =
            materialize_spack_url_for_version(text, &preferred.version, url.as_deref());
        if materialized.is_some() {
            notes.push(format!(
                "materialized dynamic url_for_version for {}",
                preferred.version
            ));
        } else {
            notes.push(
                "dynamic url_for_version could not be materialized; stale class URL ignored".into(),
            );
        }
        materialized
    } else {
        url.clone()
    };

    // Keep http(s) `url` separate from `git` so tag→archive materialization can run.
    let mut sources = vec![ForeignSource {
        url: primary_url,
        filename: None,
        sha256: preferred.sha256.clone(),
        git: git.clone(),
        tag: preferred.tag.clone(),
        commit: preferred.commit.clone(),
        target_directory: None,
    }];
    if sources[0].url.is_none() && sources[0].git.is_none() {
        sources[0].git = git.clone();
    }

    // resource(name=..., url=..., sha256=..., destination=..., placement=...)
    let placement_re = Regex::new(r#"placement\s*=\s*\{[^:}]+:\s*[\"']([^\"']+)[\"']\s*\}"#)
        .expect("resource placement re");
    for call in resource_calls {
        let inner = &text[call.inner_start..call.end.saturating_sub(1)];
        if let Some(url) = spack_string_kwarg(inner, "url") {
            let target_directory = spack_string_kwarg(inner, "destination").or_else(|| {
                placement_re
                    .captures(inner)
                    .and_then(|capture| capture.get(1).map(|value| value.as_str().to_string()))
            });
            sources.push(ForeignSource {
                url: Some(url),
                filename: spack_string_kwarg(inner, "name"),
                sha256: spack_string_kwarg(inner, "sha256"),
                target_directory,
                ..Default::default()
            });
        }
    }

    let summary = spack_docstring(text);
    let license = spack_license(text);
    let variants = parse_spack_variants(text, &mut notes);
    let rules = parse_spack_rules(text, &mut notes);

    // Meson/CMake from bases
    if build_system_hints.is_empty() {
        build_system_hints = bases;
    }

    let configopts = extract_spack_config_flags(text);
    if let Some(ref c) = configopts {
        notes.push(format!(
            "extracted {} configure flag token(s) from meson_args/cmake_args",
            c.split_whitespace().count()
        ));
    }

    let patch_n = Regex::new(r"(?m)^\s*patch\s*\(")
        .ok()
        .map(|r| r.find_iter(text).count())
        .unwrap_or(0);
    let patches = Vec::new();
    if patch_n > 0 {
        notes.push(format!(
            "{patch_n} patch() directive(s) — recorded as residual patch count"
        ));
    }

    Ok(ForeignRecipe {
        format: ForeignFormat::Spack,
        name,
        version: preferred.version.clone(),
        homepage,
        source_url: sources[0].url.clone(),
        source_filename: None,
        sha256: preferred.sha256.clone(),
        sources,
        summary: summary.clone(),
        description: summary,
        license,
        dependencies,
        build_system_hints,
        configopts,
        patches,
        variants,
        rules,
        notes,
    })
}

fn spack_license(text: &str) -> Option<String> {
    // license("MIT") or license("NCSA", checked_by=...)
    let re = Regex::new(r#"(?m)^\s*license\s*\(\s*[\"']([^\"']+)[\"']"#).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

#[derive(Debug, Clone, Copy)]
struct PythonCallSpan {
    start: usize,
    inner_start: usize,
    end: usize,
}

fn python_call_spans(text: &str, name: &str) -> Vec<PythonCallSpan> {
    let pattern = format!(r"(?m)^[ \t]*{}\s*\(", regex::escape(name));
    let Ok(regex) = Regex::new(&pattern) else {
        return Vec::new();
    };
    regex
        .find_iter(text)
        .filter_map(|matched| {
            let relative_name = matched.as_str().find(name)?;
            let start = matched.start() + relative_name;
            let inner_start = matched.end();
            let end = python_call_end(text, inner_start)?;
            Some(PythonCallSpan {
                start,
                inner_start,
                end,
            })
        })
        .collect()
}

fn python_call_end(text: &str, inner_start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = inner_start;
    let mut depth = 1usize;
    let mut quote: Option<u8> = None;
    let mut triple = false;

    while index < bytes.len() {
        if let Some(delimiter) = quote {
            if bytes[index] == b'\\' {
                index = (index + 2).min(bytes.len());
                continue;
            }
            if triple {
                if index + 2 < bytes.len()
                    && bytes[index] == delimiter
                    && bytes[index + 1] == delimiter
                    && bytes[index + 2] == delimiter
                {
                    quote = None;
                    triple = false;
                    index += 3;
                    continue;
                }
            } else if bytes[index] == delimiter {
                quote = None;
                index += 1;
                continue;
            }
            index += 1;
            continue;
        }

        match bytes[index] {
            b'#' => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'\'' | b'"' => {
                let delimiter = bytes[index];
                triple = index + 2 < bytes.len()
                    && bytes[index + 1] == delimiter
                    && bytes[index + 2] == delimiter;
                quote = Some(delimiter);
                index += if triple { 3 } else { 1 };
            }
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn parse_spack_variants(text: &str, notes: &mut Vec<String>) -> Vec<ForeignVariant> {
    let mut out = Vec::new();
    for call in python_call_spans(text, "variant") {
        let inner = &text[call.inner_start..call.end.saturating_sub(1)];
        let provenance = provenance_for_range(
            text,
            call.start,
            call.end,
            "spack-static",
            Confidence::Exact,
        );
        if let Some(variant) = parse_spack_variant_call(inner, provenance) {
            out.push(variant);
        }
    }
    if !out.is_empty() {
        notes.push(format!(
            "spack: {} variant() directive(s) extracted",
            out.len()
        ));
    }
    out
}

fn parse_spack_variant_call(inner: &str, provenance: Provenance) -> Option<ForeignVariant> {
    let name_re = Regex::new(r#"^\s*[\"']([^\"']+)[\"']"#).ok()?;
    let name = name_re.captures(inner)?.get(1)?.as_str().to_string();
    let default = Regex::new(r#"default\s*=\s*([^\s,]+)"#)
        .ok()
        .and_then(|r| r.captures(inner))
        .and_then(|c| {
            c.get(1).map(|m| {
                m.as_str()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string()
            })
        });
    let description = Regex::new(r#"description\s*=\s*[\"']([^\"']*)[\"']"#)
        .ok()
        .and_then(|r| r.captures(inner))
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
    Some(ForeignVariant {
        name,
        default,
        description,
        condition: ConditionExpr::Always,
        provenance: vec![provenance],
    })
}

fn parse_spack_rules(text: &str, notes: &mut Vec<String>) -> Vec<ForeignRule> {
    let mut rules = Vec::new();
    for (name, kind) in [
        ("conflicts", ForeignRuleKind::Conflict),
        ("requires", ForeignRuleKind::Requirement),
    ] {
        for call in python_call_spans(text, name) {
            let inner = &text[call.inner_start..call.end.saturating_sub(1)];
            let provenance = provenance_for_range(
                text,
                call.start,
                call.end,
                "spack-static",
                Confidence::Exact,
            );
            if let Some(rule) = parse_spack_rule_call(inner, kind, provenance) {
                rules.push(rule);
            }
        }
    }
    let conflicts = rules
        .iter()
        .filter(|rule| rule.kind == ForeignRuleKind::Conflict)
        .count();
    let requirements = rules
        .iter()
        .filter(|rule| rule.kind == ForeignRuleKind::Requirement)
        .count();
    if conflicts > 0 {
        notes.push(format!(
            "spack: {conflicts} conflicts() directive(s) preserved"
        ));
    }
    if requirements > 0 {
        notes.push(format!(
            "spack: {requirements} requires() directive(s) preserved"
        ));
    }
    rules.sort_by_key(|rule| rule.provenance.span.start_line);
    rules
}

fn parse_spack_rule_call(
    inner: &str,
    kind: ForeignRuleKind,
    provenance: Provenance,
) -> Option<ForeignRule> {
    let spec = Regex::new(r#"^\s*[\"']([^\"']+)[\"']"#)
        .ok()?
        .captures(inner)?
        .get(1)?
        .as_str()
        .to_string();
    let when = spack_string_kwarg(inner, "when");
    let condition = when
        .as_deref()
        .map(parse_spack_condition)
        .unwrap_or(ConditionExpr::Always);
    Some(ForeignRule {
        kind,
        spec,
        when,
        condition,
        message: spack_string_kwarg(inner, "msg"),
        provenance,
    })
}

#[derive(Debug, Clone)]
struct SpackVersion {
    version: String,
    preferred: bool,
    sha256: Option<String>,
    tag: Option<String>,
    commit: Option<String>,
    url: Option<String>,
}

fn parse_spack_versions(text: &str) -> Result<Vec<SpackVersion>, ForeignError> {
    let mut out = Vec::new();
    for call in python_call_spans(text, "version") {
        let inner = &text[call.inner_start..call.end.saturating_sub(1)];
        if let Some(version) = parse_spack_version_call(inner) {
            out.push(version);
        }
    }
    Ok(out)
}

fn parse_spack_version_call(inner: &str) -> Option<SpackVersion> {
    // First positional string is the version id
    let ver_re = Regex::new(r#"^\s*[\"']([^\"']+)[\"']"#).ok()?;
    let version = ver_re.captures(inner)?.get(1)?.as_str().to_string();
    let kw = |key: &str| -> Option<String> {
        let re = Regex::new(&format!(r#"{key}\s*=\s*[\"']([^\"']*)[\"']"#)).ok()?;
        re.captures(inner)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
    };
    Some(SpackVersion {
        version,
        preferred: Regex::new(r"\bpreferred\s*=\s*True\b")
            .ok()
            .is_some_and(|regex| regex.is_match(inner)),
        sha256: kw("sha256"),
        tag: kw("tag"),
        commit: kw("commit"),
        url: kw("url"),
    })
}

fn pick_preferred_spack_version(versions: &[SpackVersion]) -> SpackVersion {
    let is_floating = |v: &str| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "develop" | "main" | "master" | "head" | "stable" | "latest"
        )
    };
    versions
        .iter()
        .find(|version| version.preferred && !is_floating(&version.version))
        .or_else(|| {
            versions
                .iter()
                .find(|version| !is_floating(&version.version))
        })
        .or_else(|| versions.first())
        .cloned()
        .expect("versions non-empty")
}

fn materialize_spack_url_for_version(
    text: &str,
    version: &str,
    class_url: Option<&str>,
) -> Option<String> {
    if !text.contains("datetime.strptime")
        || !text.contains("stable_versions")
        || !text.contains("_update")
    {
        return None;
    }

    let (date, update) = version
        .split_once('.')
        .map(|(date, update)| (date, format!("_update{update}")))
        .unwrap_or((version, String::new()));
    let date = NaiveDate::parse_from_str(date, "%Y%m%d").ok()?;
    let stable_block = Regex::new(r"(?s)stable_versions\s*=\s*\{(.*?)\}")
        .ok()?
        .captures(text)?
        .get(1)?
        .as_str()
        .to_string();
    let quoted_version = format!("\"{version}\"");
    let single_quoted_version = format!("'{version}'");
    let release_kind = if stable_block.contains(&quoted_version)
        || stable_block.contains(&single_quoted_version)
    {
        "stable"
    } else {
        "patch"
    };
    let date = date.format("%d%b%Y").to_string();
    let date = date.trim_start_matches('0');
    let prefix = class_url?.split_once("/archive/")?.0;
    Some(format!(
        "{prefix}/archive/{release_kind}_{date}{update}.tar.gz"
    ))
}

fn parse_spack_depends_on(text: &str, notes: &mut Vec<String>) -> Vec<ForeignDep> {
    let mut out = Vec::new();
    for call in python_call_spans(text, "depends_on") {
        let inner = &text[call.inner_start..call.end.saturating_sub(1)];
        let provenance = provenance_for_range(
            text,
            call.start,
            call.end,
            "spack-static",
            Confidence::Exact,
        );
        let inherited_condition = spack_scoped_when_condition(text, call.start, notes);
        if let Some(dependency) =
            parse_spack_depends_on_call(inner, notes, provenance, inherited_condition)
        {
            out.push(dependency);
        }
    }
    out
}

fn parse_spack_depends_on_call(
    inner: &str,
    notes: &mut Vec<String>,
    provenance: Provenance,
    inherited_condition: ConditionExpr,
) -> Option<ForeignDep> {
    let spec_re = Regex::new(r#"^\s*[\"']([^\"']+)[\"']"#).ok()?;
    let spec = spec_re.captures(inner)?.get(1)?.as_str();
    let (dep_name, pin) = split_spack_spec(spec);

    if matches!(dep_name.as_str(), "c" | "cxx" | "fortran") {
        notes.push(format!("skipped language virtual depends_on({spec})"));
        return None;
    }

    // type="build" or type=("build", "run") or type=["build","run"]
    let role = {
        if let Some(c) = Regex::new(r#"type\s*=\s*[\"']([^\"']+)[\"']"#)
            .ok()
            .and_then(|r| r.captures(inner))
        {
            c.get(1).unwrap().as_str().to_string()
        } else if let Some(c) = Regex::new(r#"type\s*=\s*[\(\[]([^\)\]]+)[\)\]]"#)
            .ok()
            .and_then(|r| r.captures(inner))
        {
            // join multiple types
            let inner_types = c.get(1).unwrap().as_str();
            let types: Vec<String> = Regex::new(r#"[\"']([^\"']+)[\"']"#)
                .ok()
                .map(|r| {
                    r.captures_iter(inner_types)
                        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            if types.is_empty() {
                "run".into()
            } else {
                types.join("+")
            }
        } else {
            "run".into()
        }
    };

    let when = spack_string_kwarg(inner, "when");
    let direct_condition = when
        .as_deref()
        .map(parse_spack_condition)
        .unwrap_or(ConditionExpr::Always);
    let condition = condition_all(inherited_condition, direct_condition);
    if let Some(when) = &when {
        notes.push(format!("depends_on({spec}) condition preserved: {when}"));
    }

    Some(ForeignDep {
        name: spack_dep_to_eb_name(&dep_name),
        pin,
        role,
        original_spec: Some(spec.into()),
        condition,
        provenance: vec![provenance],
    })
}

fn spack_scoped_when_condition(
    text: &str,
    call_start: usize,
    notes: &mut Vec<String>,
) -> ConditionExpr {
    let prefix = &text[..call_start];
    let call_indent = prefix
        .rsplit('\n')
        .next()
        .map(|line| {
            line.chars()
                .take_while(|character| character.is_whitespace())
                .count()
        })
        .unwrap_or(0);
    let mut scope_indent = call_indent;

    for line in prefix.lines().rev().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line
            .chars()
            .take_while(|character| character.is_whitespace())
            .count();
        if indent >= scope_indent {
            continue;
        }
        scope_indent = indent;
        let Some(expression) = trimmed
            .strip_prefix("with when(")
            .and_then(|value| value.strip_suffix("):"))
        else {
            continue;
        };
        return materialize_spack_scoped_when(expression.trim(), prefix, notes);
    }

    ConditionExpr::Always
}

fn materialize_spack_scoped_when(
    expression: &str,
    prefix: &str,
    notes: &mut Vec<String>,
) -> ConditionExpr {
    if expression.len() >= 2 {
        let first = expression.as_bytes()[0];
        let last = expression.as_bytes()[expression.len() - 1];
        if matches!(first, b'\'' | b'"') && first == last {
            return parse_spack_condition(&expression[1..expression.len() - 1]);
        }
    }

    let binding_pattern = format!(
        r#"(?m)^[ \t]*for\s+{}\s+in\s*\(([^)]*)\)\s*:\s*$"#,
        regex::escape(expression)
    );
    let values = Regex::new(&binding_pattern)
        .ok()
        .and_then(|regex| regex.captures_iter(prefix).last())
        .and_then(|capture| capture.get(1).map(|values| values.as_str().to_string()))
        .map(|values| {
            Regex::new(r#"[\"']([^\"']+)[\"']"#)
                .ok()
                .map(|regex| {
                    regex
                        .captures_iter(&values)
                        .filter_map(|capture| capture.get(1))
                        .map(|value| parse_spack_condition(value.as_str()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();

    match values.len() {
        0 => {
            notes.push(format!(
                "dynamic scoped when({expression}) could not be materialized; nested dependency is inactive"
            ));
            ConditionExpr::Never
        }
        1 => values.into_iter().next().unwrap_or(ConditionExpr::Never),
        _ => ConditionExpr::Any(values),
    }
}

fn spack_string_kwarg(inner: &str, key: &str) -> Option<String> {
    let regex = Regex::new(&format!(r#"{key}\s*=\s*[\"']([^\"']*)[\"']"#)).ok()?;
    regex
        .captures(inner)
        .and_then(|capture| capture.get(1).map(|value| value.as_str().to_string()))
}

fn parse_spack_condition(spec: &str) -> ConditionExpr {
    let terms = split_spack_condition_terms(spec)
        .into_iter()
        .filter_map(parse_spack_condition_term)
        .collect::<Vec<_>>();
    match terms.len() {
        0 => ConditionExpr::Always,
        1 => terms.into_iter().next().unwrap_or(ConditionExpr::Always),
        _ => ConditionExpr::All(terms),
    }
}

fn split_spack_condition_terms(spec: &str) -> Vec<&str> {
    let mut terms = Vec::new();
    let mut start = 0usize;
    let mut bracket_depth = 0usize;
    for (index, character) in spec.char_indices() {
        match character {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            character if character.is_whitespace() && bracket_depth == 0 => {
                if start < index {
                    terms.push(spec[start..index].trim());
                }
                start = index + character.len_utf8();
            }
            character if bracket_depth == 0 && start < index => {
                let first = spec[start..].chars().next();
                let starts_version = first == Some('@');
                let starts_feature = matches!(first, Some('+') | Some('~'));
                let starts_new_term = (starts_version
                    && matches!(character, '+' | '~' | '%' | '^'))
                    || (starts_feature && matches!(character, '+' | '~'));
                if starts_new_term {
                    terms.push(spec[start..index].trim());
                    start = index;
                }
            }
            _ => {}
        }
    }
    if start < spec.len() {
        terms.push(spec[start..].trim());
    }
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

fn parse_spack_condition_term(term: &str) -> Option<ConditionExpr> {
    if let Some(feature) = term.strip_prefix('+') {
        return Some(ConditionExpr::Predicate(ConditionPredicate::Feature {
            name: feature.into(),
            enabled: true,
        }));
    }
    if let Some(feature) = term.strip_prefix('~') {
        return Some(ConditionExpr::Predicate(ConditionPredicate::Feature {
            name: feature.into(),
            enabled: false,
        }));
    }
    if let Some(range) = term.strip_prefix('@') {
        return Some(ConditionExpr::Predicate(
            ConditionPredicate::PackageVersion {
                requirement: spack_version_range(range),
            },
        ));
    }
    if let Some(compiler) = term.strip_prefix('%') {
        let (name, version) = compiler
            .split_once('@')
            .map(|(name, version)| (name, Some(spack_version_range(version))))
            .unwrap_or((compiler, None));
        return Some(ConditionExpr::Predicate(ConditionPredicate::Compiler {
            name: name.into(),
            version,
        }));
    }
    if let Some(dependency) = term.strip_prefix('^') {
        let dependency = dependency
            .trim_start_matches('[')
            .split([']', '@', '+', '~', '%'])
            .find(|part| !part.is_empty() && !part.contains('='))
            .unwrap_or(dependency);
        return Some(ConditionExpr::Predicate(
            ConditionPredicate::DependencyFeature {
                dependency: dependency.into(),
                name: "selected".into(),
                enabled: true,
            },
        ));
    }
    if let Some((name, value)) = term.split_once('=') {
        let name = name.trim();
        let value = value.trim();
        if !name.is_empty() && !value.is_empty() {
            return Some(ConditionExpr::Predicate(
                ConditionPredicate::VariableComparison {
                    left: name.into(),
                    operator: "==".into(),
                    right: value.into(),
                },
            ));
        }
    }
    None
}

fn spack_version_range(range: &str) -> String {
    if let Some((minimum, maximum)) = range.split_once(':') {
        match (minimum.trim(), maximum.trim()) {
            ("", "") => String::new(),
            ("", maximum) => format!("<={maximum}"),
            (minimum, "") => format!(">={minimum}"),
            (minimum, maximum) => format!(">={minimum},<={maximum}"),
        }
    } else {
        format!("=={}", range.trim())
    }
}

fn spack_class_to_pkg_name(class_name: &str) -> String {
    // Spack: directory name is usually hyphenated; class PyFoo -> py-foo
    if let Some(rest) = class_name.strip_prefix("Py") {
        if !rest.is_empty() && rest.starts_with(|c: char| c.is_uppercase()) {
            return format!("py-{}", camel_to_kebab(rest));
        }
    }
    // R packages: RFoo -> r-foo is less common; keep camel_to_kebab
    camel_to_kebab(class_name)
}

fn camel_to_kebab(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                let prev = chars[i - 1];
                let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
                if prev.is_lowercase() || next_lower {
                    out.push('-');
                }
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn spack_dep_to_eb_name(name: &str) -> String {
    name.to_string()
}

fn split_spack_spec(spec: &str) -> (String, Option<String>) {
    // Package identity stops before versions, variants, compilers, and
    // assignment tokens. The complete spec remains in `original_spec`.
    let name_end = spec
        .char_indices()
        .find_map(|(index, character)| {
            (character.is_whitespace() || matches!(character, '@' | '+' | '~' | '%'))
                .then_some(index)
        })
        .unwrap_or(spec.len());
    let name = spec[..name_end].to_string();
    let pin = spec.find('@').and_then(|at| {
        let version = &spec[at + 1..];
        let end = version
            .char_indices()
            .find_map(|(index, character)| {
                (character.is_whitespace() || matches!(character, '+' | '~' | '%')).then_some(index)
            })
            .unwrap_or(version.len());
        (end > 0).then(|| version[..end].to_string())
    });
    (name, pin)
}

fn spack_string_attr(text: &str, attr: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?m)^\s*{attr}\s*=\s*[\"']([^\"']+)[\"']"#)).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

fn spack_docstring(text: &str) -> Option<String> {
    let re = Regex::new(r#"(?s)class\s+\w+\s*\([^)]*\)\s*:\s*(?:r)?\"\"\"(.+?)\"\"\""#).ok()?;
    re.captures(text).and_then(|c| {
        c.get(1)
            .map(|m| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONDA_ZLIB: &str = r#"
package:
  name: zlib
  version: 1.3.1
source:
  url: https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz
  sha256: 9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23
requirements:
  build:
    - make
  run:
    - libgcc-ng
about:
  home: https://zlib.net/
  summary: zlib compression library
"#;

    const SPACK_ZLIB: &str = r#"
class Zlib(Package):
    """A free compression library."""

    homepage = "https://zlib.net"
    url = "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"

    version("1.3.1", sha256="9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23")
    version("1.2.13", sha256="b3a24de97a8fdbc835b9833169501030b8977031bcb54b3b3ac13740f846ab30")

    depends_on("c", type="build")
    depends_on("gmake", type="build")
"#;

    const SPACK_MULTI_BASE: &str = r#"
class Qmcpack(CMakePackage, CudaPackage):
    """QMCPACK."""

    homepage = "https://www.qmcpack.org/"
    git = "https://github.com/QMCPACK/qmcpack.git"

    version("develop")
    version("4.3.0", tag="v4.3.0", commit="bb7eede051f98ec03296664b304982e655f960c4")
    version("4.2.0", tag="v4.2.0", commit="44a7f7e99a5770ea368b8ea35b181329606bc343")

    depends_on("c", type="build")
    depends_on("cxx", type="build")
    depends_on("cmake@3.17.0:", when="@3.16.0:", type="build")
    depends_on("boost@1.61.0:+exception+serialization+random", when="@3.6.0:", type="build")
    depends_on("libxml2")
    depends_on("mpi", when="+mpi")
    depends_on("python@3.10:", when="@4.3:", type=("build", "run", "test"))
    depends_on("hdf5~mpi", when="~phdf5")
    depends_on("blas")
    depends_on("lapack")
"#;

    const CONDA_RATTLE_EON_MIN: &str = r#"
context:
  version: "2.16.0"

package:
  name: eon
  version: ${{ version }}

source:
  - url: https://github.com/TheochemUI/eOn/releases/download/v${{ version }}/eon-v${{ version }}.tar.xz
    sha256: 3d4da89a393c8821bf370cb97c9d2403718d83f9cbb5e8b918cd90af14ed52dc
  - url: https://github.com/OmniPotentRPC/rgpot/archive/refs/tags/v2.2.1.tar.gz
    sha256: d4687bc719e19174e89288dd16dd45d7a8645d7205c7f8d8fc4d677266055918
    target_directory: subprojects/rgpot

requirements:
  build:
    - ${{ compiler('c') }}
    - cmake
    - ninja
    - meson
  host:
    - python
    - numpy
    - xtb
    - libmetatomic-torch >=0.1.15,<0.2
  run:
    - python
    - xtb
    - quill

about:
  homepage: https://eondocs.org/
  summary: "Algorithms for long time scales"
"#;

    #[test]
    fn parse_conda_zlib_fields() {
        let r = parse_conda_forge(CONDA_ZLIB).expect("conda");
        assert_eq!(r.name, "zlib");
        assert_eq!(r.version, "1.3.1");
        assert!(r.source_url.as_ref().unwrap().contains("zlib-1.3.1.tar.gz"));
        assert_eq!(
            r.sha256.as_deref(),
            Some("9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23")
        );
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"make"), "{names:?}");
        assert!(names.contains(&"libgcc-ng"), "{names:?}");
    }

    #[test]
    fn parse_conda_rattler_context_and_multi_source() {
        let r = parse_conda_forge(CONDA_RATTLE_EON_MIN).expect("rattler eon");
        assert_eq!(r.name, "eon");
        assert_eq!(r.version, "2.16.0");
        assert_eq!(r.sources.len(), 2, "multi-source: {:?}", r.sources);
        assert!(
            r.source_url
                .as_ref()
                .unwrap()
                .contains("eon-v2.16.0.tar.xz"),
            "{:?}",
            r.source_url
        );
        assert!(
            r.sha256.as_ref().unwrap().starts_with("3d4da89a"),
            "{:?}",
            r.sha256
        );
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"meson"), "{names:?}");
        assert!(names.contains(&"xtb"), "{names:?}");
        assert!(names.contains(&"libmetatomic-torch"), "{names:?}");
        assert!(
            !names.iter().any(|n| n.contains("compiler")),
            "compiler macros skipped: {names:?}"
        );
        assert_eq!(r.homepage.as_deref(), Some("https://eondocs.org/"));
    }

    #[test]
    fn parse_spack_zlib_fields() {
        let r = parse_spack_package(SPACK_ZLIB).expect("spack");
        assert_eq!(r.name, "zlib");
        assert_eq!(r.version, "1.3.1");
        assert!(r.source_url.as_ref().unwrap().contains("zlib-1.3.1"));
        assert!(r.sha256.is_some());
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"gmake"), "{names:?}");
        assert!(!names.contains(&"c"), "language virtual skipped");
    }

    #[test]
    fn parse_spack_multi_base_and_prefer_non_develop() {
        let r = parse_spack_package(SPACK_MULTI_BASE).expect("qmcpack-like");
        assert_eq!(r.name, "qmcpack");
        assert_eq!(r.version, "4.3.0", "skip develop");
        assert!(
            r.build_system_hints.iter().any(|h| h.contains("CMake")),
            "{:?}",
            r.build_system_hints
        );
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"boost"), "{names:?}");
        assert!(names.contains(&"hdf5"), "{names:?}");
        assert!(names.contains(&"python"), "{names:?}");
        assert!(names.contains(&"libxml2"), "{names:?}");
        // multi-type
        let py = r.dependencies.iter().find(|d| d.name == "python").unwrap();
        assert!(
            py.role.contains("build") && py.role.contains("run"),
            "role={}",
            py.role
        );
    }

    #[test]
    fn detect_format_by_filename() {
        assert_eq!(
            detect_foreign_format(Path::new("/x/meta.yaml")),
            Some(ForeignFormat::CondaForge)
        );
        assert_eq!(
            detect_foreign_format(Path::new("recipe.yaml")),
            Some(ForeignFormat::CondaForge)
        );
        assert_eq!(
            detect_foreign_format(Path::new("package.py")),
            Some(ForeignFormat::Spack)
        );
        assert_eq!(detect_foreign_format(Path::new("foo.eb")), None);
    }
}
