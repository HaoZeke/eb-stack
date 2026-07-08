//! Emit next-generation EasyBuild easyconfigs from an existing recipe.
//!
//! Surgical assignment/list rewrites: only `toolchain`, optional application
//! `version`, and named dependency / build-dependency version fields change.
//! All other source bytes stay verbatim.

use crate::domain::Toolchain;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EmitError {
    #[error("missing name = ... in source easyconfig")]
    MissingName,
    #[error("missing version = ... in source easyconfig (and no --version override)")]
    MissingVersion,
    #[error("missing toolchain = {{...}} in source easyconfig")]
    MissingToolchain,
    #[error("rewrite failed: {0}")]
    Rewrite(String),
}

/// Parameters for producing a next-generation easyconfig.
#[derive(Debug, Clone)]
pub struct EmitParams {
    /// Target toolchain generation (always rewritten).
    pub toolchain: Toolchain,
    /// Optional new application version; when `None`, source `version` is kept.
    pub version: Option<String>,
    /// Per-dependency (and build-dependency) version overrides keyed by package name.
    ///
    /// If the override string starts with a comparison operator (`==`, `>=`, `<=`,
    /// `>`, `<`, `!`), it replaces the entire version field of matching tuples.
    /// Otherwise the operator already present on the source version (if any) is
    /// preserved and only the version token is replaced.
    pub dep_versions: HashMap<String, String>,
}

/// Result of a next-generation emit: rewritten text and conventional basename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitResult {
    pub text: String,
    /// EasyBuild filename: `{name}-{version}-{toolchain_name}-{toolchain_version}.eb`
    pub filename: String,
}

/// Derive the conventional EasyBuild easyconfig basename.
pub fn easyconfig_filename(name: &str, version: &str, toolchain: &Toolchain) -> String {
    format!(
        "{}-{}-{}-{}.eb",
        name, version, toolchain.name, toolchain.version
    )
}

/// Emit next-generation easyconfig text and conventional filename from source text.
pub fn emit_next_generation(source: &str, params: &EmitParams) -> Result<EmitResult, EmitError> {
    let name = assign_string_raw(source, "name").ok_or(EmitError::MissingName)?;
    let app_version = match &params.version {
        Some(v) => v.clone(),
        None => assign_string_raw(source, "version").ok_or(EmitError::MissingVersion)?,
    };

    // Ensure source has a toolchain assignment we can rewrite.
    if !source.lines().any(|l| {
        let t = l.trim();
        t.starts_with("toolchain") && t.contains('=')
    }) {
        return Err(EmitError::MissingToolchain);
    }

    let mut text = source.to_string();
    text = rewrite_toolchain(&text, &params.toolchain)?;
    if params.version.is_some() {
        text = rewrite_string_assign(&text, "version", &app_version)?;
    }
    if !params.dep_versions.is_empty() {
        text = rewrite_dep_list_versions(&text, "dependencies", &params.dep_versions)?;
        text = rewrite_dep_list_versions(&text, "builddependencies", &params.dep_versions)?;
    }

    let filename = easyconfig_filename(&name, &app_version, &params.toolchain);
    Ok(EmitResult { text, filename })
}

/// Read a source file and emit the next-generation recipe.
pub fn emit_next_generation_from_path(
    path: &std::path::Path,
    params: &EmitParams,
) -> Result<EmitResult, EmitError> {
    let source = std::fs::read_to_string(path).map_err(|e| {
        EmitError::Rewrite(format!("read {}: {}", path.display(), e))
    })?;
    emit_next_generation(&source, params)
}

fn assign_string_raw(src: &str, key: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix(key)
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('='))
            .map(str::trim_start)
        else {
            continue;
        };
        let bytes = rest.as_bytes();
        if bytes.is_empty() {
            continue;
        }
        let q = bytes[0];
        if q == b'\'' || q == b'"' {
            if let Some(end) = rest[1..].find(q as char) {
                return Some(rest[1..1 + end].to_string());
            }
        }
    }
    None
}

