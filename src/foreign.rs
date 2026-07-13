//! Ingest foreign package recipes (conda-forge, Spack) into EasyBuild scaffolds.
//!
//! Parsers are intentionally restricted and pure: recipe text → [`ForeignRecipe`]
//! intermediate fields → parseable `.eb` text. They do **not** execute Jinja,
//! Spack Python, or invent EasyBuild generation-native dependency versions.
//! Residuals surface as header warnings in the emitted file and in
//! [`IngestResult::warnings`].

use crate::domain::Toolchain;
use crate::eb_emit::easyconfig_filename;
use crate::eb_parse::{companion_easyconfig_basename, easyconfig_letter_dir};
use regex::Regex;
use serde::{Deserialize, Serialize};
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
    /// Pin string as written in the foreign recipe when present (e.g. `>=12`).
    pub pin: Option<String>,
    /// Role: `build`, `host`, `run`, or Spack `type=...` when known.
    pub role: String,
}

/// Intermediate fields shared by all foreign formats before EB emit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignRecipe {
    pub format: ForeignFormat,
    pub name: String,
    pub version: String,
    pub homepage: Option<String>,
    pub source_url: Option<String>,
    pub source_filename: Option<String>,
    pub sha256: Option<String>,
    pub summary: Option<String>,
    pub dependencies: Vec<ForeignDep>,
    /// Human notes from the parser (Jinja skipped, multi-version, etc.).
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

/// Result of ingest → EB emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestResult {
    pub recipe: ForeignRecipe,
    pub filename: String,
    pub text: String,
    pub warnings: Vec<String>,
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
    let text = std::fs::read_to_string(path).map_err(|e| {
        ForeignError::Io(format!("read {}: {e}", path.display()))
    })?;
    let fmt = format
        .or_else(|| detect_foreign_format(path))
        .ok_or_else(|| ForeignError::Unsupported(path.display().to_string()))?;
    parse_foreign_str(fmt, &text)
}

/// Emit a parseable EasyBuild scaffold from intermediate fields.
///
/// Dependency **versions** are never treated as EB generation consensus: when a
/// foreign pin supplies an exact version token it is used as a residual pin with
/// a warning; otherwise the dep is listed only in the header comments (names
/// still appear in the file for review).
pub fn emit_easyconfig_from_foreign(
    recipe: &ForeignRecipe,
    toolchain: &Toolchain,
) -> IngestResult {
    let mut warnings = recipe.notes.clone();
    warnings.push(format!(
        "ingested from {}; toolchain/easyblock/build logic are residual — review before `eb` install",
        recipe.format.as_str()
    ));

    let homepage = recipe
        .homepage
        .clone()
        .unwrap_or_else(|| format!("https://example.invalid/TODO-{}", recipe.name));
    let summary = recipe
        .summary
        .clone()
        .unwrap_or_else(|| {
            format!(
                "Scaffold from {} for {}-{}",
                recipe.format.as_str(),
                recipe.name,
                recipe.version
            )
        });

    let (source_urls_line, sources_line, checksums_line) = source_block(recipe, &mut warnings);

    let mut dep_comment_lines: Vec<String> = Vec::new();
    let mut dep_tuples: Vec<String> = Vec::new();
    for d in &recipe.dependencies {
        let pin_note = d.pin.as_deref().unwrap_or("");
        dep_comment_lines.push(format!(
            "#   - {} [{}]{}",
            d.name,
            d.role,
            if pin_note.is_empty() {
                String::new()
            } else {
                format!(" pin={pin_note}")
            }
        ));
        if let Some(exact) = d.pin.as_deref().and_then(exact_version_token) {
            dep_tuples.push(format!("    ('{}', '{}'),", d.name, exact));
            warnings.push(format!(
                "dependency {} uses foreign pin version {exact} — not EB generation consensus; re-resolve with bump --easyconfigs",
                d.name
            ));
        } else {
            warnings.push(format!(
                "dependency {} ({}) has no exact version pin — name listed in header only",
                d.name, d.role
            ));
        }
    }

    let deps_block = if dep_tuples.is_empty() {
        "dependencies = []\n".to_string()
    } else {
        format!(
            "dependencies = [\n{}\n]\n",
            dep_tuples.join("\n")
        )
    };

    let tc_line = format!(
        "toolchain = {{'name': '{}', 'version': '{}'}}",
        toolchain.name, toolchain.version
    );

    let warn_header: String = warnings
        .iter()
        .map(|w| format!("# WARNING: {w}"))
        .collect::<Vec<_>>()
        .join("\n");

    let foreign_deps_header = if dep_comment_lines.is_empty() {
        "# Foreign dependencies: (none extracted)\n".to_string()
    } else {
        format!(
            "# Foreign-origin dependencies (names from {}; versions not EB-resolved):\n{}\n",
            recipe.format.as_str(),
            dep_comment_lines.join("\n")
        )
    };

    let text = format!(
        r#"# EasyBuild scaffold generated by eb-stack ingest ({origin}).
# Residuals: easyblock choice, build steps, and EB generation-native dep versions.
{warn_header}
{foreign_deps_header}easyblock = 'ConfigureMake'

name = '{name}'
version = '{version}'
homepage = '{homepage}'
description = """{summary}

Origin: {origin}. Fill easyblock/build options before production install."""

{tc_line}

{source_urls_line}
{sources_line}
{checksums_line}
builddependencies = []

{deps_block}
sanity_check_paths = {{
    'files': [],
    'dirs': ['lib', 'include'],
}}

moduleclass = 'lib'
"#,
        origin = recipe.format.as_str(),
        warn_header = warn_header,
        foreign_deps_header = foreign_deps_header,
        name = recipe.name,
        version = recipe.version,
        homepage = escape_py_single(&homepage),
        summary = escape_py_triple(&summary),
        tc_line = tc_line,
        source_urls_line = source_urls_line,
        sources_line = sources_line,
        checksums_line = checksums_line,
        deps_block = deps_block,
    );

    let filename = easyconfig_filename(&recipe.name, &recipe.version, toolchain);
    IngestResult {
        recipe: recipe.clone(),
        filename,
        text,
        warnings,
    }
}

/// Parse path, emit scaffold, optionally write to out path or conventional name.
pub fn ingest_foreign_to_easyconfig(
    path: &Path,
    format: Option<ForeignFormat>,
    toolchain: &Toolchain,
) -> Result<IngestResult, ForeignError> {
    let recipe = parse_foreign_path(path, format)?;
    Ok(emit_easyconfig_from_foreign(&recipe, toolchain))
}

fn source_block(recipe: &ForeignRecipe, warnings: &mut Vec<String>) -> (String, String, String) {
    if let Some(url) = &recipe.source_url {
        let fname = recipe
            .source_filename
            .clone()
            .unwrap_or_else(|| filename_from_url(url));
        // Split URL into base + file when possible.
        let (base, file) = split_url_base_file(url, &fname);
        let source_urls = format!("source_urls = ['{base}']");
        let sources = format!("sources = ['{file}']");
        let checksums = if let Some(sum) = &recipe.sha256 {
            format!("checksums = ['{sum}']")
        } else {
            warnings.push(
                "no sha256 in foreign recipe; checksums left as 64-zero placeholder".into(),
            );
            format!("checksums = ['{}']", "0".repeat(64))
        };
        (source_urls, sources, checksums)
    } else {
        warnings.push("no source URL in foreign recipe; placeholder source used".into());
        (
            "source_urls = ['https://example.invalid/TODO/']".into(),
            "sources = [SOURCE_TAR_GZ]".into(),
            format!("checksums = ['{}']", "0".repeat(64)),
        )
    }
}

fn filename_from_url(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("source.tar.gz")
        .to_string()
}