/// Rewrite `key = '...'` or `key = "..."` keeping the original quote style.
fn rewrite_string_assign(src: &str, key: &str, new_val: &str) -> Result<String, EmitError> {
    // No backreferences (regex crate): match single- and double-quoted forms.
    let re_s = regex::Regex::new(&format!(
        r#"(?m)^(?P<prefix>\s*{}\s*=\s*)'(?P<old>[^']*)'"#,
        regex::escape(key)
    ))
    .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let re_d = regex::Regex::new(&format!(
        r#"(?m)^(?P<prefix>\s*{}\s*=\s*)"(?P<old>[^"]*)""#,
        regex::escape(key)
    ))
    .map_err(|e| EmitError::Rewrite(e.to_string()))?;

    if re_s.is_match(src) {
        let out = re_s.replace(src, |caps: &regex::Captures| {
            format!("{}'{}'", &caps["prefix"], new_val)
        });
        return Ok(out.into_owned());
    }
    if re_d.is_match(src) {
        let out = re_d.replace(src, |caps: &regex::Captures| {
            format!("{}\"{}\"", &caps["prefix"], new_val)
        });
        return Ok(out.into_owned());
    }
    Err(EmitError::Rewrite(format!(
        "no {key} = '...' assignment to rewrite"
    )))
}