fn split_url_base_file<'a>(url: &'a str, fname: &str) -> (String, String) {
    if let Some(pos) = url.rfind('/') {
        let base = url[..=pos].to_string();
        let file = url[pos + 1..].to_string();
        if file.is_empty() {
            (base, fname.to_string())
        } else {
            (base, file)
        }
    } else {
        (url.to_string(), fname.to_string())
    }
}

/// Exact version token from a pin (`1.2.3`, `==1.2.3`) — not ranges.
fn exact_version_token(pin: &str) -> Option<String> {
    let p = pin.trim();
    if p.is_empty() || p == "*" {
        return None;
    }
    let p = p.strip_prefix("==").unwrap_or(p).trim();
    if p.starts_with('>') || p.starts_with('<') || p.starts_with('!') || p.contains(',') {
        return None;
    }
    // bare version-ish: digits and dots / letters
    if p.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && p.chars().any(|c| c.is_ascii_digit())
    {
        Some(p.to_string())
    } else {
        None
    }
}

fn escape_py_single(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn escape_py_triple(s: &str) -> String {
    s.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"")
}

// ---------------------------------------------------------------------------
// conda-forge (classic meta.yaml / plain recipe.yaml subset)
// ---------------------------------------------------------------------------

fn parse_conda_forge(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut notes = Vec::new();
    if text.contains("{{") || text.contains("{%") {
        notes.push(
            "Jinja constructs present; parser reads plain YAML-like keys only (no Jinja eval)"
                .into(),
        );
    }

    let name = yaml_scalar_under(text, "package", "name")
        .or_else(|| yaml_top_scalar(text, "name"))
        .ok_or_else(|| ForeignError::Parse("conda: missing package.name".into()))?;
    let version = yaml_scalar_under(text, "package", "version")
        .or_else(|| yaml_top_scalar(text, "version"))
        .ok_or_else(|| ForeignError::Parse("conda: missing package.version".into()))?;

    let source_url = yaml_scalar_under(text, "source", "url")
        .or_else(|| yaml_scalar_under(text, "source", "git_url"));
    let source_filename = yaml_scalar_under(text, "source", "fn");
    let sha256 = yaml_scalar_under(text, "source", "sha256");
    let homepage = yaml_scalar_under(text, "about", "home")
        .or_else(|| yaml_scalar_under(text, "about", "homepage"));
    let summary = yaml_scalar_under(text, "about", "summary");

    let mut dependencies = Vec::new();
    for (section, role) in [("build", "build"), ("host", "host"), ("run", "run")] {
        for raw in yaml_list_under_requirements(text, section) {
            if let Some(dep) = parse_conda_dep_line(&raw, role) {
                // Skip Jinja/compiler macros.
                if dep.name.contains("{{") || dep.name.starts_with("{{") {
                    continue;
                }
                dependencies.push(dep);
            }
        }
    }

    Ok(ForeignRecipe {
        format: ForeignFormat::CondaForge,
        name: sanitize_pkg_name(&name),
        version: version.trim().to_string(),
        homepage,
        source_url,
        source_filename,
        sha256,
        summary,
        dependencies,
        notes,
    })
}

fn parse_conda_dep_line(raw: &str, role: &str) -> Option<ForeignDep> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // "name", "name version", "name >=1.2", "name 1.2.*"
    let mut parts = line.split_whitespace();
    let name = parts.next()?.to_string();
    let pin = {
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() {
            None
        } else {
            Some(rest.join(" "))
        }
    };
    Some(ForeignDep {
        name: sanitize_pkg_name(&name),
        pin,
        role: role.into(),
    })
}

/// Read `key: value` immediately under a top-level `section:` mapping.
fn yaml_scalar_under(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    let section_re = Regex::new(&format!(r"(?m)^({section})\s*:\s*$")).ok()?;
    let key_re = Regex::new(&format!(r"(?m)^[ \t]+({key})\s*:\s*(.+?)\s*$")).ok()?;
    for line in text.lines() {
        if section_re.is_match(line) {
            in_section = true;
            continue;
        }
        if in_section {
            // left a top-level key
            if Regex::new(r"(?m)^[A-Za-z_]").ok()?.is_match(line)
                && !line.starts_with(' ')
                && !line.starts_with('\t')
                && line.contains(':')
            {
                // another top-level section
                if !line.trim_start().starts_with('#') {
                    in_section = false;
                }
            }
        }
        if in_section {
            if let Some(c) = key_re.captures(line) {
                return Some(strip_yaml_value(c.get(2)?.as_str()));
            }
        }
    }
    None
}

fn yaml_top_scalar(text: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(r"(?m)^[ \t]*{key}\s*:\s*(.+?)\s*$")).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1).map(|m| strip_yaml_value(m.as_str())))
}

fn strip_yaml_value(v: &str) -> String {
    let v = v.trim();
    let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
    let v = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(v);
    // strip trailing comments
    match v.find('#') {
        Some(i) if !v[..i].trim().is_empty() => v[..i].trim().to_string(),
        _ => v.to_string(),
    }
}

fn yaml_list_under_requirements(text: &str, section: &str) -> Vec<String> {
    // requirements:\n  build:\n    - make\n
    let mut out = Vec::new();
    let mut in_req = false;
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if Regex::new(r"(?m)^requirements\s*:\s*$")
            .ok()
            .map(|r| r.is_match(trimmed))
            .unwrap_or(false)
        {
            in_req = true;
            in_section = false;
            continue;
        }
        if in_req {
            // left requirements (top-level key)
            if Regex::new(r"^[A-Za-z_]").ok().map(|r| r.is_match(trimmed)).unwrap_or(false)
                && trimmed.contains(':')
                && !trimmed.starts_with(' ')
            {
                break;
            }
            let sec_pat = format!(r"^[ \t]+{section}\s*:\s*$");
            if Regex::new(&sec_pat).ok().map(|r| r.is_match(trimmed)).unwrap_or(false) {
                in_section = true;
                continue;
            }
            // another subsection under requirements
            if in_section
                && Regex::new(r"^[ \t]+[A-Za-z_][A-Za-z0-9_]*\s*:\s*$")
                    .ok()
                    .map(|r| r.is_match(trimmed))
                    .unwrap_or(false)
            {
                in_section = false;
            }
            if in_section {
                if let Some(rest) = trimmed.trim_start().strip_prefix('-') {
                    let item = rest.trim();
                    if !item.is_empty() && item != "[]" {
                        out.push(item.to_string());
                    }
                }
            }
        }
    }
    out
}

fn sanitize_pkg_name(name: &str) -> String {
    // conda lowercases; keep EasyBuild-ish name but strip empties
    let n = name.trim();
    if n.is_empty() {
        return "unknown".into();
    }
    n.to_string()
}

// ---------------------------------------------------------------------------
// Spack package.py restricted parse (no exec)
// ---------------------------------------------------------------------------