/// Rewrite `toolchain = {'name': ..., 'version': ...}` (single- or multi-line).
fn rewrite_toolchain(src: &str, tc: &Toolchain) -> Result<String, EmitError> {
    // Locate the toolchain assignment span (from "toolchain" through matching '}').
    let lines: Vec<&str> = src.lines().collect();
    let mut start_line = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("toolchain") && t.contains('=') && !t.starts_with("toolchainopts") {
            start_line = Some(i);
            break;
        }
    }
    let start_line = start_line.ok_or(EmitError::MissingToolchain)?;

    // Rebuild with name/version values substituted inside the dict, preserving
    // surrounding whitespace and quote characters on each key/value pair.
    let mut capturing = false;
    let mut depth = 0i32;
    let mut span_start = None;
    let mut span_end = None;
    let bytes = src.as_bytes();
    // Find byte offset of start of the start_line.
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut off = 0usize;
    for line in &lines {
        line_offsets.push(off);
        off += line.len();
        if off < src.len() {
            // account for '\n' (and optionally '\r' already in line for \r\n we use lines())
            if src.as_bytes().get(off) == Some(&b'\n') {
                off += 1;
            } else if src.as_bytes().get(off) == Some(&b'\r') {
                off += 1;
                if src.as_bytes().get(off) == Some(&b'\n') {
                    off += 1;
                }
            }
        }
    }

    let search_from = line_offsets[start_line];
    let mut i = search_from;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if !capturing {
            // find '=' then '{'
            if c == '=' {
                // skip whitespace after =
                let mut j = i + 1;
                while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && bytes[j] == b'{' {
                    capturing = true;
                    depth = 1;
                    span_start = Some(j);
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                span_end = Some(i + 1);
                break;
            }
        }
        i += 1;
    }

    let (s, e) = match (span_start, span_end) {
        (Some(s), Some(e)) => (s, e),
        _ => {
            return Err(EmitError::Rewrite(
                "could not locate toolchain dict braces".into(),
            ))
        }
    };

    let dict = &src[s..e];
    // No backreferences: handle '…' and "…" separately.
    let name_s = regex::Regex::new(r#"(['"]name['"]\s*:\s*)'([^']*)'"#)
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let name_d = regex::Regex::new(r#"(['"]name['"]\s*:\s*)"([^"]*)""#)
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let ver_s = regex::Regex::new(r#"(['"]version['"]\s*:\s*)'([^']*)'"#)
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let ver_d = regex::Regex::new(r#"(['"]version['"]\s*:\s*)"([^"]*)""#)
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;

    let has_name = name_s.is_match(dict) || name_d.is_match(dict);
    let has_ver = ver_s.is_match(dict) || ver_d.is_match(dict);
    if !has_name || !has_ver {
        return Err(EmitError::Rewrite(
            "toolchain dict missing name/version string entries".into(),
        ));
    }

    let dict = if name_s.is_match(dict) {
        name_s
            .replace(dict, |caps: &regex::Captures| {
                format!("{}'{}'", &caps[1], tc.name)
            })
            .into_owned()
    } else {
        name_d
            .replace(dict, |caps: &regex::Captures| {
                format!("{}\"{}\"", &caps[1], tc.name)
            })
            .into_owned()
    };
    let dict = if ver_s.is_match(&dict) {
        ver_s
            .replace(&dict, |caps: &regex::Captures| {
                format!("{}'{}'", &caps[1], tc.version)
            })
            .into_owned()
    } else {
        ver_d
            .replace(&dict, |caps: &regex::Captures| {
                format!("{}\"{}\"", &caps[1], tc.version)
            })
            .into_owned()
    };

    let mut out = String::with_capacity(src.len() + 16);
    out.push_str(&src[..s]);
    out.push_str(&dict);
    out.push_str(&src[e..]);
    Ok(out)
}

/// Whether `s` starts with a version comparison operator.
fn override_has_operator(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("==")
        || s.starts_with(">=")
        || s.starts_with("<=")
        || s.starts_with("!=")
        || s.starts_with('>')
        || s.starts_with('<')
        || s.starts_with('!')
}

/// Split a source version field into optional operator prefix and version token.
fn split_op_version(version_field: &str) -> (&str, &str) {
    let v = version_field.trim();
    for op in ["==", ">=", "<=", "!=", ">", "<", "!"] {
        if let Some(rest) = v.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("", v)
}

/// Apply override policy to a single dependency version field.
fn apply_version_override(source_field: &str, override_val: &str) -> String {
    if override_has_operator(override_val) {
        override_val.to_string()
    } else {
        let (op, _) = split_op_version(source_field);
        format!("{op}{override_val}")
    }
}

/// Rewrite version strings inside `key = [ ... ]` for named deps present in `overrides`.
fn rewrite_dep_list_versions(
    src: &str,
    key: &str,
    overrides: &HashMap<String, String>,
) -> Result<String, EmitError> {
    let re_hdr = regex::Regex::new(&format!(r"(?m)^(\s*{}\s*=\s*\[)", regex::escape(key)))
        .map_err(|e| EmitError::Rewrite(e.to_string()))?;
    let Some(m) = re_hdr.find(src) else {
        // No such list — nothing to rewrite (not an error).
        return Ok(src.to_string());
    };
    let list_open_end = m.end(); // index just after '['
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut i = list_open_end;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                break;
            }
        }
        i += 1;
    }
    if depth != 0 {
        return Err(EmitError::Rewrite(format!(
            "unclosed {key} list"
        )));
    }
    let body = &src[list_open_end..i];
    let new_body = rewrite_dep_tuples_in_body(body, overrides)?;
    let mut out = String::with_capacity(src.len() + 32);
    out.push_str(&src[..list_open_end]);
    out.push_str(&new_body);
    out.push_str(&src[i..]);
    Ok(out)
}

fn rewrite_dep_tuples_in_body(
    body: &str,
    overrides: &HashMap<String, String>,
) -> Result<String, EmitError> {
    // ('Name', 'ver') or ("Name", "ver") — no backreferences (regex crate).
    // Replace only the version (second) string when name is in overrides.
    let re = regex::Regex::new(
        r#"\(\s*'(?P<n1>[^']+)'\s*,\s*'(?P<v1>[^']+)'|\(\s*"(?P<n2>[^"]+)"\s*,\s*"(?P<v2>[^"]+)""#,
    )
    .map_err(|e| EmitError::Rewrite(e.to_string()))?;

    let out = re.replace_all(body, |caps: &regex::Captures| {
        let full = caps.get(0).unwrap().as_str();
        let (name, ver, ver_name) = if let Some(n) = caps.name("n1") {
            (n.as_str(), caps.name("v1").unwrap().as_str(), "v1")
        } else {
            (
                caps.name("n2").unwrap().as_str(),
                caps.name("v2").unwrap().as_str(),
                "v2",
            )
        };
        if let Some(new_ver) = overrides.get(name) {
            let applied = apply_version_override(ver, new_ver);
            let ver_m = caps.name(ver_name).unwrap();
            let start = ver_m.start() - caps.get(0).unwrap().start();
            let end = ver_m.end() - caps.get(0).unwrap().start();
            let mut s = full.to_string();
            s.replace_range(start..end, &applied);
            s
        } else {
            full.to_string()
        }
    });
    Ok(out.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn foss(ver: &str) -> Toolchain {
        Toolchain {
            name: "foss".into(),
            version: ver.into(),
        }
    }

    const MINIMAL: &str = "\
name = 'GROMACS'
version = '2024.1'
toolchain = {'name': 'foss', 'version': '2025a'}
toolchainopts = {'openmp': True, 'usempi': True}
homepage = 'https://www.gromacs.org'
description = \"GROMACS molecular dynamics (fixture for stack upgrade).\"
dependencies = [
    ('OpenBLAS', '0.3.23'),
    ('OpenMPI', '4.1.5'),
    ('FFTW', '3.3.10'),
]
";

    const WITH_BUILDDEPS: &str = "\
name = 'Demo'
version = '1.0'
toolchain = {'name': 'foss', 'version': '2025a'}
homepage = 'https://example.invalid'
dependencies = [
    ('OpenMPI', '>=4.1.5'),
]
builddependencies = [
    ('CMake', '3.26.3'),
]
";

    #[test]
    fn toolchain_only_bump_preserves_rest() {
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: None,
            dep_versions: HashMap::new(),
        };
        let r = emit_next_generation(MINIMAL, &params).expect("emit");
        assert_eq!(r.filename, "GROMACS-2024.1-foss-2025b.eb");
        assert!(r.text.contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
        assert!(r.text.contains("version = '2024.1'"));
        assert!(r.text.contains("toolchainopts = {'openmp': True, 'usempi': True}"));
        assert!(r.text.contains("homepage = 'https://www.gromacs.org'"));
        assert!(r
            .text
            .contains("description = \"GROMACS molecular dynamics (fixture for stack upgrade).\""));
        assert!(r.text.contains("('OpenBLAS', '0.3.23')"));
        assert!(r.text.contains("('OpenMPI', '4.1.5')"));
        assert!(r.text.contains("('FFTW', '3.3.10')"));
        // Non-rewritten lines byte-identical to source counterparts.
        for line in MINIMAL.lines() {
            if line.trim().starts_with("toolchain") && !line.trim().starts_with("toolchainopts") {
                continue;
            }
            assert!(
                r.text.lines().any(|l| l == line),
                "missing preserved line: {line:?}"
            );
        }
    }

    #[test]
    fn version_and_toolchain_rewrite() {
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: Some("2025.0".into()),
            dep_versions: HashMap::new(),
        };
        let r = emit_next_generation(MINIMAL, &params).expect("emit");
        assert_eq!(r.filename, "GROMACS-2025.0-foss-2025b.eb");
        assert!(r.text.contains("version = '2025.0'"));
        assert!(r.text.contains("toolchain = {'name': 'foss', 'version': '2025b'}"));
        assert!(r.text.contains("name = 'GROMACS'"));
        assert!(r.text.contains("homepage = 'https://www.gromacs.org'"));
    }

    #[test]
    fn dependency_version_overrides() {
        let mut deps = HashMap::new();
        deps.insert("OpenBLAS".into(), "0.3.27".into());
        deps.insert("OpenMPI".into(), "5.0.3".into());
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: Some("2025.0".into()),
            dep_versions: deps,
        };
        let r = emit_next_generation(MINIMAL, &params).expect("emit");
        assert_eq!(r.filename, "GROMACS-2025.0-foss-2025b.eb");
        assert!(r.text.contains("('OpenBLAS', '0.3.27')"));
        assert!(r.text.contains("('OpenMPI', '5.0.3')"));
        // Unmentioned dep unchanged.
        assert!(r.text.contains("('FFTW', '3.3.10')"));
        assert!(r.text.contains("homepage = 'https://www.gromacs.org'"));
    }

    #[test]
    fn preserves_operator_unless_override_has_one() {
        let mut deps = HashMap::new();
        deps.insert("OpenMPI".into(), "4.1.6".into());
        deps.insert("CMake".into(), "3.27.0".into());
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: None,
            dep_versions: deps,
        };
        let r = emit_next_generation(WITH_BUILDDEPS, &params).expect("emit");
        assert_eq!(r.filename, "Demo-1.0-foss-2025b.eb");
        // Operator preserved on OpenMPI.
        assert!(r.text.contains("('OpenMPI', '>=4.1.6')"));
        // builddependencies also rewritten.
        assert!(r.text.contains("('CMake', '3.27.0')"));
        assert!(r.text.contains("homepage = 'https://example.invalid'"));
    }

    #[test]
    fn override_with_operator_replaces_whole_field() {
        let mut deps = HashMap::new();
        deps.insert("OpenMPI".into(), "==5.0.3".into());
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: None,
            dep_versions: deps,
        };
        let r = emit_next_generation(WITH_BUILDDEPS, &params).expect("emit");
        assert!(r.text.contains("('OpenMPI', '==5.0.3')"));
    }

    #[test]
    fn filename_helper_matches_fixture_shape() {
        assert_eq!(
            easyconfig_filename("GROMACS", "2025.0", &foss("2025b")),
            "GROMACS-2025.0-foss-2025b.eb"
        );
    }

    #[test]
    fn double_quoted_style_preserved() {
        let src = "name = \"Pkg\"\nversion = \"1.0\"\ntoolchain = {\"name\": \"foss\", \"version\": \"2025a\"}\n";
        let params = EmitParams {
            toolchain: foss("2025b"),
            version: Some("2.0".into()),
            dep_versions: HashMap::new(),
        };
        let r = emit_next_generation(src, &params).expect("emit");
        assert_eq!(r.filename, "Pkg-2.0-foss-2025b.eb");
        assert!(r.text.contains("version = \"2.0\""));
        assert!(r
            .text
            .contains("toolchain = {\"name\": \"foss\", \"version\": \"2025b\"}"));
    }
}