fn parse_spack_package(text: &str) -> Result<ForeignRecipe, ForeignError> {
    let mut notes = Vec::new();
    notes.push("Spack package.py parsed with restricted regex — no Python execution".into());

    // class Zlib(Package) or class PyFoo(PythonPackage)
    let class_re = Regex::new(
        r"(?m)^class\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*([A-Za-z_][A-Za-z0-9_]*)\s*\)",
    )
    .map_err(|e| ForeignError::Parse(e.to_string()))?;
    let class_cap = class_re
        .captures(text)
        .ok_or_else(|| ForeignError::Parse("spack: no class Name(Base) found".into()))?;
    let class_name = class_cap.get(1).unwrap().as_str();
    let name = spack_class_to_pkg_name(class_name);

    let homepage = spack_string_attr(text, "homepage");
    let url = spack_string_attr(text, "url");
    let git = spack_string_attr(text, "git");
    let source_url = url.or(git);

    // version("1.3.1", sha256="...")
    let ver_re = Regex::new(
        r#"(?m)version\s*\(\s*[\"']([^\"']+)[\"']\s*(?:,\s*sha256\s*=\s*[\"']([0-9a-fA-F]{64})[\"'])?"#,
    )
    .map_err(|e| ForeignError::Parse(e.to_string()))?;
    let mut versions: Vec<(String, Option<String>)> = Vec::new();
    for c in ver_re.captures_iter(text) {
        let v = c.get(1).unwrap().as_str().to_string();
        let sum = c.get(2).map(|m| m.as_str().to_string());
        versions.push((v, sum));
    }
    if versions.is_empty() {
        return Err(ForeignError::Parse(
            "spack: no version(\"...\") directives found".into(),
        ));
    }
    // Prefer first version directive (Spack often lists preferred first).
    let (version, sha256) = versions[0].clone();
    if versions.len() > 1 {
        notes.push(format!(
            "multiple version() directives ({}); using first ({version})",
            versions.len()
        ));
    }

    // depends_on("foo") or depends_on("foo", type="build")
    let dep_re = Regex::new(
        r#"(?m)depends_on\s*\(\s*[\"']([^\"']+)[\"'](?:\s*,\s*type\s*=\s*[\"']([^\"']+)[\"'])?"#,
    )
    .map_err(|e| ForeignError::Parse(e.to_string()))?;
    let mut dependencies = Vec::new();
    for c in dep_re.captures_iter(text) {
        let raw = c.get(1).unwrap().as_str();
        let role = c
            .get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "run".into());
        // depends_on("foo@1.2") or "foo+bar"
        let (dep_name, pin) = split_spack_spec(raw);
        // Skip language virtuals that are not EB packages
        if matches!(dep_name.as_str(), "c" | "cxx" | "fortran" | "python")
            && role == "build"
            && pin.is_none()
            && dep_name == "c"
        {
            // still record as build note
            notes.push(format!("skipped language virtual depends_on({raw})"));
            continue;
        }
        if dep_name == "c" || dep_name == "cxx" || dep_name == "fortran" {
            notes.push(format!("skipped language virtual depends_on({raw})"));
            continue;
        }
        dependencies.push(ForeignDep {
            name: spack_dep_to_eb_name(&dep_name),
            pin,
            role,
        });
    }

    let summary = spack_docstring(text);

    Ok(ForeignRecipe {
        format: ForeignFormat::Spack,
        name,
        version,
        homepage,
        source_url,
        source_filename: None,
        sha256,
        summary,
        dependencies,
        notes,
    })
}

fn spack_class_to_pkg_name(class_name: &str) -> String {
    // Zlib -> zlib, PyNumpy -> py-numpy (rough)
    if let Some(rest) = class_name.strip_prefix("Py") {
        if !rest.is_empty() && rest.starts_with(char::is_uppercase) {
            return format!("py-{}", camel_to_kebab(rest));
        }
    }
    camel_to_kebab(class_name)
}

fn camel_to_kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn spack_dep_to_eb_name(name: &str) -> String {
    // gmake -> make is residual; keep spack name for honesty
    name.to_string()
}

fn split_spack_spec(spec: &str) -> (String, Option<String>) {
    // foo@1.2.3, foo+bar, foo
    if let Some((n, ver)) = spec.split_once('@') {
        let ver = ver.split(['+', '~', '%']).next().unwrap_or(ver);
        (n.to_string(), Some(ver.to_string()))
    } else {
        let n = spec.split(['+', '~', '%']).next().unwrap_or(spec);
        (n.to_string(), None)
    }
}

fn spack_string_attr(text: &str, attr: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?m)^\s*{attr}\s*=\s*[\"']([^\"']+)[\"']"#
    ))
    .ok()?;
    re.captures(text)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

fn spack_docstring(text: &str) -> Option<String> {
    // class ...:\n    """doc"""
    let re = Regex::new(r#"(?s)class\s+\w+\s*\([^)]*\)\s*:\s*(?:r)?\"\"\"(.+?)\"\"\""#).ok()?;
    re.captures(text).and_then(|c| {
        c.get(1)
            .map(|m| m.as_str().split_whitespace().collect::<Vec<_>>().join(" "))
    })
}

/// Write ingest result using letter/name layout when `out` is a directory.
pub fn write_ingest_result(
    result: &IngestResult,
    toolchain: &Toolchain,
    out: Option<&Path>,
    out_dir: Option<&Path>,
) -> Result<std::path::PathBuf, ForeignError> {
    let path = if let Some(p) = out {
        p.to_path_buf()
    } else if let Some(dir) = out_dir {
        let letter = easyconfig_letter_dir(&result.recipe.name);
        let sub = dir.join(&letter).join(&result.recipe.name);
        std::fs::create_dir_all(&sub).map_err(|e| {
            ForeignError::Io(format!("mkdir {}: {e}", sub.display()))
        })?;
        let base = companion_easyconfig_basename(
            &result.recipe.name,
            &result.recipe.version,
            toolchain,
            None,
        );
        sub.join(base)
    } else {
        std::path::PathBuf::from(&result.filename)
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ForeignError::Io(format!("mkdir {}: {e}", parent.display()))
        })?;
    }
    std::fs::write(&path, &result.text)
        .map_err(|e| ForeignError::Io(format!("write {}: {e}", path.display())))?;
    Ok(path)
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

    fn foss() -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: "2024a".into(),
        }
    }

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
    fn parse_spack_zlib_fields() {
        let r = parse_spack_package(SPACK_ZLIB).expect("spack");
        assert_eq!(r.name, "zlib");
        assert_eq!(r.version, "1.3.1");
        assert!(r.source_url.as_ref().unwrap().contains("zlib-1.3.1"));
        assert!(r.sha256.is_some());
        let names: Vec<_> = r.dependencies.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"gmake"), "{names:?}");
        assert!(!names.iter().any(|n| *n == "c"), "language virtual skipped");
    }

    #[test]
    fn emit_conda_tracks_fixture_and_reparses() {
        let r = parse_conda_forge(CONDA_ZLIB).unwrap();
        let out = emit_easyconfig_from_foreign(&r, &foss());
        assert!(out.text.contains("name = 'zlib'"));
        assert!(out.text.contains("version = '1.3.1'"));
        assert!(out.text.contains("zlib-1.3.1.tar.gz"));
        assert!(out.text.contains("libgcc-ng") || out.text.contains("make"));
        assert!(out.text.contains("conda-forge") || out.text.contains("ingested"));
        let resolved = crate::eb_parse::resolve_easyconfig_str(&out.text)
            .expect("emitted eb must re-parse");
        assert_eq!(resolved.name, "zlib");
        assert_eq!(resolved.version, "1.3.1");
        assert_eq!(resolved.toolchain.name, "foss");
        assert_eq!(resolved.toolchain.version, "2024a");
    }

    #[test]
    fn emit_spack_tracks_fixture_and_reparses() {
        let r = parse_spack_package(SPACK_ZLIB).unwrap();
        let out = emit_easyconfig_from_foreign(&r, &foss());
        assert!(out.text.contains("name = 'zlib'"));
        assert!(out.text.contains("version = '1.3.1'"));
        assert!(out.text.contains("zlib-1.3.1.tar.gz"));
        assert!(out.text.contains("gmake") || out.text.contains("spack"));
        let resolved = crate::eb_parse::resolve_easyconfig_str(&out.text)
            .expect("emitted eb must re-parse");
        assert_eq!(resolved.name, "zlib");
        assert_eq!(resolved.version, "1.3.1");
    }

    #[test]
    fn detect_format_by_filename() {
        assert_eq!(
            detect_foreign_format(Path::new("/x/meta.yaml")),
            Some(ForeignFormat::CondaForge)
        );
        assert_eq!(
            detect_foreign_format(Path::new("package.py")),
            Some(ForeignFormat::Spack)
        );
        assert_eq!(detect_foreign_format(Path::new("foo.eb")), None);
    }
}
